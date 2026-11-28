// SPDX-License-Identifier: CC0-1.0

//! EVERY operation this model declares is claimed on its own `rpcv2Cbor` path.
//!
//! `signing_name_claim.rs` states the rule this crate's claim seam implements — a
//! service under a SigV4 signing name it shares claims only requests that
//! address its OWN modelled wire surface — over a sample of this model's
//! operations and the traffic of the name's other services, and owns the request
//! builder both suites drive the seam through. This suite is the CATALOG half of
//! the same rule: the claim must hold for every operation the model declares,
//! not only the few a sample names, so a seam narrowed by one more condition
//! cannot silently stop claiming one of them.
//!
//! The catalog is READ from the model the crate ships rather than restated here,
//! so a model refresh that adds or renames an operation is covered without
//! editing this file.

use crate::signing_name_claim::{claims, SERVICE_SHAPE, SHARED_SIGNING_NAME};

use std::sync::Arc;

use service_com_amazonaws_elbproxy_elbproxy::EmulatedService;

/// The Smithy model this crate is generated from, as shipped beside it.
const MODEL: &str = include_str!("../../smithy-model/com-amazonaws-elbproxy-elbproxy.json");

/// Every operation shape the shipped model declares, by local name: the catalog
/// the generated routing table is built from, read off the model itself.
fn modelled_operations() -> Vec<String> {
    let model: serde_json::Value = serde_json::from_str(MODEL).expect("the shipped model is JSON");
    let shapes = model
        .get("shapes")
        .and_then(serde_json::Value::as_object)
        .expect("the model declares shapes");
    let mut operations: Vec<String> = shapes
        .iter()
        .filter(|(_, shape)| {
            shape.get("type").and_then(serde_json::Value::as_str) == Some("operation")
        })
        .map(|(id, _)| {
            id.rsplit_once('#')
                .map_or_else(|| id.clone(), |(_, name)| name.to_string())
        })
        .collect();
    operations.sort();
    operations
}

#[tokio::test]
async fn every_modelled_operation_is_claimed_on_this_services_own_path() {
    let service = EmulatedService::new(Arc::new(engine::InMemoryEngine::new()));
    let operations = modelled_operations();
    assert!(
        !operations.is_empty(),
        "the shipped model must declare operations, got none"
    );

    for operation in &operations {
        let uri = format!("/service/{SERVICE_SHAPE}/operation/{operation}");
        assert!(
            claims(&service, &uri, None, SHARED_SIGNING_NAME),
            "case {operation:?}: this service must claim POST {uri}, got can_handle false"
        );
        // And the same route under another service's credential scope stays that
        // service's, operation by operation: the claim is the pair, never the
        // path alone.
        assert!(
            !claims(&service, &uri, None, "s3"),
            "case {operation:?}: POST {uri} signed for another service must not be claimed"
        );
    }
}
