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
#[path = "all/already_exists_wording.rs"]
mod already_exists_wording;
#[path = "all/client_error_status.rs"]
mod client_error_status;
#[path = "all/dataset_export.rs"]
mod dataset_export;
#[path = "all/dataset_group_lifecycle.rs"]
mod dataset_group_lifecycle;
#[path = "all/dataset_type_casing.rs"]
mod dataset_type_casing;
#[path = "all/dataset_type_echo.rs"]
mod dataset_type_echo;
#[path = "all/inference_pipeline.rs"]
mod inference_pipeline;
#[path = "all/list_schemas.rs"]
mod list_schemas;
#[path = "all/refusal_wording.rs"]
mod refusal_wording;
#[path = "all/schema_required_fields.rs"]
mod schema_required_fields;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
