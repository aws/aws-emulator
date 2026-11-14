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
#[path = "all/connection_endpoint_scope.rs"]
mod connection_endpoint_scope;
#[path = "all/connection_error_envelope.rs"]
mod connection_error_envelope;
#[path = "all/connection_id_validity.rs"]
mod connection_id_validity;
#[path = "all/connection_lifecycle.rs"]
mod connection_lifecycle;
#[path = "all/connections_error_layer.rs"]
mod connections_error_layer;
#[path = "all/invoke_path_routing.rs"]
mod invoke_path_routing;
#[path = "all/manage_connections_authorization.rs"]
mod manage_connections_authorization;
#[path = "all/probe.rs"]
mod probe;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
// Not a suite of its own: the harness `connection_lifecycle` and `probe` share, reached
// as `crate::support`. It is declared here because every file under `tests/all/` must be.
#[path = "all/support.rs"]
mod support;
