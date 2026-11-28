// SPDX-License-Identifier: CC0-1.0

//! Which requests this service CLAIMS, under a SigV4 signing name it does not own alone.
//!
//! `ELBProxy` signs as `elasticloadbalancing`
//! (`smithy_query(query="service_info")`: `sigv4Name: elasticloadbalancing`) — the signing
//! name both Elastic Load Balancing models carry as well, while each of the three serves
//! its own endpoint (`elb-proxy` here) and its own wire surface. The router resolves a
//! request's routing key from the SigV4 credential scope first, so all three see the key
//! `elasticloadbalancing`; a service that claims on the signing name alone claims the
//! others' traffic, and the router refuses every such request as `AmbiguousService`
//! instead of serving it.
//!
//! The rule under test: a service under a shared signing name claims a request only when
//! the routing key names that signing name AND the request addresses THIS service's own
//! modelled wire surface. This model speaks `rpcv2Cbor`, which posts every operation to
//! `/service/<Service>/operation/<Operation>` and names the target service in that path,
//! so the service that path names has to be this one — and the shared key's awsQuery
//! traffic (`POST /` with an `Action` form field, which is how every Elastic Load
//! Balancing SDK call arrives) belongs to the Elastic Load Balancing front door and is
//! claimed here not at all.
//!
//! The rows span what the rule generalises over: this service's own operations on its own
//! protocol path, that path naming another service, the shared key's awsQuery path, an
//! `X-Amz-Target` of an awsJson peer, and this service's own path signed for a different
//! service. The claim seam is asked DIRECTLY, because a single-service router claims
//! everything it is handed, so dispatch cannot see the rule; the cross-service proof that
//! the three route apart with every service mounted is
//! `core/router/tests/all/elb_classic_and_v2_signing_name_routing.rs`.

use std::sync::Arc;

use service_com_amazonaws_elbproxy_elbproxy::EmulatedService;
use smithy_server_runtime::Service;

/// The local name of this model's service shape, which an `rpcv2Cbor` request names in its
/// path (`smithy_query(query="service_info")` reports the service as `ELBProxy`).
pub(crate) const SERVICE_SHAPE: &str = "ELBProxy";

/// The signing name this service shares with both Elastic Load Balancing models.
pub(crate) const SHARED_SIGNING_NAME: &str = "elasticloadbalancing";

/// A SigV4 `Authorization` header whose credential scope names `signing_name`. Nothing
/// verifies the signature here: the claim seam reads the scope's service field only.
pub(crate) fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261120/us-east-1/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// Whether this service claims the described request.
pub(crate) fn claims(
    service: &EmulatedService<engine::InMemoryEngine>,
    uri: &str,
    target: Option<&str>,
    signing_name: &str,
) -> bool {
    let mut builder = http::Request::builder()
        .method("POST")
        .uri(uri)
        // A loopback host names no service, so the SigV4 credential scope is what the
        // routing key comes from — exactly as it does for an SDK pointed at the emulator.
        .header("host", "localhost:9297")
        .header("authorization", authorization(signing_name))
        .header("content-type", "application/cbor");
    if let Some(target) = target {
        builder = builder.header("x-amz-target", target);
    }
    let (parts, ()) = builder.body(()).expect("build the request").into_parts();
    service.can_handle(&parts)
}

/// One row: the case, the request's URI, its `X-Amz-Target`, the service its credential
/// scope names, and whether this service must claim it.
type Row = (
    &'static str,
    String,
    Option<&'static str>,
    &'static str,
    bool,
);

/// RULE: the claim is this service's own `rpcv2Cbor` surface under the shared signing name,
/// and nothing of what the name's other services serve.
#[tokio::test]
async fn the_service_claims_its_own_cbor_surface_and_no_load_balancing_traffic() {
    let service = EmulatedService::new(Arc::new(engine::InMemoryEngine::new()));

    let own = |operation: &str| format!("/service/{SERVICE_SHAPE}/operation/{operation}");
    let rows: Vec<Row> = vec![
        // Its own operations, on its own protocol path: claimed. Three of the eighteen
        // `smithy_query(query="list_operations")` reports, one of them a lifecycle write
        // and one a tagging call.
        (
            "its own CreateProxy on the cbor path",
            own("CreateProxy"),
            None,
            SHARED_SIGNING_NAME,
            true,
        ),
        (
            "its own GetProxyAttributes on the cbor path",
            own("GetProxyAttributes"),
            None,
            SHARED_SIGNING_NAME,
            true,
        ),
        (
            "its own ListTagsForResource on the cbor path",
            own("ListTagsForResource"),
            None,
            SHARED_SIGNING_NAME,
            true,
        ),
        (
            // The path SPACE is this service's, and the operation named on it is this
            // service's to refuse: a request that reaches nobody would be the router's
            // `404 UnknownService` instead.
            "its own cbor path carrying a name no operation has",
            own("NoSuchOperationAtAll"),
            None,
            SHARED_SIGNING_NAME,
            true,
        ),
        (
            // The shared key's awsQuery traffic: `POST /` with the action in the form
            // body, which is how every Elastic Load Balancing call arrives. Claiming it
            // here is what made the router answer `500 AmbiguousService`.
            "the Elastic Load Balancing front door's awsQuery path",
            "/".to_string(),
            None,
            SHARED_SIGNING_NAME,
            false,
        ),
        (
            "an awsJson peer naming itself in X-Amz-Target on the shared path",
            "/".to_string(),
            Some("SomeOtherService.SomeOperation"),
            SHARED_SIGNING_NAME,
            false,
        ),
        (
            // The cbor path of ANOTHER service: it names that service, so this one does
            // not claim it however the request is signed.
            "another service's cbor path",
            "/service/SomeOtherService/operation/CreateProxy".to_string(),
            None,
            SHARED_SIGNING_NAME,
            false,
        ),
        (
            // And this service's own path signed for a different service: the routing key
            // names that service, not the shared one.
            "its own cbor path signed for another service",
            own("CreateProxy"),
            None,
            "s3",
            false,
        ),
    ];

    for (case, uri, target, signing_name, expected) in rows {
        let claimed = claims(&service, &uri, target, signing_name);
        assert_eq!(
            claimed, expected,
            "case {case:?} (POST {uri}, target {target:?}, signed for {signing_name}): \
             expected can_handle to be {expected}, got {claimed}"
        );
    }
}
