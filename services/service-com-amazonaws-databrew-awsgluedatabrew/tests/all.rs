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
#[path = "all/databrew_core_loop.rs"]
mod databrew_core_loop;
#[path = "all/databrew_harness.rs"]
mod databrew_harness;
#[path = "all/dataset_in_use_refusal.rs"]
mod dataset_in_use_refusal;
#[path = "all/dataset_update_lifecycle.rs"]
mod dataset_update_lifecycle;
#[path = "all/enum_value_set_order.rs"]
mod enum_value_set_order;
#[path = "all/project_delete.rs"]
mod project_delete;
#[path = "all/project_lifecycle.rs"]
mod project_lifecycle;
#[path = "all/project_not_found_wording.rs"]
mod project_not_found_wording;
#[path = "all/recipe_already_attached_refusal.rs"]
mod recipe_already_attached_refusal;
#[path = "all/recipe_job_execution.rs"]
mod recipe_job_execution;
#[path = "all/recipe_job_output_object_headers.rs"]
mod recipe_job_output_object_headers;
#[path = "all/recipe_job_schema_evaluation.rs"]
mod recipe_job_schema_evaluation;
#[path = "all/recipe_lifecycle.rs"]
mod recipe_lifecycle;
#[path = "all/recipe_publishing.rs"]
mod recipe_publishing;
#[path = "all/recipe_version_batch_delete.rs"]
mod recipe_version_batch_delete;
#[path = "all/recipe_version_held_message.rs"]
mod recipe_version_held_message;
#[path = "all/recipe_version_listing.rs"]
mod recipe_version_listing;
#[path = "all/recipe_version_resolution.rs"]
mod recipe_version_resolution;
#[path = "all/recipe_version_single_delete.rs"]
mod recipe_version_single_delete;
#[path = "all/recipe_version_working_refusal_invariance.rs"]
mod recipe_version_working_refusal_invariance;
#[path = "all/recipe_version_working_refusal_wording.rs"]
mod recipe_version_working_refusal_wording;
#[path = "all/recipe_working_update.rs"]
mod recipe_working_update;
#[path = "all/resource_not_found_message.rs"]
mod resource_not_found_message;
#[path = "all/schedule_delete.rs"]
mod schedule_delete;
#[path = "all/schedule_lifecycle.rs"]
mod schedule_lifecycle;
#[path = "all/schedule_refusal_wording.rs"]
mod schedule_refusal_wording;
#[path = "all/schedule_update.rs"]
mod schedule_update;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
