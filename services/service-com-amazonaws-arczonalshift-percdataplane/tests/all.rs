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
#[path = "all/access_denied_precedence.rs"]
mod access_denied_precedence;
#[path = "all/autoshift_observer_notification.rs"]
mod autoshift_observer_notification;
#[path = "all/practice_run_configuration.rs"]
mod practice_run_configuration;
#[path = "all/practice_run_configuration_delete.rs"]
mod practice_run_configuration_delete;
#[path = "all/practice_run_configuration_update.rs"]
mod practice_run_configuration_update;
#[path = "all/refusal_disclosure.rs"]
mod refusal_disclosure;
#[path = "all/refusal_message_rules.rs"]
mod refusal_message_rules;
#[path = "all/registered_managed_resources.rs"]
mod registered_managed_resources;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/zonal_autoshift_configuration.rs"]
mod zonal_autoshift_configuration;
#[path = "all/zonal_shift_lifecycle.rs"]
mod zonal_shift_lifecycle;
