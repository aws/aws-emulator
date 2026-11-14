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
#[path = "all/data_plane.rs"]
mod data_plane;
#[path = "all/identity_authorization.rs"]
mod identity_authorization;
#[path = "all/isolines.rs"]
mod isolines;
#[path = "all/modelled_constraint_refusals.rs"]
mod modelled_constraint_refusals;
#[path = "all/probe.rs"]
mod probe;
#[path = "all/refusal_wording.rs"]
mod refusal_wording;
#[path = "all/reported_time_offsets.rs"]
mod reported_time_offsets;
#[path = "all/response_detail.rs"]
mod response_detail;
#[path = "all/road_graph_metrics.rs"]
mod road_graph_metrics;
#[path = "all/route_matrix_boundary.rs"]
mod route_matrix_boundary;
#[path = "all/route_matrix_metrics.rs"]
mod route_matrix_metrics;
#[path = "all/routing.rs"]
mod routing;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/success_response_synthesis.rs"]
mod success_response_synthesis;
#[path = "all/travel_steps.rs"]
mod travel_steps;
#[path = "all/zz_probe_values.rs"]
mod zz_probe_values;
