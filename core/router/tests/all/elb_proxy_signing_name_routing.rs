// SPDX-License-Identifier: CC0-1.0

//! THREE emulated services share the `elasticloadbalancing` SigV4 signing name — each
//! must still be claimed by exactly one of them in the assembled router.
//!
//! The ELB Proxy model declares `aws.auth#sigv4` name `elasticloadbalancing`
//! (`smithy_query(query="service_info")`: `sigv4Name` `elasticloadbalancing`,
//! `awsService.endpointPrefix` `elb-proxy`) — the same name the Classic Elastic Load
//! Balancing model and the Elastic Load Balancing v2 model declare, and the router
//! resolves a request's routing key from the SigV4 credential scope FIRST. A service that
//! claims on the signing name alone therefore claims its peers' traffic too, the router
//! finds several claimants and rejects the request rather than serving it from an
//! arbitrary one — which made the WHOLE Elastic Load Balancing surface unreachable, every
//! call answering `500 AmbiguousService` in a JSON body an awsQuery client cannot parse.
//!
//! The discriminator is the service a request's own RPC wire form NAMES. ELB Proxy speaks
//! `rpcv2Cbor`, whose every request is `POST /service/ELBProxy/operation/<Operation>` and
//! carries no `X-Amz-Target` at all; both Elastic Load Balancing models speak `awsQuery`,
//! which posts every request to `/` and names its API in the envelope's `Version`. So the
//! first URI path segment splits ELB Proxy from the pair, and
//! `core/router/tests/all/elb_classic_and_v2_signing_name_routing.rs` is where the split
//! BETWEEN the pair is pinned.
//!
//! The rows differ along the wire, the endpoint host, the operation addressed, and which
//! of the three services owns the request. Every row signs the shared name, the way an AWS
//! SDK for any of the three APIs does, because that is the collision under test.
//!
//! Assembling the router costs stack, so it runs on an explicitly sized thread for the
//! same reason `boot_serve.rs` does.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};

/// Keep this at or above the size `boot_serve.rs` uses.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every signed row below is signed for.
const REGION: &str = "us-east-1";

/// The signing name all three services declare, and which every AWS SDK for any of the
/// three APIs sends.
const SHARED_SIGNING_NAME: &str = "elasticloadbalancing";

/// The API version the Classic Elastic Load Balancing model declares.
const CLASSIC_VERSION: &str = "2012-06-01";

/// The API version the Elastic Load Balancing v2 model declares.
const V2_VERSION: &str = "2015-12-01";

/// The header the RPC v2 CBOR specification requires on every request of that wire, and
/// the value it carries.
const SMITHY_PROTOCOL: (&str, &str) = ("smithy-protocol", "rpc-v2-cbor");

/// The media type that wire carries its bodies under.
const CBOR_MEDIA_TYPE: &str = "application/cbor";

/// The CBOR item for an EMPTY map: RFC 8949 major type 5 with a zero-length argument.
///
/// Written as the one byte it is rather than through a CBOR crate because the router's
/// test manifest is re-rendered whenever a service is added, so it carries no dependency
/// of this suite's own. Every row below addresses an operation with no required input
/// member, so an empty map is a complete input.
const EMPTY_CBOR_MAP: &[u8] = &[0xa0];

