// SPDX-License-Identifier: CC0-1.0

//! THREE emulated services sign as `elasticloadbalancing`, and each claims only its own
//! traffic.
//!
//! The two Elastic Load Balancing query APIs share that signing name with each other — the
//! rule for that pair is the sibling suite
//! `elb_classic_and_v2_signing_name_routing` — and the ELB Proxy model
//! (`smithy_query(query="service_info")`: `sigv4Name` `elasticloadbalancing`,
//! `endpointPrefix` `elb-proxy`, protocol `rpcv2Cbor`) publishes the same signing name
//! again. The router resolves a request's key from the SigV4 credential scope first, so all
//! three resolve to the one key, and a service that claims on that key ALONE claims its
//! peers' traffic: the router, finding more than one claimant, answers `500
//! AmbiguousService` — a code that exists in no AWS model, in a JSON body neither an
//! awsQuery nor an rpcv2Cbor client can parse — so none of the three is reachable.
//!
//! The rule pinned here: a service under a shared signing name claims a request only when
//! the request also addresses that service's OWN modelled wire surface. For the two query
//! APIs that surface is the root path every awsQuery action posts to, with the envelope's
//! `Version` naming which API serves it; for the `rpcv2Cbor` peer it is the
//! `/service/<Service>/operation/<Operation>` path, which NAMES the service. So exactly one
//! service claims each request, and a request that addresses none of the three surfaces is
//! the router's honest `404 UnknownService` rather than an answer from a service that does
//! not serve it.
//!
//! Two layers are asserted: the claim predicate itself, over a table of addressings, and
//! the whole router's dispatch, so a row proves the request reached the one model that owns
//! it. Assembling the router costs stack (see `boot_serve.rs`), so the dispatch rows run on
//! an explicitly sized thread, as the sibling suite's do.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use smithy_server_runtime::Service;

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every signed request below is signed for.
const REGION: &str = "us-east-1";

/// The API version the Classic model declares.
const CLASSIC_VERSION: &str = "2012-06-01";

/// The API version the ELBv2 model declares.
const V2_VERSION: &str = "2015-12-01";

/// The `rpcv2Cbor` path of an ELB Proxy operation: that protocol posts
/// `POST /service/{Service}/operation/{Operation}`, naming the service shape `ELBProxy`.
fn proxy_path(operation: &str) -> String {
    format!("/service/ELBProxy/operation/{operation}")
}

/// The SigV4 `Authorization` header of a request signed for `signing_name`, as every AWS
/// SDK sends it. The signature is never checked here: the claim predicate reads only the
/// credential scope.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// The claim table: it asks each of the three services'
/// [`smithy_server_runtime::Service::can_handle`] directly, so a row pins both that the
/// right service claims a request and that NO other does — the condition the router turns
/// into `500 AmbiguousService` when it fails.
#[test]
fn exactly_one_of_the_three_elasticloadbalancing_services_claims_each_request() {
    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
    let classic =
        service_com_amazonaws_elasticloadbalancing_elasticloadbalancing_v7::EmulatedService::new(
            Arc::clone(&engine),
        );
    let v2 =
        service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::EmulatedService::new(
            Arc::clone(&engine),
        );
    let proxy = service_com_amazonaws_elbproxy_elbproxy::EmulatedService::new(engine);

    // (case, path, the signing name the scope names, the services that must claim it)
    let rows: &[(&str, String, &str, &[&str])] = &[
        (
            "an awsQuery action on the shared endpoint's root path",
            "/".to_string(),
            "elasticloadbalancing",
            &["classic"],
        ),
        (
            "an awsQuery action signed for the ELBv2 model's own key",
            "/".to_string(),
            "elasticloadbalancingv2",
            &["elbv2"],
        ),
        (
            "an rpcv2Cbor request naming the ELB Proxy service",
            proxy_path("ListProxies"),
            "elasticloadbalancing",
            &["elbproxy"],
        ),
        (
            // Another operation of the same service: the claim is on the service the path
            // names, not on one operation of it.
            "an rpcv2Cbor request naming ELB Proxy's create",
            proxy_path("CreateProxy"),
            "elasticloadbalancing",
            &["elbproxy"],
        ),
        (
            // A path that names a service nothing of this family is: claimed by nobody,
            // which is the router's `404 UnknownService` and not an ambiguity.
            "an rpcv2Cbor request naming a service this family does not hold",
            "/service/ELBMirror/operation/ListProxies".to_string(),
            "elasticloadbalancing",
            &[],
        ),
        (
            // Neither query model binds a URI at all, so a path outside the root is no
            // model's surface either.
            "a path no Elastic Load Balancing model publishes",
            "/2015-12-01/load-balancers".to_string(),
            "elasticloadbalancing",
            &[],
        ),
    ];

    for (case, path, signing_name, expected) in rows {
        let parts = http::Request::builder()
            .method("POST")
            .uri(path.as_str())
            .header("host", "localhost:9297")
            .header("authorization", authorization(signing_name))
            .body(bytes::Bytes::new())
            .expect("a well-formed request")
            .into_parts()
            .0;
        let claimants: Vec<&str> = [
            ("classic", classic.can_handle(&parts)),
            ("elbv2", v2.can_handle(&parts)),
            ("elbproxy", proxy.can_handle(&parts)),
        ]
        .into_iter()
        .filter_map(|(name, claimed)| claimed.then_some(name))
        .collect();
        assert_eq!(
            claimants,
            expected.to_vec(),
            "case {case:?} (POST {path} signed for {signing_name}): expected it to be \
             claimed by exactly {expected:?}, got {claimants:?}"
        );
    }
}

