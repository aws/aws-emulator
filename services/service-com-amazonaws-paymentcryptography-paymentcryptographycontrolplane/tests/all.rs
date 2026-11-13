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
#[path = "all/alias_binding_lifecycle.rs"]
mod alias_binding_lifecycle;
#[path = "all/alias_name_conflict_sentence.rs"]
mod alias_name_conflict_sentence;
#[path = "all/client_fault_envelope.rs"]
mod client_fault_envelope;
#[path = "all/client_error_status.rs"]
mod client_error_status;
#[path = "all/alias_registry_existence.rs"]
mod alias_registry_existence;
#[path = "all/certificate_signing_request.rs"]
mod certificate_signing_request;
#[path = "all/export_and_replication_parameters.rs"]
mod export_and_replication_parameters;
#[path = "all/export_key.rs"]
mod export_key;
#[path = "all/key_lifecycle.rs"]
mod key_lifecycle;
#[path = "all/key_modes_of_use_discriminator.rs"]
mod key_modes_of_use_discriminator;
#[path = "all/key_state_refusal.rs"]
mod key_state_refusal;
#[path = "all/key_usage_and_replication.rs"]
mod key_usage_and_replication;
#[path = "all/resource_policy.rs"]
mod resource_policy;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/stop_key_usage.rs"]
mod stop_key_usage;
#[path = "all/validation_refusal_message.rs"]
mod validation_refusal_message;
