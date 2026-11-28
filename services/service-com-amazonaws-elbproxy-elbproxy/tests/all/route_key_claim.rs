// SPDX-License-Identifier: CC0-1.0

//! EVERY modelled operation of this service is claimed on this service's own protocol path.
//!
//! The sibling suite `signing_name_claim.rs` states the rule this crate's claim seam
//! implements — under the `elasticloadbalancing` signing name it shares with both Elastic
//! Load Balancing models, this service claims a request only when the request addresses its
//! own `rpcv2Cbor` surface — and drives it over a sample of three of its operations and the
//! traffic shapes it must NOT claim.
//!
//! What this suite adds is the other half of that rule: the claim covers the service's WHOLE
//! modelled surface, not a sample of it. The operation list is read out of the model file
//! rather than written here, so an operation added by a later model refresh is covered the
//! moment it is modelled: every one of them, addressed at
//! `/service/<Service>/operation/<Operation>` and signed for the shared name, is claimed.
//!
//! The claim seam is asked DIRECTLY, for the reason that sibling gives: a single-service
//! router claims everything it is handed, so dispatch cannot see the rule. The cross-service
//! proof that the services sharing the name route apart with every service mounted is
//! `core/router/tests/all/elb_classic_and_v2_signing_name_routing.rs`.

use std::sync::Arc;

use service_com_amazonaws_elbproxy_elbproxy::EmulatedService;
use smithy_server_runtime::Service;

/// The local name of this model's service shape, which an `rpcv2Cbor` request names in its
/// path (`smithy_query(query="service_info")` reports the service as `ELBProxy`).
const SERVICE_SHAPE: &str = "ELBProxy";

/// The signing name this service shares with both Elastic Load Balancing models.
const SHARED_SIGNING_NAME: &str = "elasticloadbalancing";

/// The namespace every shape of this model is in, which the operation ids below carry.
const NAMESPACE: &str = "com.amazonaws.elbproxy#";

/// Every operation this model declares, read off the model file: the shape ids whose shape is
/// an `operation`, as their local names.
fn modelled_operations() -> Vec<String> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-elbproxy-elbproxy.json"
    );
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("the model file reads back: {error}"));
    let parsed: serde_json::Value =
        serde_json::from_str(&text).unwrap_or_else(|error| panic!("the model parses: {error}"));
    let shapes = parsed["shapes"]
        .as_object()
        .unwrap_or_else(|| panic!("a Smithy model declares shapes"))
        .clone();
    let operations: Vec<String> = shapes
        .iter()
        .filter(|(_, shape)| shape["type"].as_str() == Some("operation"))
        .filter_map(|(id, _)| id.strip_prefix(NAMESPACE).map(str::to_string))
        .collect();
    assert!(
        !operations.is_empty(),
        "expected the model to declare operations, got none in {} shapes",
        shapes.len()
    );
    operations
}

/// Whether this service claims a request for `operation` on its own protocol path, signed for
/// the shared name.
fn claims(service: &EmulatedService<engine::InMemoryEngine>, operation: &str) -> bool {
    let (parts, ()) = http::Request::builder()
        .method("POST")
        .uri(format!("/service/{SERVICE_SHAPE}/operation/{operation}"))
        // A loopback host names no service, so the SigV4 credential scope is what the
        // routing key comes from — exactly as it does for an SDK pointed at the emulator.
        .header("host", "localhost:9297")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261120/us-east-1/\
                 {SHARED_SIGNING_NAME}/aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .header("content-type", "application/cbor")
        .body(())
        .expect("build the request")
        .into_parts();
    service.can_handle(&parts)
}

#[tokio::test]
async fn every_modelled_operation_is_claimed_on_this_services_own_path() {
    let service = EmulatedService::new(Arc::new(engine::InMemoryEngine::new()));

    for operation in modelled_operations() {
        let claimed = claims(&service, &operation);
        assert!(
            claimed,
            "case {operation:?}: expected this service to claim its own operation addressed \
             at /service/{SERVICE_SHAPE}/operation/{operation} and signed for \
             {SHARED_SIGNING_NAME}, got can_handle = {claimed}"
        );
    }
}
