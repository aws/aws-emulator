// SPDX-License-Identifier: CC0-1.0

//! ELB Proxy is a THIRD emulated service behind the Elastic Load Balancing signing name.
//!
//! Its model signs `elasticloadbalancing` (`smithy_query(query="service_info")`:
//! `sigv4Name`/`arnNamespace`), the name the Classic Elastic Load Balancing and the ELBv2
//! models already share, while its own identity is `ELB Proxy`, served at endpoint prefix
//! `elb-proxy` and speaking rpcv2Cbor rather than awsQuery. A service that claims on the
//! shared signing name ALONE claims its peers' traffic too, and the router rejects a
//! request with more than one claimant rather than serving it by an arbitrary service:
//! `500 AmbiguousService`, which left every Elastic Load Balancing API unreachable.
//!
//! The rule this suite pins, end to end through the whole router: under one shared signing
//! name every request reaches EXACTLY ONE service, told apart by the wire surface it
//! addresses — the rpcv2Cbor envelope that NAMES this model
//! (`POST /service/ELBProxy/operation/<Operation>`) reaches it, and the awsQuery envelope
//! (`POST /` with an `Action` form field) reaches the Elastic Load Balancing front door,
//! at either API version it serves.
//!
//! The claim seam itself is asked directly by that service's own
//! `tests/all/signing_name_claim.rs`; what only the assembled router can show is that no
//! request has two claimants or none, which is what this suite drives. The sibling
//! `elb_classic_and_v2_signing_name_routing` suite is the same rule for the two
//! query-protocol models.
//!
//! Assembling the router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on an explicitly sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every request below is signed for.
const REGION: &str = "us-east-1";

/// The signing name all three models carry.
const SHARED_SIGNING_NAME: &str = "elasticloadbalancing";

/// The path an rpcv2Cbor request to the ELB Proxy model carries: that protocol posts to
/// `/service/<service shape>/operation/<operation>`, naming the service in the path, and
/// `ListProxies` is one of this model's own operations.
const PROXY_LIST_PATH: &str = "/service/ELBProxy/operation/ListProxies";

/// The router-level refusals, neither of which may ever answer a request: several
/// claimants, and none at all.
const ROUTING_REFUSALS: [&str; 2] = ["AmbiguousService", "UnknownService"];

/// A SigV4 `Authorization` header naming `signing_name` as its credential scope's service
/// — the signal the router reads first, and what every AWS SDK sends.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=00"
    )
}

/// One call: the wire surface it addresses under the shared signing name, and what the ONE
/// service that owns that surface must answer.
struct Call {
    case: &'static str,
    path: &'static str,
    /// `Some(body)` for an awsQuery envelope (form-encoded), `None` for the rpcv2Cbor
    /// envelope, which carries an empty CBOR map and that protocol's own header.
    query_body: Option<&'static str>,
    expected_status: u16,
    /// A fragment the answer must carry, for a row that pins WHAT answered.
    expected_body_contains: &'static [&'static str],
}

#[test]
fn elb_proxy_and_the_load_balancing_front_door_route_apart_under_one_signing_name() {
    std::thread::Builder::new()
        .name("elb-proxy-routing".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);

            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a current-thread runtime");

            // The rows differ in the wire surface the request addresses, which is the one
            // thing that tells the three services sharing the signing name apart.
            let calls: &[Call] = &[
                Call {
                    // The rpcv2Cbor envelope that NAMES the ELB Proxy model: it reaches
                    // that model, whose operations are still scaffolded, so the answer is
                    // the scaffold's own 501 — reaching a stub is reaching the service.
                    case: "the rpcv2Cbor envelope naming the ELB Proxy model",
                    path: PROXY_LIST_PATH,
                    query_body: None,
                    expected_status: 501,
                    expected_body_contains: &[],
                },
                Call {
                    // The awsQuery envelope at the Classic API version: the front door that
                    // owns the shared name answers it, in its own XML.
                    case: "the awsQuery envelope at the Classic API version",
                    path: "/",
                    query_body: Some("Action=DescribeLoadBalancers&Version=2012-06-01"),
                    expected_status: 200,
                    expected_body_contains: &["<DescribeLoadBalancersResponse"],
                },
                Call {
                    // And at the ELBv2 API version, which that same front door serves out
                    // of the ELBv2 model.
                    case: "the awsQuery envelope at the ELBv2 API version",
                    path: "/",
                    query_body: Some("Action=DescribeLoadBalancers&Version=2015-12-01"),
                    expected_status: 200,
                    expected_body_contains: &["<DescribeLoadBalancersResponse"],
                },
            ];

            for call in calls {
                let case = call.case;
                let mut builder = http::Request::builder()
                    .method("POST")
                    .uri(format!("http://localhost{}", call.path))
                    .header("host", "localhost")
                    .header("authorization", authorization(SHARED_SIGNING_NAME));
                let body = match call.query_body {
                    Some(form) => {
                        builder = builder
                            .header("content-type", "application/x-www-form-urlencoded");
                        bytes::Bytes::from(form)
                    }
                    None => {
                        builder = builder
                            .header("content-type", "application/cbor")
                            .header("smithy-protocol", "rpc-v2-cbor");
                        // An empty CBOR map: every member of that operation's input is
                        // optional, so the request names none.
                        bytes::Bytes::from_static(&[0xA0])
                    }
                };
                let request = builder.body(body).expect("a well-formed request");
                let response = runtime.block_on(router.dispatch(request));
                let status = response.status().as_u16();
                let rendered = String::from_utf8_lossy(response.body()).to_string();

                for refusal in ROUTING_REFUSALS {
                    assert!(
                        !rendered.contains(refusal),
                        "case {case:?}: the request must reach exactly one service, got \
                         {status} with {rendered}"
                    );
                }
                assert_eq!(
                    status, call.expected_status,
                    "case {case:?}: the owning service must answer HTTP {}, got {status} with \
                     {rendered}",
                    call.expected_status
                );
                for fragment in call.expected_body_contains {
                    assert!(
                        rendered.contains(fragment),
                        "case {case:?}: the answer must carry {fragment:?}, got {status} with \
                         {rendered}"
                    );
                }
            }
        })
        .expect("the routing thread spawns")
        .join()
        .expect("every request under the shared signing name must reach exactly one service");
}
