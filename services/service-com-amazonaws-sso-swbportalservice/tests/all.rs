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
#[path = "all/authentication_precedes_reading.rs"]
mod authentication_precedes_reading;
#[path = "all/bearer_token_gate.rs"]
mod bearer_token_gate;
#[path = "all/credential_expiry.rs"]
mod credential_expiry;
#[path = "all/endpoint_agnostic_routing.rs"]
mod endpoint_agnostic_routing;
#[path = "all/endpoint_host_claiming.rs"]
mod endpoint_host_claiming;
#[path = "all/minted_sign_in.rs"]
mod minted_sign_in;
#[path = "all/page_bound_clamp.rs"]
mod page_bound_clamp;
#[path = "all/page_token_spelling.rs"]
mod page_token_spelling;
#[path = "all/portal_session.rs"]
mod portal_session;
#[path = "all/route_path_agreement.rs"]
mod route_path_agreement;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/unauthorized_rejection.rs"]
mod unauthorized_rejection;
#[path = "all/user_role_assignment.rs"]
mod user_role_assignment;
