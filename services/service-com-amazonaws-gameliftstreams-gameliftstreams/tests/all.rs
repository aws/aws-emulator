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
#[path = "all/admin_shell_not_found.rs"]
mod admin_shell_not_found;
#[path = "all/application_associations.rs"]
mod application_associations;
#[path = "all/control_plane_fidelity.rs"]
mod control_plane_fidelity;
#[path = "all/not_found_messages.rs"]
mod not_found_messages;
#[path = "all/refusal_envelope.rs"]
mod refusal_envelope;
#[path = "all/remove_stream_group_locations.rs"]
mod remove_stream_group_locations;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/stream_group_capacity.rs"]
mod stream_group_capacity;
#[path = "all/stream_group_locations.rs"]
mod stream_group_locations;
#[path = "all/stream_session_admin_shell.rs"]
mod stream_session_admin_shell;
#[path = "all/stream_session_placement.rs"]
mod stream_session_placement;
#[path = "all/stream_url_not_found.rs"]
mod stream_url_not_found;
#[path = "all/stream_urls.rs"]
mod stream_urls;
#[path = "all/stream_workflow.rs"]
mod stream_workflow;
#[path = "all/tags.rs"]
mod tags;
#[path = "all/untag_resource.rs"]
mod untag_resource;
#[path = "all/update_stream_group.rs"]
mod update_stream_group;
