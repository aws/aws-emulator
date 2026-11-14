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
#[path = "all/arn_pattern_refusal_clauses.rs"]
mod arn_pattern_refusal_clauses;
#[path = "all/connector_lifecycle.rs"]
mod connector_lifecycle;
#[path = "all/connector_prerequisites.rs"]
mod connector_prerequisites;
#[path = "all/directory_registration_authorization.rs"]
mod directory_registration_authorization;
#[path = "all/directory_registration_lifecycle.rs"]
mod directory_registration_lifecycle;
// The shared harness every suite drives the service through: one copy of the
// router, the request builder, the `@httpLabel` encoding and the error-code read.
#[path = "all/harness.rs"]
mod harness;
#[path = "all/pagination_token_refusals.rs"]
mod pagination_token_refusals;
#[path = "all/pattern_refusal_messages.rs"]
mod pattern_refusal_messages;
#[path = "all/resource_not_found_messages.rs"]
mod resource_not_found_messages;
#[path = "all/service_principal_name_lifecycle.rs"]
mod service_principal_name_lifecycle;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/template_name_grammar.rs"]
mod template_name_grammar;
