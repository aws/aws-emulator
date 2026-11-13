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
#[path = "all/asset_deletion.rs"]
mod asset_deletion;
#[path = "all/asset_updates.rs"]
mod asset_updates;
#[path = "all/asset_lookup.rs"]
mod asset_lookup;
#[path = "all/data_grant_listing.rs"]
mod data_grant_listing;
#[path = "all/data_grants.rs"]
mod data_grants;
#[path = "all/data_set_notifications.rs"]
mod data_set_notifications;
#[path = "all/data_sets.rs"]
mod data_sets;
#[path = "all/event_actions.rs"]
mod event_actions;
#[path = "all/invalid_parameter_message.rs"]
mod invalid_parameter_message;
#[path = "all/job_lifecycle.rs"]
mod job_lifecycle;
#[path = "all/job_state_conflict_message.rs"]
mod job_state_conflict_message;
#[path = "all/jobs.rs"]
mod jobs;
#[path = "all/no_content_framing.rs"]
mod no_content_framing;
#[path = "all/no_content_wire_probe.rs"]
mod no_content_wire_probe;
#[path = "all/not_found_message.rs"]
mod not_found_message;
#[path = "all/optional_page_size.rs"]
mod optional_page_size;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/revision_assets.rs"]
mod revision_assets;
#[path = "all/revision_deletion.rs"]
mod revision_deletion;
#[path = "all/revision_finalization.rs"]
mod revision_finalization;
#[path = "all/revisions.rs"]
mod revisions;
#[path = "all/s3_data_access_jobs.rs"]
mod s3_data_access_jobs;
#[path = "all/signed_url_uploads.rs"]
mod signed_url_uploads;
#[path = "all/start_job.rs"]
mod start_job;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/timestamp_wire_form.rs"]
mod timestamp_wire_form;
