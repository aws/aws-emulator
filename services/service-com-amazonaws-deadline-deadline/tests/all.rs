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
#[path = "all/batch_get_job.rs"]
mod batch_get_job;
#[path = "all/budget.rs"]
mod budget;
#[path = "all/create_queue_fleet_association.rs"]
mod create_queue_fleet_association;
#[path = "all/date_time_wire_form.rs"]
mod date_time_wire_form;
#[path = "all/delete_fleet.rs"]
mod delete_fleet;
#[path = "all/delete_limit.rs"]
mod delete_limit;
#[path = "all/delete_storage_profile.rs"]
mod delete_storage_profile;
#[path = "all/farm_queue_job.rs"]
mod farm_queue_job;
#[path = "all/fleet.rs"]
mod fleet;
#[path = "all/job_run_status.rs"]
mod job_run_status;
#[path = "all/limit.rs"]
mod limit;
#[path = "all/list_limits.rs"]
mod list_limits;
#[path = "all/not_found_message_template.rs"]
mod not_found_message_template;
#[path = "all/queue_fleet_association.rs"]
mod queue_fleet_association;
#[path = "all/queue_limit_association.rs"]
mod queue_limit_association;
#[path = "all/resource_not_found_wording.rs"]
mod resource_not_found_wording;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/storage_profile.rs"]
mod storage_profile;
#[path = "all/update_fleet.rs"]
mod update_fleet;
#[path = "all/worker.rs"]
mod worker;
