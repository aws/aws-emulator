// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! Put a new suite at `tests/all/<name>.rs` and declare it below, keeping the list
//! sorted.
//!
//! A suite that is not declared here compiles nowhere and its tests never run. A
//! suite left at the top level of `tests/` fails the build.

// `tests/all.rs` is a test-target CRATE ROOT, so a bare `mod` resolves to a SIBLING
// (`tests/state_roundtrip.rs`) — which the layout check refuses. The `#[path]` is what
// points it into `tests/all/`.
#[path = "all/connector_identity_and_conflicts.rs"]
mod connector_identity_and_conflicts;

#[path = "all/create_request_rules.rs"]
mod create_request_rules;

#[path = "all/delete_idempotence.rs"]
mod delete_idempotence;

#[path = "all/identifier_refusals.rs"]
mod identifier_refusals;

#[path = "all/network_connector_lifecycle.rs"]
mod network_connector_lifecycle;

#[path = "all/not_found_message.rs"]
mod not_found_message;

#[path = "all/referenced_resources.rs"]
mod referenced_resources;

#[path = "all/removed_connector_refusal.rs"]
mod removed_connector_refusal;

#[path = "all/state_gate_refusal_wording.rs"]
mod state_gate_refusal_wording;

#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;

#[path = "all/timestamp_and_state_reason_wire.rs"]
mod timestamp_and_state_reason_wire;

#[path = "all/validation_message_wording.rs"]
mod validation_message_wording;
