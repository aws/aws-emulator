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
#[path = "all/configuration_set_lifecycle.rs"]
mod configuration_set_lifecycle;
#[path = "all/configuration_set_names.rs"]
mod configuration_set_names;
#[path = "all/event_destination_updates.rs"]
mod event_destination_updates;
#[path = "all/legacy_surface_authorization.rs"]
mod legacy_surface_authorization;
#[path = "all/legacy_surface_concurrent_denial.rs"]
mod legacy_surface_concurrent_denial;
#[path = "all/parameter_refusal_wording.rs"]
mod parameter_refusal_wording;
#[path = "all/served_operations.rs"]
mod served_operations;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
