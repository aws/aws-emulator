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
#[path = "all/application_lifecycle.rs"]
mod application_lifecycle;
#[path = "all/application_mode_and_maintenance_window.rs"]
mod application_mode_and_maintenance_window;
#[path = "all/application_version_reads.rs"]
mod application_version_reads;
#[path = "all/body_timestamp_class.rs"]
mod body_timestamp_class;
#[path = "all/conditional_token_identity.rs"]
mod conditional_token_identity;
#[path = "all/error_envelope_wording.rs"]
mod error_envelope_wording;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/modeled_client_errors.rs"]
mod modeled_client_errors;
#[path = "all/ops_name_bindings.rs"]
mod ops_name_bindings;
#[path = "all/runtime_configuration_defaults.rs"]
mod runtime_configuration_defaults;
#[path = "all/scratch.rs"]
mod scratch;
#[path = "all/snapshot_delete_application_status.rs"]
mod snapshot_delete_application_status;
#[path = "all/snapshot_deletion.rs"]
mod snapshot_deletion;
#[path = "all/sql_input_and_configuration_defaults.rs"]
mod sql_input_and_configuration_defaults;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/stated_enum_value_set.rs"]
mod stated_enum_value_set;
#[path = "all/studio_notebook_catalog.rs"]
mod studio_notebook_catalog;
#[path = "all/tags_versions_and_snapshots.rs"]
mod tags_versions_and_snapshots;
#[path = "all/update_application_detail_echo.rs"]
mod update_application_detail_echo;
#[path = "all/version_history_summaries.rs"]
mod version_history_summaries;