/// Which wire a row's request arrives on.
#[derive(Clone, Copy)]
enum Wire {
    /// ELB Proxy: the service and the operation in the path, the body as CBOR, and no
    /// `X-Amz-Target`.
    RpcV2Cbor { operation: &'static str },
    /// Both Elastic Load Balancing models: `POST /`, the action and the API version in a
    /// form-encoded body.
    AwsQuery {
        action: &'static str,
        version: &'static str,
    },
}

/// What must happen to a request any of the three services could claim on the shared
/// signing name: exactly one service answers it, with this status and a body carrying this
/// marker — a string ONLY that service's answer can produce, so it identifies which crate
/// served the request.
struct Case {
    name: &'static str,
    host: &'static str,
    wire: Wire,
    who: &'static str,
    status: u16,
    marker: &'static str,
}

const CASES: &[Case] = &[
    Case {
        // The ELB Proxy wire form, signed the way an AWS SDK for ELB Proxy signs it: the
        // shared name. The 501 is this operation's own scaffold answer, which only the ELB
        // Proxy crate can give — reaching it at all is what proves exactly one service
        // claimed the request.
        name: "an ELB Proxy CBOR request signed with the shared name",
        host: "elb-proxy.us-east-1.amazonaws.com",
        wire: Wire::RpcV2Cbor {
            operation: "ListProxies",
        },
        who: "ELB Proxy",
        status: 501,
        marker: "ListProxies",
    },
    Case {
        // The same wire form on a loopback endpoint: the host does not decide, the path
        // does.
        name: "an ELB Proxy CBOR request on a loopback endpoint",
        host: "127.0.0.1:9297",
        wire: Wire::RpcV2Cbor {
            operation: "ListProxies",
        },
        who: "ELB Proxy",
        status: 501,
        marker: "ListProxies",
    },
    Case {
        // And the pair whose traffic the ELB Proxy claim must not swallow: a Classic
        // request on the shared name still reaches the Classic model.
        name: "a Classic Elastic Load Balancing request on the shared name",
        host: "elasticloadbalancing.us-east-1.amazonaws.com",
        wire: Wire::AwsQuery {
            action: "DescribeLoadBalancers",
            version: CLASSIC_VERSION,
        },
        who: "Classic Elastic Load Balancing",
        status: 200,
        marker: "<DescribeLoadBalancersResponse",
    },
    Case {
        // The ELBv2 API version on the same shared front door.
        name: "an ELBv2 request on the shared name",
        host: "elasticloadbalancing.us-east-1.amazonaws.com",
        wire: Wire::AwsQuery {
            action: "DescribeLoadBalancers",
            version: V2_VERSION,
        },
        who: "Elastic Load Balancing v2",
        status: 200,
        marker: "<DescribeLoadBalancersResponse",
    },
];

/// The SigV4 `Authorization` header of a request signed for `signing_name`, as every AWS
/// SDK sends it. The signature is never checked here: the claim predicate reads only the
/// credential scope.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

#[test]
fn elb_proxy_and_the_elastic_load_balancing_apis_route_apart() {
    std::thread::Builder::new()
        .name("elb-proxy-routing".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                // The IO and TIME drivers a service's background work needs, without
                // which tokio panics.
                .enable_all()
                .build()
                .expect("a current-thread runtime");
            runtime.block_on(async {
                let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
                let mut router = build_router(engine);
                // Signature verification belongs to the emulator, not to this check:
                // these rows are about which service claims a request.
                router.set_request_authentication(false);

                for case in CASES {
                    let mut builder = http::Request::builder()
                        .method("POST")
                        .header("host", case.host);
                    let payload: Vec<u8> = match case.wire {
                        Wire::RpcV2Cbor { operation } => {
                            builder = builder
                                .uri(format!(
                                    "https://{}/service/ELBProxy/operation/{operation}",
                                    case.host
                                ))
                                .header("content-type", CBOR_MEDIA_TYPE)
                                .header("accept", CBOR_MEDIA_TYPE)
                                .header(SMITHY_PROTOCOL.0, SMITHY_PROTOCOL.1);
                            EMPTY_CBOR_MAP.to_vec()
                        }
                        Wire::AwsQuery { action, version } => {
                            builder = builder
                                .uri(format!("https://{}/", case.host))
                                .header("content-type", "application/x-www-form-urlencoded");
                            format!("Action={action}&Version={version}").into_bytes()
                        }
                    };
                    builder =
                        builder.header("authorization", authorization(SHARED_SIGNING_NAME));
                    let request = builder
                        .body(bytes::Bytes::from(payload))
                        .expect("the request is well formed");

                    let response = router.dispatch(request).await;
                    let status = response.status().as_u16();
                    // The marker is read out of the bytes as they arrived: a CBOR map
                    // writes its member names and text values as text, so the string that
                    // identifies the answering service is as readable here as in an XML
                    // body.
                    let body = String::from_utf8_lossy(response.body().as_ref()).to_string();

                    assert!(
                        !body.contains("AmbiguousService"),
                        "case {:?}: several services claimed one request: expected exactly \
                         one claimant, got {status} {body}",
                        case.name
                    );
                    assert!(
                        !body.contains("UnknownService"),
                        "case {:?}: no service claimed the request: expected exactly one \
                         claimant, got {status} {body}",
                        case.name
                    );
                    assert_eq!(
                        status, case.status,
                        "case {:?}: expected HTTP {} from {}, got {status} {body}",
                        case.name, case.status, case.who
                    );
                    assert!(
                        body.contains(case.marker),
                        "case {:?}: expected a body from {} carrying {:?}, got {status} {body}",
                        case.name,
                        case.who,
                        case.marker
                    );
                }
            });
        })
        .expect("spawn the routing-check thread")
        .join()
        .expect("ELB Proxy and the Elastic Load Balancing APIs must route apart");
}
