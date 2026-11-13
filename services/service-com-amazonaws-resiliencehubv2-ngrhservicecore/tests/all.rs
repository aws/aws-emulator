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
#[path = "all/constraint_report_wording.rs"]
mod constraint_report_wording;
#[path = "all/effective_policy_values.rs"]
mod effective_policy_values;
#[path = "all/invoker_role_trust.rs"]
mod invoker_role_trust;
#[path = "all/policy_lifecycle.rs"]
mod policy_lifecycle;
#[path = "all/policy_update_and_deletion.rs"]
mod policy_update_and_deletion;
#[path = "all/service_function_lifecycle.rs"]
mod service_function_lifecycle;
#[path = "all/service_lifecycle.rs"]
mod service_lifecycle;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/test_template_catalog.rs"]
mod test_template_catalog;
