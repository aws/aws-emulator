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
#[path = "all/agent_space_lifecycle.rs"]
mod agent_space_lifecycle;
#[path = "all/agent_space_not_found.rs"]
mod agent_space_not_found;
#[path = "all/agent_space_not_found_refusal.rs"]
mod agent_space_not_found_refusal;
#[path = "all/asset_content_export.rs"]
mod asset_content_export;
#[path = "all/asset_file_listing.rs"]
mod asset_file_listing;
#[path = "all/asset_file_updates.rs"]
mod asset_file_updates;
#[path = "all/asset_files.rs"]
mod asset_files;
#[path = "all/asset_metadata_and_deletion.rs"]
mod asset_metadata_and_deletion;
#[path = "all/asset_versions.rs"]
mod asset_versions;
#[path = "all/assets.rs"]
mod assets;
#[path = "all/backlog_task_refusal_arms.rs"]
mod backlog_task_refusal_arms;
#[path = "all/in_space_resource_refusals.rs"]
mod in_space_resource_refusals;
#[path = "all/operator_app.rs"]
mod operator_app;
#[path = "all/probe_mime.rs"]
mod probe_mime;
#[path = "all/recommendation_reads.rs"]
mod recommendation_reads;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/trigger_refusal_arms.rs"]
mod trigger_refusal_arms;
#[path = "all/trigger_refusal_wire_envelope.rs"]
mod trigger_refusal_wire_envelope;
#[path = "all/triggers.rs"]
mod triggers;
