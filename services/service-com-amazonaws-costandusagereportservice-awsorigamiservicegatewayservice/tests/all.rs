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
#[path = "all/held_report_name_refusal.rs"]
mod held_report_name_refusal;
#[path = "all/modelled_constraints.rs"]
mod modelled_constraints;
#[path = "all/page_token_issuance.rs"]
mod page_token_issuance;
#[path = "all/refusal_envelope.rs"]
mod refusal_envelope;
#[path = "all/refusal_message_text.rs"]
mod refusal_message_text;
#[path = "all/report_definition_lifecycle.rs"]
mod report_definition_lifecycle;
#[path = "all/reported_definition_round_trip.rs"]
mod reported_definition_round_trip;
#[path = "all/service_populated_read_back.rs"]
mod service_populated_read_back;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
