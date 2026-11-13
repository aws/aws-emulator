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
#[path = "all/archived_media_read_path.rs"]
mod archived_media_read_path;
#[path = "all/control_plane_stream_resolution.rs"]
mod control_plane_stream_resolution;
#[path = "all/data_plane_identity_authorization.rs"]
mod data_plane_identity_authorization;
#[path = "all/get_dash_streaming_session_url.rs"]
mod get_dash_streaming_session_url;
#[path = "all/get_images_generation.rs"]
mod get_images_generation;
#[path = "all/get_images_request_bounds.rs"]
mod get_images_request_bounds;
#[path = "all/get_images_resize_pair.rs"]
mod get_images_resize_pair;
#[path = "all/get_images_sampling.rs"]
mod get_images_sampling;
#[path = "all/get_images_window_bound.rs"]
mod get_images_window_bound;
#[path = "all/refusal_message_wording.rs"]
mod refusal_message_wording;
#[path = "all/registered_stream_reads.rs"]
mod registered_stream_reads;
#[path = "all/route_claims.rs"]
mod route_claims;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/stream_readable_state.rs"]
mod stream_readable_state;
