// SPDX-License-Identifier: CC0-1.0

//! Every operation this service's model binds is SERVED: the request reaches a handler.
//!
//! The rule under test: an operation the model declares is one this emulator answers for.
//! `build.rs` discovers the handlers from `src/ops/`, but what this service answers from is
//! its own management-event routing table (`src/service/management_events.rs`), so an
//! operation present in the model and in `src/ops/` and ABSENT from that table is answered
//! `501 NotImplemented` — the framework's "this emulator does not serve that operation"
//! signal — however complete its handler is. A model refresh that adds operations is exactly
//! when the two can drift, and nothing else in this crate's suites would notice: a 501 reads
//! like a stub rather than like a wiring mistake.
//!
//! So the subject here is the WIRING, not any operation's behaviour: every bound operation is
//! called, with an empty body, and the only thing asserted is that the answer is not the
//! not-served signal. What each one then makes of an empty request — a refusal naming a
//! required member, or a success — is its own suite's business.

use serde_json::{json, Value};

use crate::wire::{call, emulator};

/// This service's model, as the crate ships it: the one statement of which operations exist.
const MODEL: &str =
    include_str!("../../smithy-model/com-amazonaws-ecr-amazonec2containerregistry-v20150921.json");

/// The wire error code the framework answers with for an operation it does not serve, and the
/// HTTP status that goes with it.
const NOT_SERVED: (u16, &str) = (501, "NotImplemented");

/// Every operation the model's service shape binds, by the name the wire addresses it under.
fn modelled_operations() -> Vec<String> {
    let model: Value = serde_json::from_str(MODEL).expect("this service's model parses as JSON");
    let shapes = model["shapes"]
        .as_object()
        .expect("a Smithy model declares shapes")
        .clone();
    let mut bound: Vec<String> = shapes
        .values()
        .filter(|shape| shape["type"] == json!("service"))
        .flat_map(|service| {
            service["operations"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .filter_map(|operation| {
            operation["target"]
                .as_str()
                .and_then(|target| target.split('#').nth(1))
                .map(str::to_string)
        })
        .collect();
    bound.sort();
    bound.dedup();
    bound
}

/// No operation the model binds answers the not-served signal.
#[tokio::test]
async fn every_modelled_operation_reaches_a_handler() {
    let operations = modelled_operations();
    assert!(
        operations.len() > 1,
        "the model must bind the operations this case walks, got {operations:?}"
    );

    let router = emulator();
    for operation in operations {
        // The emptiest request there is: an operation that models a required member refuses
        // it at the boundary, which is already past the routing this case is about.
        let answer = call(&router, &operation, &json!({})).await;
        let (status, code) = NOT_SERVED;
        assert_ne!(
            (answer.status, answer.error_code()),
            (status, code),
            "case {operation:?}: the model binds this operation, so it must be served rather \
             than answered {code}; add its row to `audited_table` in \
             `src/service/management_events.rs` when its handler exists. Got HTTP {} with {:?}",
            answer.status,
            answer.body
        );
    }
}