/// One dispatched request and what the ONE model that owns it must answer.
struct Dispatched {
    case: &'static str,
    /// The path the request is sent to.
    path: String,
    /// The `rpcv2Cbor` framing headers, for the rows addressed in that protocol.
    cbor: bool,
    /// The form body of an awsQuery row, empty for a CBOR one.
    body: String,
    expected_status: u16,
    /// Text the answer must carry, which is what says WHICH model answered.
    expected_body_contains: &'static [&'static str],
}

#[test]
fn each_service_of_the_shared_signing_name_answers_its_own_requests() {
    std::thread::Builder::new()
        .name("elb-family-routing".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);

            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a current-thread runtime");

            let calls = [
                Dispatched {
                    case: "the Classic model's own API version",
                    path: "/".to_string(),
                    cbor: false,
                    body: format!("Action=DescribeLoadBalancers&Version={CLASSIC_VERSION}"),
                    expected_status: 200,
                    expected_body_contains: &["<DescribeLoadBalancersResponse"],
                },
                Dispatched {
                    case: "the ELBv2 model's API version on the same endpoint",
                    path: "/".to_string(),
                    cbor: false,
                    body: format!("Action=DescribeLoadBalancers&Version={V2_VERSION}"),
                    expected_status: 200,
                    expected_body_contains: &["<DescribeLoadBalancersResponse"],
                },
                Dispatched {
                    // ELB Proxy's operations are scaffolds, so the honest answer is its
                    // `501 NotImplemented` — reaching that answer at all is what the
                    // ambiguity refusal made impossible.
                    case: "an rpcv2Cbor request naming the ELB Proxy service",
                    path: proxy_path("ListProxies"),
                    cbor: true,
                    body: String::new(),
                    expected_status: 501,
                    expected_body_contains: &["NotImplemented"],
                },
                Dispatched {
                    // No service of this family claims it, so the router says so rather
                    // than one of them answering for an endpoint it does not serve.
                    case: "an rpcv2Cbor request naming a service this family does not hold",
                    path: "/service/ELBMirror/operation/ListProxies".to_string(),
                    cbor: true,
                    body: String::new(),
                    expected_status: 404,
                    expected_body_contains: &["UnknownService"],
                },
            ];

            for call in &calls {
                let case = call.case;
                let mut builder = http::Request::builder()
                    .method("POST")
                    .uri(call.path.as_str())
                    .header("host", "localhost:9297")
                    .header("authorization", authorization("elasticloadbalancing"));
                builder = if call.cbor {
                    builder
                        .header("content-type", "application/cbor")
                        .header("smithy-protocol", "rpc-v2-cbor")
                } else {
                    builder.header("content-type", "application/x-www-form-urlencoded")
                };
                let request = builder
                    .body(bytes::Bytes::from(call.body.clone()))
                    .expect("a well-formed request");
                let response = runtime.block_on(router.dispatch(request));
                let status = response.status().as_u16();
                let rendered = String::from_utf8_lossy(response.body()).to_string();

                // The routing failure the shared signing name used to cause may never
                // happen: more than one claimant is `500 AmbiguousService`.
                assert!(
                    !rendered.contains("AmbiguousService"),
                    "case {case:?}: the request must reach at most one service, got \
                     {status} with {rendered}"
                );
                assert_eq!(
                    status, call.expected_status,
                    "case {case:?}: expected HTTP {}, got {status} with {rendered}",
                    call.expected_status
                );
                for expected in call.expected_body_contains {
                    assert!(
                        rendered.contains(expected),
                        "case {case:?}: expected {expected:?} in the answer, got {status} \
                         with {rendered}"
                    );
                }
            }
        })
        .expect("spawn the routing thread")
        .join()
        .expect("every service of the shared signing name must answer its own requests");
}
