// SPDX-License-Identifier: CC0-1.0

//! THREE emulated services declare the SigV4 signing name `elasticloadbalancing`.
//!
//! Classic Elastic Load Balancing (`sdkId: Elastic Load Balancing`), Elastic Load Balancing
//! v2 (`sdkId: Elastic Load Balancing v2`) and ELB Proxy (`sdkId: ELB Proxy`) all declare
//! `aws.auth#sigv4` name and `arnNamespace` `elasticloadbalancing`
//! (`smithy_query(query="service_info")` on each model). A SigV4 credential scope is the
//! routing signal the front door reads FIRST, so a generated signing-name claim on more than
//! one of them makes every Elastic Load Balancing request claimed twice and answered
//! `500 AmbiguousService` — a code no AWS model carries, in a body no awsQuery client can
//! parse — which takes the Classic and ELBv2 APIs down with it.
//!
//! The rule this suite pins: a signing name shared by several emulated services resolves to
//! EXACTLY ONE claimant per request. The awsQuery traffic of the shared name is the Classic
//! model's, whose own identity IS that name; the ELBv2 model is also reached under the routing
//! key that names IT (`elasticloadbalancingv2`); and ELB Proxy, which speaks `rpcv2Cbor`, is
//! reached on the shared name by the path that protocol names its service in. The sibling
//! suite `elb_classic_and_v2_signing_name_routing.rs` pins how the shared front door then
//! serves both Elastic Load Balancing API versions; this one pins that every one of those
//! addressing forms reaches exactly one service.
//!
//! This suite needs the WHOLE router: the ambiguity exists only when every service is
//! mounted, so a single-service test cannot see it. Assembling the router costs stack (see
//! `boot_serve.rs`), so it is built — and the requests driven — on an explicitly sized
//! thread. The shared-name awsQuery rows deliberately overlap the sibling suite: they are the
//! regression a THIRD claimant of that name reintroduces, so the row that addresses ELB Proxy
//! is asserted beside them rather than on its own.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every request below is addressed in.
const REGION: &str = "us-east-1";

/// The API version the Classic Elastic Load Balancing model declares.
const CLASSIC_VERSION: &str = "2012-06-01";

/// The API version the Elastic Load Balancing v2 model declares.
const V2_VERSION: &str = "2015-12-01";

/// The header the RPC v2 CBOR specification requires on every request of that wire, and the
/// value it carries.
const SMITHY_PROTOCOL: (&str, &str) = ("smithy-protocol", "rpc-v2-cbor");

/// The media type that wire carries its bodies under.
const CBOR_MEDIA_TYPE: &str = "application/cbor";

/// The CBOR encoding of the empty map — the whole input of the `ListProxies` row below, whose
/// every input member is optional. One byte (RFC 8949 major type 5, argument 0), written here
/// rather than with a CBOR crate because the router's test manifest is re-rendered whenever a
/// service is added and so carries no dependency of this suite's own.
const CBOR_EMPTY_MAP: &[u8] = &[0xa0];

/// How a request names the service it is for, and what it carries.
enum Wire {
    /// An awsQuery form body — `Action` and `Version` — signed for a credential scope naming
    /// `key`, which is what every AWS SDK for either Elastic Load Balancing API sends.
    QuerySigned {
        key: &'static str,
        action: &'static str,
        version: &'static str,
    },
    /// An RPC v2 CBOR call signed for a credential scope naming `key`, at the endpoint host of
    /// the model it is for. The operation is named by the path that protocol addresses it with,
    /// `/service/<Service>/operation/<Operation>`, which is the signal that tells this model's
    /// traffic from the awsQuery traffic of the models it shares the signing name with.
    CborSigned {
        key: &'static str,
        host: &'static str,
        service: &'static str,
        operation: &'static str,
    },
}

/// One case: what a failing assertion calls it, how the request is addressed, and the status
/// the ONE service that claims it must answer.
struct Case {
    why: &'static str,
    wire: Wire,
    /// The status the claiming service answers. For a scaffolded operation that is the 501 of
    /// its own not-implemented channel, which is still that service answering rather than the
    /// router refusing to choose one.
    expected_status: u16,
}

