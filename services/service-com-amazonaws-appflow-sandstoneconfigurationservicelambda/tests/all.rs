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
#[path = "all/connector_catalog.rs"]
mod connector_catalog;
#[path = "all/connector_configuration.rs"]
mod connector_configuration;
#[path = "all/connector_describe.rs"]
mod connector_describe;
#[path = "all/flow_concurrent_start.rs"]
mod flow_concurrent_start;
#[path = "all/flow_execution_cancellation.rs"]
mod flow_execution_cancellation;
#[path = "all/flow_lifecycle.rs"]
mod flow_lifecycle;
#[path = "all/flow_principal_identity.rs"]
mod flow_principal_identity;
#[path = "all/flow_run_completion_metadata.rs"]
mod flow_run_completion_metadata;
#[path = "all/flow_run_transfer.rs"]
mod flow_run_transfer;
#[path = "all/flow_state_fidelity.rs"]
mod flow_state_fidelity;
#[path = "all/flow_update.rs"]
mod flow_update;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
