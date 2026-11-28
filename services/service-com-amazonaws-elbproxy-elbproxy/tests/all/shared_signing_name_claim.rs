// SPDX-License-Identifier: CC0-1.0

//! What the FRONT DOOR does with a request under the SigV4 signing name this service
//! shares, driven through a `MultiServiceRouter` rather than asked of the claim seam.
//!
//! `ELBProxy` signs as `elasticloadbalancing` (`smithy_query(query="service_info")`:
//! `sigv4Name`), the name both Elastic Load Balancing models carry as well. Which requests
//! the seam claims is tabled in [`crate::signing_name_claim`]; this suite is the other half
//! of that rule — that the claim is what the ROUTER acts on, so a request addressed to this
//! service's own `rpcv2Cbor` surface is dispatched to it and an Elastic Load Balancing
//! awsQuery request under the same signing name is not dispatched here at all (with only
//! this service mounted, nothing claims it and the router says so).
//!
//! The cross-service proof that the three services route apart with every service mounted
//! is `core/router/tests/all/elb_classic_and_v2_signing_name_routing.rs`.

use std::sync::Arc;

use service_com_amazonaws_elbproxy_elbproxy::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The local name of this model's service shape, which an `rpcv2Cbor` request names in its
/// path (`smithy_query(query="service_info")` reports the service as `ELBProxy`).
const SERVICE_SHAPE: &str = "ELBProxy";

/// An operation this service's model declares, for the URI below.
const OPERATION: &str = "ListProxies";

/// The signing name this service shares with both Elastic Load Balancing models.
const SHARED_SIGNING_NAME: &str = "elasticloadbalancing";

/// The host every request below is addressed to: the endpoint the shared signing name
/// names, which is also the last routing signal an unsigned request carries.
const SHARED_HOST: &str = "elasticloadbalancing.us-east-1.amazonaws.com";

/// A SigV4 `Authorization` header whose credential scope names `signing_name`. Nothing
/// verifies the signature here: routing reads the scope's service field only.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261120/us-east-1/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

#[tokio::test]
async fn the_front_door_serves_this_services_address_and_refuses_what_is_not_its_own() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));

    // A request addressed to this service's own `rpcv2Cbor` surface reaches it: whatever it
    // is answered, the answer is this service's and not the router's "no service claims
    // this" or "several do" refusal.
    let addressed = http::Request::builder()
        .method("POST")
        .uri(format!("/service/{SERVICE_SHAPE}/operation/{OPERATION}"))
        .header("host", SHARED_HOST)
        .header("content-type", "application/cbor")
        .header("smithy-protocol", "rpc-v2-cbor")
        .header("authorization", authorization(SHARED_SIGNING_NAME))
        // An empty CBOR map: this operation's input has no required member.
        .body(bytes::Bytes::from_static(&[0xa0]))
        .expect("a well-formed request");
    let response = router.dispatch(addressed).await;
    let rendered = String::from_utf8_lossy(response.body()).to_string();
    assert!(
        !rendered.contains("UnknownService") && !rendered.contains("AmbiguousService"),
        "case \"this service's own address\": the request must reach this service, got {} \
         with {rendered}",
        response.status()
    );

    // An Elastic Load Balancing awsQuery request, signed for the same name and posted to
    // `/`: this service claims none of that traffic, so with only this service mounted the
    // router finds no claimant for it — which is what keeps it the Elastic Load Balancing
    // front door's when every service is mounted.
    let shared = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", SHARED_HOST)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(SHARED_SIGNING_NAME))
        .body(bytes::Bytes::from_static(
            b"Action=DescribeLoadBalancers&Version=2012-06-01",
        ))
        .expect("a well-formed request");
    let response = router.dispatch(shared).await;
    let status = response.status().as_u16();
    let rendered = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        (status, rendered.contains("UnknownService")),
        (404, true),
        "case \"an Elastic Load Balancing query request\": this service must not claim it, \
         got {status} with {rendered}"
    );
}
