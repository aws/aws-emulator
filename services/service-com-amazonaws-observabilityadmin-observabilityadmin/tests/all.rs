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
#[path = "all/centralization_rules.rs"]
mod centralization_rules;

#[path = "all/delete_s3_table_integration.rs"]
mod delete_s3_table_integration;

#[path = "all/delete_telemetry_pipeline.rs"]
mod delete_telemetry_pipeline;

#[path = "all/front_door_authorization.rs"]
mod front_door_authorization;

#[path = "all/pipeline_configuration_summary.rs"]
mod pipeline_configuration_summary;

#[path = "all/pipeline_mutation_target.rs"]
mod pipeline_mutation_target;

#[path = "all/refusal_contract.rs"]
mod refusal_contract;

#[path = "all/s3_table_integration_listing.rs"]
mod s3_table_integration_listing;

#[path = "all/s3_table_integrations.rs"]
mod s3_table_integrations;

#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;

#[path = "all/success_response_projection.rs"]
mod success_response_projection;

#[path = "all/telemetry_feature_status.rs"]
mod telemetry_feature_status;

#[path = "all/telemetry_pipeline_listing.rs"]
mod telemetry_pipeline_listing;

#[path = "all/telemetry_pipeline_updates.rs"]
mod telemetry_pipeline_updates;

#[path = "all/telemetry_pipelines.rs"]
mod telemetry_pipelines;

#[path = "all/telemetry_rules.rs"]
mod telemetry_rules;

#[path = "all/test_telemetry_pipeline.rs"]
mod test_telemetry_pipeline;

#[path = "all/validate_telemetry_pipeline_configuration.rs"]
mod validate_telemetry_pipeline_configuration;