/// The rows differ along what the rule generalises over: which of the three models the request
/// is for, which routing signal names it (a credential scope on the shared name, a credential
/// scope on a model's own key, the RPC path naming a model), which wire protocol it speaks
/// (awsQuery and RPC v2 CBOR), and which API version an envelope that carries one names.
const CASES: &[Case] = &[
    Case {
        why: "the Classic API on the shared signing name",
        wire: Wire::QuerySigned {
            key: "elasticloadbalancing",
            action: "DescribeLoadBalancers",
            version: CLASSIC_VERSION,
        },
        expected_status: 200,
    },
    Case {
        why: "the ELBv2 API on the shared signing name",
        wire: Wire::QuerySigned {
            key: "elasticloadbalancing",
            action: "DescribeTargetGroups",
            version: V2_VERSION,
        },
        expected_status: 200,
    },
    Case {
        why: "the ELBv2 API on the routing key that names it",
        wire: Wire::QuerySigned {
            key: "elasticloadbalancingv2",
            action: "DescribeLoadBalancers",
            version: V2_VERSION,
        },
        expected_status: 200,
    },
    Case {
        why: "the ELB Proxy API, which shares the signing name and speaks another protocol",
        wire: Wire::CborSigned {
            key: "elasticloadbalancing",
            host: "elb-proxy.us-east-1.amazonaws.com",
            service: "ELBProxy",
            operation: "ListProxies",
        },
        // Every ELB Proxy operation is still a scaffold, so the service that claims this
        // request answers its own 501 — what matters here is that one service claimed it and
        // that the path named a modelled operation of that model (an unroutable RPC v2 CBOR
        // path is a 404 from the same service).
        expected_status: 501,
    },
];

#[test]
fn every_elastic_load_balancing_front_door_is_claimed_by_exactly_one_service() {
    std::thread::Builder::new()
        .name("elb-shared-name-routing".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                // As in the sibling ELB suite: the IO and TIME drivers a service's background
                // work needs, without which tokio panics.
                .enable_all()
                .build()
                .expect("a current-thread runtime");
            runtime.block_on(async {
                let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
                let mut router = build_router(engine);
                // Signature verification belongs to the emulator, not to this check: these
                // cases are about which service claims a request.
                router.set_request_authentication(false);

                for case in CASES {
                    let why = case.why;
                    let request = match &case.wire {
                        Wire::QuerySigned {
                            key,
                            action,
                            version,
                        } => http::Request::builder()
                            .method("POST")
                            .uri("/")
                            .header("host", "localhost")
                            .header("content-type", "application/x-www-form-urlencoded")
                            .header(
                                "authorization",
                                format!(
                                    "AWS4-HMAC-SHA256 \
                                     Credential=AKIDEXAMPLE/20260101/{REGION}/{key}/aws4_request, \
                                     SignedHeaders=host, Signature=0123456789abcdef"
                                ),
                            )
                            .body(bytes::Bytes::from(format!(
                                "Action={action}&Version={version}"
                            )))
                            .expect("the request is well formed"),
                        Wire::CborSigned {
                            key,
                            host,
                            service,
                            operation,
                        } => http::Request::builder()
                            .method("POST")
                            .uri(format!(
                                "https://{host}/service/{service}/operation/{operation}"
                            ))
                            .header("host", *host)
                            .header("content-type", CBOR_MEDIA_TYPE)
                            .header("accept", CBOR_MEDIA_TYPE)
                            .header(SMITHY_PROTOCOL.0, SMITHY_PROTOCOL.1)
                            .header(
                                "authorization",
                                format!(
                                    "AWS4-HMAC-SHA256 \
                                     Credential=AKIDEXAMPLE/20260101/{REGION}/{key}/aws4_request, \
                                     SignedHeaders=host, Signature=0123456789abcdef"
                                ),
                            )
                            .body(bytes::Bytes::from_static(CBOR_EMPTY_MAP))
                            .expect("the request is well formed"),
                    };

                    let response = router.dispatch(request).await;
                    let status = response.status().as_u16();
                    // The body is read as it arrived: a CBOR map writes its member names as
                    // text, so a router-level refusal is as readable here as in an XML or JSON
                    // body.
                    let rendered = String::from_utf8_lossy(response.body().as_ref()).to_string();

                    // Neither routing failure may happen: `AmbiguousService` (500, several
                    // claimants) nor `UnknownService` (404, none).
                    assert!(
                        !rendered.contains("AmbiguousService")
                            && !rendered.contains("UnknownService"),
                        "case {why:?}: the request must reach exactly one service, got {status} \
                         with {rendered}"
                    );
                    assert_eq!(
                        status, case.expected_status,
                        "case {why:?}: the one service that claims it must answer HTTP {}, got \
                         {status} with {rendered}",
                        case.expected_status
                    );
                }
            });
        })
        .expect("spawn the sized thread")
        .join()
        .expect("Elastic Load Balancing's three front doors must route apart");
}
