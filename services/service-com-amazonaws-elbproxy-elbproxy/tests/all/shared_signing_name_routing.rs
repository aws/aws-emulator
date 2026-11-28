// SPDX-License-Identifier: CC0-1.0

//! This service mounted BESIDE the Elastic Load Balancing front door it shares its SigV4
//! signing name with.
//!
//! The sibling suite `signing_name_claim.rs` owns the claim table of this service's own
//! surface — which request shapes it claims and which it leaves alone — asked of this
//! service alone. What a single service cannot show is the condition the router actually
//! turns into `500 AmbiguousService`: that no OTHER mounted service claims the same
//! request. So this suite mounts the Classic Elastic Load Balancing service, which owns
//! the shared `elasticloadbalancing` routing key, over the same engine and pins the pair:
//!
//! * a load balancing request on that key has exactly ONE claimant — the front door that
//!   owns the name — whichever signal resolves the key (a credential scope, or the host
//!   label of an anonymous request);
//! * and dispatched through a router carrying both services, such a request is answered in
//!   that model's own `awsQuery` document rather than the router's ambiguity refusal.

use std::sync::Arc;

use service_com_amazonaws_elasticloadbalancing_elasticloadbalancing_v7::EmulatedService as ClassicService;
use service_com_amazonaws_elbproxy_elbproxy::EmulatedService as ProxyService;
use smithy_server_runtime::{MultiServiceRouter, Service};

/// The signing name this model declares, which both Elastic Load Balancing models declare
/// too — the routing key every AWS SDK client of any of them sends.
const SHARED_KEY: &str = "elasticloadbalancing";

/// The region every request below is signed for.
const REGION: &str = "us-east-1";

/// The host an AWS SDK Elastic Load Balancing client addresses.
const LOAD_BALANCING_HOST: &str = "elasticloadbalancing.us-east-1.amazonaws.com";

/// The SigV4 `Authorization` header of a request signed for `signing_name`, as every AWS
/// SDK sends it. No signature is verified here: a claim seam reads only the scope.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// A load balancing request as an `awsQuery` client sends one: the action and the API
/// version as form fields of `POST /`.
fn load_balancing_request(signed_for: Option<&str>) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method("POST")
        .uri(format!("https://{LOAD_BALANCING_HOST}/"))
        .header("host", LOAD_BALANCING_HOST)
        .header("content-type", "application/x-www-form-urlencoded");
    if let Some(signing_name) = signed_for {
        builder = builder.header("authorization", authorization(signing_name));
    }
    builder
        .body(bytes::Bytes::from(
            // The Classic Load Balancer model's own API version and one of its actions.
            "Action=DescribeLoadBalancers&Version=2012-06-01",
        ))
        .expect("a well-formed request")
}

/// Exactly one of the two services mounted under the shared name claims a load balancing
/// request — the condition the router refuses when it fails.
#[test]
fn a_load_balancing_request_has_exactly_one_claimant() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let classic = ClassicService::new(Arc::clone(&engine));
    let proxy = ProxyService::new(engine);

    // (case, the name the request is signed for, or None for an anonymous request whose
    // host label resolves the key)
    let rows: [(&str, Option<&str>); 2] = [
        ("signed for the shared name", Some(SHARED_KEY)),
        ("anonymous, addressed to the endpoint host", None),
    ];

    for (case, signed_for) in rows {
        let parts = load_balancing_request(signed_for).into_parts().0;
        let claimants: Vec<&str> = [
            ("the Elastic Load Balancing front door", classic.can_handle(&parts)),
            ("this service", proxy.can_handle(&parts)),
        ]
        .into_iter()
        .filter_map(|(name, claimed)| claimed.then_some(name))
        .collect();
        assert_eq!(
            claimants,
            vec!["the Elastic Load Balancing front door"],
            "case {case:?}: expected the owning front door to be the only claimant, got \
             {claimants:?}"
        );
    }
}

/// And end to end, with both services on one router over one engine: the owning model
/// answers in its own document.
#[tokio::test]
async fn a_load_balancing_request_still_reaches_its_own_model() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router
        .set_request_authentication(false)
        .register_service(ClassicService::new(Arc::clone(&engine)))
        .register_service(ProxyService::new(engine));

    let response = router.dispatch(load_balancing_request(Some(SHARED_KEY))).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();

    assert_eq!(
        status, 200,
        "case \"classic DescribeLoadBalancers\": the owning model must answer it, got \
         {status} with {body}"
    );
    assert!(
        body.contains("<DescribeLoadBalancersResponse"),
        "case \"classic DescribeLoadBalancers\": expected that model's own awsQuery \
         document, got {status} with {body}"
    );
    assert!(
        !body.contains("AmbiguousService"),
        "case \"classic DescribeLoadBalancers\": the shared name must resolve to one \
         claimant, got {status} with {body}"
    );
}
