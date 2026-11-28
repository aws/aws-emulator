// SPDX-License-Identifier: CC0-1.0

//! What the ROUTER does with a request under the signing name this service shares.
//!
//! The claim rule itself — which requests `can_handle` answers for, under a SigV4 signing
//! name `ELBProxy` shares with both Elastic Load Balancing models — is tabled in
//! [`super::signing_name_claim`], which asks that seam directly. This suite is the other
//! half: the claim decides what a request ADDRESSED to this emulator is answered, so it is
//! driven through `MultiServiceRouter::dispatch` rather than through the seam.
//!
//! RULE: a request on this model's own `rpcv2Cbor` surface
//! (`POST /service/ELBProxy/operation/<Operation>`) reaches this service, and the shared
//! key's awsQuery traffic (`POST /` with the action in the form body, which is how every
//! Elastic Load Balancing call arrives) reaches it not at all — so a router holding this
//! service and the two load-balancing ones has exactly one claimant for each, rather than
//! the `500 AmbiguousService` a signing-name-only claim produced. The cross-service proof
//! with every service mounted is
//! `core/router/tests/all/elb_classic_and_v2_signing_name_routing.rs`.

use std::sync::Arc;

use service_com_amazonaws_elbproxy_elbproxy::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The local name of this model's service shape, which an `rpcv2Cbor` request names in its
/// path (`smithy_query(query="service_info")` reports the service as `ELBProxy`).
const SERVICE_SHAPE: &str = "ELBProxy";

/// The signing name this service shares with both Elastic Load Balancing models.
const SHARED_SIGNING_NAME: &str = "elasticloadbalancing";

/// A SigV4 `Authorization` header whose credential scope names the shared signing name,
/// which is the signal the router resolves a routing key from. Nothing verifies the
/// signature here.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 \
         Credential=AKIDEXAMPLE/20261120/us-east-1/{SHARED_SIGNING_NAME}/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// One request through the router, as `<status> <body>`.
async fn dispatch(router: &MultiServiceRouter, path: &str) -> String {
    let request = http::Request::builder()
        .method("POST")
        // A loopback host names no service, so the credential scope above is what the
        // routing key comes from — exactly as it is for an SDK pointed at the emulator.
        .uri(format!("http://localhost:9297{path}"))
        .header("host", "localhost:9297")
        .header("authorization", authorization())
        .header("smithy-protocol", "rpc-v2-cbor")
        .header("content-type", "application/cbor")
        .body(bytes::Bytes::new())
        .expect("build the request");
    let response = router.dispatch(request).await;
    format!(
        "{} {}",
        response.status().as_u16(),
        String::from_utf8_lossy(response.body())
    )
}

#[tokio::test]
async fn the_router_reaches_this_service_only_through_its_own_wire_form() {
    let mut router = MultiServiceRouter::new();
    // Signature verification is the core's concern; the header is a routing signal here.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::new(
        engine::InMemoryEngine::new(),
    )));

    // Its own protocol path, for operations of three different kinds: the request reaches
    // this service, so neither routing refusal is answered.
    for operation in ["CreateProxy", "GetProxyAttributes", "ListTagsForResource"] {
        let answered = dispatch(&router, &format!("/service/{SERVICE_SHAPE}/operation/{operation}")).await;
        assert!(
            !answered.contains("UnknownService") && !answered.contains("AmbiguousService"),
            "case {operation:?}: this model's own route must reach this service, got {answered}"
        );
    }

    // The Elastic Load Balancing front door's awsQuery form on the shared key, with and
    // without the action in the query string: claimed by nobody here, which is what leaves
    // it to the one service that owns it when all three are mounted.
    for path in ["/", "/?Action=DescribeLoadBalancers"] {
        let answered = dispatch(&router, path).await;
        assert!(
            answered.contains("UnknownService"),
            "case {path:?}: the load-balancing APIs' awsQuery form must not be claimed here, \
             got {answered}"
        );
    }
}
