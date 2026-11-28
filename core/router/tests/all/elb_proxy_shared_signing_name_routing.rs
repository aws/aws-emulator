// SPDX-License-Identifier: CC0-1.0

//! A THIRD service behind the one Elastic Load Balancing signing name must not take the
//! two load balancing APIs' traffic.
//!
//! The ELB Proxy model is signed under `elasticloadbalancing`
//! (`smithy_query(query="service_info")`: `sigv4Name`/`arnNamespace`), the name Classic
//! Elastic Load Balancing and Elastic Load Balancing v2 are signed under too, and the
//! router resolves a request's routing key from the SigV4 credential scope FIRST — so a
//! service that claims requests by that name alone claims its peers' traffic as well, the
//! router finds several claimants and refuses the request with `500 AmbiguousService`
//! instead of serving it. When that service was mounted, an `awsQuery` request that named
//! no ELB Proxy operation at all stopped being served: a load balancing call was answered
//! by a router fault rather than by the model that owns it.
//!
//! The rule this suite pins: a signing name shared by several emulated services resolves
//! to EXACTLY ONE of them for the traffic of each API — the Classic model owns the shared
//! name, and serves both load balancing API versions from the single endpoint AWS serves
//! them on, while the ELBv2 model stays reachable under the routing key that names IT; a
//! peer that speaks ANOTHER wire claims only its own wire form and never the `awsQuery`
//! traffic of this name. The answer in every row is the owning model's own — an awsQuery
//! envelope a query client can parse, never a router-level refusal.
//!
//! The rows span what the rule generalises over: which API the request is for, which
//! routing key names it, whether the envelope names an API version at all, and whether the
//! action is declared by one model or by both.
//!
//! This suite needs the WHOLE router: the ambiguity exists only when the services are
//! mounted together, so a single-service test cannot see it. Assembling the router costs
//! stack (see `boot_serve.rs`), so it is built — and the requests driven — on an
//! explicitly sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every signed request below is signed for.
const REGION: &str = "us-east-1";

/// The API version the Classic Elastic Load Balancing model declares.
const CLASSIC_VERSION: &str = "2012-06-01";

/// The API version the Elastic Load Balancing v2 model declares.
const V2_VERSION: &str = "2015-12-01";

/// The signing name the three models share.
const SHARED_NAME: &str = "elasticloadbalancing";

/// The routing key that names the ELBv2 model itself.
const V2_KEY: &str = "elasticloadbalancingv2";

/// One call: the key it is addressed under, the `Action` and `Version` its envelope names,
/// and what the ONE model that owns it must answer.
struct Row {
    case: &'static str,
    /// The routing key the request's credential scope names.
    key: &'static str,
    action: &'static str,
    /// The `Version` the envelope carries, or `None` to send no `Version` field at all.
    version: Option<&'static str>,
    expected_status: u16,
    /// A string only the OWNING model's answer can carry, so the row identifies which
    /// model answered rather than only that one did.
    marker: &'static str,
}

/// The rows.
const ROWS: &[Row] = &[
    Row {
        // The shared name, for the Classic API: one claimant, answered out of the Classic
        // model's own shapes.
        case: "a Classic action signed for the shared name",
        key: SHARED_NAME,
        action: "DescribeLoadBalancers",
        version: Some(CLASSIC_VERSION),
        expected_status: 200,
        marker: "DescribeLoadBalancersResult",
    },
    Row {
        // The same name, for the ELBv2 API: the envelope's version names the model, and
        // the front door that owns the shared name serves it.
        case: "an ELBv2 action signed for the shared name",
        key: SHARED_NAME,
        action: "DescribeTargetGroups",
        version: Some(V2_VERSION),
        expected_status: 200,
        marker: "DescribeTargetGroupsResult",
    },
    Row {
        // An action only the ELBv2 model declares, with NO version field at all: the
        // envelope names no API, so the front door that owns the shared name reads it as
        // its OWN API version, where that action is declared by nothing — and words the
        // modelled refusal naming both. This is the request a third claimant took down,
        // and what matters here is that ONE model answered it in its own envelope.
        case: "an ELBv2-only action on the shared name with no Version field",
        key: SHARED_NAME,
        action: "DescribeTargetGroups",
        version: None,
        expected_status: 400,
        marker: "InvalidAction",
    },
    Row {
        // An action BOTH models declare, at the Classic version: the version decides which
        // model serves it, and the two models require DIFFERENT members of it — so the
        // member the refusal names says which model answered. Here it is the Classic
        // model's own.
        case: "an action both models declare, at the Classic version",
        key: SHARED_NAME,
        action: "DescribeTags",
        version: Some(CLASSIC_VERSION),
        expected_status: 400,
        marker: "loadBalancerNames",
    },
    Row {
        // The same action at the ELBv2 version: the other model serves it, and names the
        // member IT requires.
        case: "an action both models declare, at the ELBv2 version",
        key: SHARED_NAME,
        action: "DescribeTags",
        version: Some(V2_VERSION),
        expected_status: 400,
        marker: "resourceArns",
    },
    Row {
        // The ELBv2 model under the key that names IT, which the shared name must not
        // have taken over.
        case: "an ELBv2 action under the key that names the ELBv2 model",
        key: V2_KEY,
        action: "DescribeTargetGroups",
        version: Some(V2_VERSION),
        expected_status: 200,
        marker: "DescribeTargetGroupsResult",
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
fn every_load_balancing_request_reaches_exactly_one_service() {
    std::thread::Builder::new()
        .name("elb-proxy-routing".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("build a current-thread runtime");
            runtime.block_on(async {
                let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
                let mut router = build_router(engine);
                // Signature verification belongs to the emulator, not to this check:
                // these rows are about which service claims a request.
                router.set_request_authentication(false);

                for row in ROWS {
                    let case = row.case;
                    let mut body = format!("Action={}", row.action);
                    if let Some(version) = row.version {
                        body.push_str(&format!("&Version={version}"));
                    }
                    let request = http::Request::builder()
                        .method("POST")
                        .uri("https://localhost/")
                        .header("host", "localhost")
                        .header("content-type", "application/x-www-form-urlencoded")
                        .header("authorization", authorization(row.key))
                        .body(bytes::Bytes::from(body))
                        .expect("the request is well formed");

                    let response = router.dispatch(request).await;
                    let status = response.status().as_u16();
                    let body = String::from_utf8_lossy(response.body().as_ref()).to_string();

                    assert!(
                        !body.contains("AmbiguousService"),
                        "case {case:?}: several services claimed one request: expected \
                         exactly one claimant, got {status} with {body}"
                    );
                    assert!(
                        !body.contains("UnknownService"),
                        "case {case:?}: no service claimed the request: expected exactly \
                         one claimant, got {status} with {body}"
                    );
                    // Whatever the answer is, it is an awsQuery envelope — XML, which a
                    // query client can parse — and never the router's JSON document.
                    assert!(
                        body.trim_start().starts_with('<') && !body.contains("__type"),
                        "case {case:?}: an awsQuery client must get XML, got {status} with \
                         {body}"
                    );
                    assert_eq!(
                        status, row.expected_status,
                        "case {case:?}: the owning model must answer HTTP {}, got {status} \
                         with {body}",
                        row.expected_status
                    );
                    assert!(
                        body.contains(row.marker),
                        "case {case:?}: expected the owning model's answer to carry {:?}, \
                         got {status} with {body}",
                        row.marker
                    );
                }
            });
        })
        .expect("spawn the routing thread")
        .join()
        .expect("every load balancing request must reach exactly one service");
}
