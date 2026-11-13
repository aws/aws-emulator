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
#[path = "all/control_plane_stream_reads.rs"]
mod control_plane_stream_reads;
#[path = "all/get_media.rs"]
mod get_media;
#[path = "all/ingest_plane_addressing.rs"]
mod ingest_plane_addressing;
#[path = "all/media_endpoint_grouping.rs"]
mod media_endpoint_grouping;
#[path = "all/put_media_ingest.rs"]
mod put_media_ingest;
#[path = "all/read_plane_addressing.rs"]
mod read_plane_addressing;
#[path = "all/route_claim.rs"]
mod route_claim;
#[path = "all/route_claims.rs"]
mod route_claims;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
