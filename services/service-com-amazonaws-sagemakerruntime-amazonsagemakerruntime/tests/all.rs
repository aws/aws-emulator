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
#[path = "all/async_input_source.rs"]
mod async_input_source;
#[path = "all/endpoint_invocation.rs"]
mod endpoint_invocation;
#[path = "all/endpoint_registry.rs"]
mod endpoint_registry;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/invocation_authorization.rs"]
mod invocation_authorization;
#[path = "all/invoke_endpoint.rs"]
mod invoke_endpoint;
#[path = "all/invoke_endpoint_async.rs"]
mod invoke_endpoint_async;
#[path = "all/invoke_endpoint_with_response_stream.rs"]
mod invoke_endpoint_with_response_stream;
#[path = "all/pattern_clause_quoting.rs"]
mod pattern_clause_quoting;
#[path = "all/refusal_envelope.rs"]
mod refusal_envelope;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
