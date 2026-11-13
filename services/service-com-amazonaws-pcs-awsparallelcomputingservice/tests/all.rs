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
#[path = "all/cluster_lifecycle.rs"]
mod cluster_lifecycle;
#[path = "all/cluster_update_and_endpoints.rs"]
mod cluster_update_and_endpoints;
#[path = "all/delete_ordering.rs"]
mod delete_ordering;
#[path = "all/refusal_wording.rs"]
mod refusal_wording;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/subnet_placement.rs"]
mod subnet_placement;
