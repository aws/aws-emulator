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
#[path = "all/front_door_refusals.rs"]
mod front_door_refusals;
#[path = "all/list_pipes_token_refusal.rs"]
mod list_pipes_token_refusal;
#[path = "all/pipe_body_paths.rs"]
mod pipe_body_paths;
#[path = "all/pipe_bring_up.rs"]
mod pipe_bring_up;
#[path = "all/pipe_configuration_rules.rs"]
mod pipe_configuration_rules;
#[path = "all/pipe_data_plane.rs"]
mod pipe_data_plane;
#[path = "all/pipe_dead_letter.rs"]
mod pipe_dead_letter;
#[path = "all/pipe_fifo_delivery.rs"]
mod pipe_fifo_delivery;
#[path = "all/pipe_description.rs"]
mod pipe_description;
#[path = "all/pipe_desired_state.rs"]
mod pipe_desired_state;
#[path = "all/pipe_dynamodb_stream_source.rs"]
mod pipe_dynamodb_stream_source;
#[path = "all/pipe_execution_role.rs"]
mod pipe_execution_role;
#[path = "all/pipe_filter_body_format.rs"]
mod pipe_filter_body_format;
#[path = "all/pipe_filter_clause_form.rs"]
mod pipe_filter_clause_form;
#[path = "all/pipe_filter_reinstall.rs"]
mod pipe_filter_reinstall;
#[path = "all/pipe_filtered_batch_envelope.rs"]
mod pipe_filtered_batch_envelope;
#[path = "all/pipe_kinesis_stream_source.rs"]
mod pipe_kinesis_stream_source;
#[path = "all/pipe_lambda_target.rs"]
mod pipe_lambda_target;
#[path = "all/pipe_lifecycle.rs"]
mod pipe_lifecycle;
#[path = "all/pipe_lifecycle_conflicts.rs"]
mod pipe_lifecycle_conflicts;
#[path = "all/pipe_listing_state_reason.rs"]
mod pipe_listing_state_reason;
#[path = "all/pipe_metric_attribution.rs"]
mod pipe_metric_attribution;
#[path = "all/pipe_metrics.rs"]
mod pipe_metrics;
#[path = "all/pipe_partial_batch_failure.rs"]
mod pipe_partial_batch_failure;
#[path = "all/pipe_record_envelope.rs"]
mod pipe_record_envelope;
#[path = "all/pipe_refusal_bodies.rs"]
mod pipe_refusal_bodies;
#[path = "all/pipe_reported_configuration.rs"]
mod pipe_reported_configuration;
#[path = "all/pipe_source_checkout.rs"]
mod pipe_source_checkout;
#[path = "all/pipe_state_reason.rs"]
mod pipe_state_reason;
#[path = "all/pipe_tagging.rs"]
mod pipe_tagging;
#[path = "all/pipe_target_parameter_refusal.rs"]
mod pipe_target_parameter_refusal;
#[path = "all/pipe_target_stage_observability.rs"]
mod pipe_target_stage_observability;
#[path = "all/pipe_unassumable_role.rs"]
mod pipe_unassumable_role;
#[path = "all/pipe_update.rs"]
mod pipe_update;
#[path = "all/service_set_tag_keys.rs"]
mod service_set_tag_keys;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_key_rules.rs"]
mod tag_key_rules;
#[path = "all/tag_mutation_no_content.rs"]
mod tag_mutation_no_content;
#[path = "all/tagging_refusal_wording.rs"]
mod tagging_refusal_wording;
#[path = "all/tagging_refusals.rs"]
mod tagging_refusals;
