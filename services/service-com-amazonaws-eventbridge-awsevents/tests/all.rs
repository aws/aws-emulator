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
#[path = "all/api_destination_delivery.rs"]
mod api_destination_delivery;
#[path = "all/api_destination_invocation_parameters.rs"]
mod api_destination_invocation_parameters;
#[path = "all/api_destination_rate_limit_refusal.rs"]
mod api_destination_rate_limit_refusal;
#[path = "all/api_destination_state_settles_with_its_connection.rs"]
mod api_destination_state_settles_with_its_connection;
#[path = "all/api_destination_survives_its_connections_deletion.rs"]
mod api_destination_survives_its_connections_deletion;
#[path = "all/api_destination_update.rs"]
mod api_destination_update;
#[path = "all/archive_and_api_destination.rs"]
mod archive_and_api_destination;
#[path = "all/archive_registry.rs"]
mod archive_registry;
#[path = "all/archive_replay.rs"]
mod archive_replay;
#[path = "all/archive_state_is_settled.rs"]
mod archive_state_is_settled;
#[path = "all/bus_forward_authorization.rs"]
mod bus_forward_authorization;
#[path = "all/bus_forward_failure_accounting.rs"]
mod bus_forward_failure_accounting;
#[path = "all/bus_forward_refusal_telemetry.rs"]
mod bus_forward_refusal_telemetry;
#[path = "all/bus_policy_deny_precedence.rs"]
mod bus_policy_deny_precedence;
#[path = "all/bus_policy_publication_authorization.rs"]
mod bus_policy_publication_authorization;
#[path = "all/client_error_wording.rs"]
mod client_error_wording;
#[path = "all/connection_authorization.rs"]
mod connection_authorization;
#[path = "all/connection_authorization_is_settled_by_its_credentials.rs"]
mod connection_authorization_is_settled_by_its_credentials;
#[path = "all/connection_filter_exclusivity_and_update_completeness.rs"]
mod connection_filter_exclusivity_and_update_completeness;
#[path = "all/connection_lifecycle.rs"]
mod connection_lifecycle;
#[path = "all/connection_oauth_grant.rs"]
mod connection_oauth_grant;
#[path = "all/connection_refusal_wording.rs"]
mod connection_refusal_wording;
#[path = "all/cross_region_bus_target.rs"]
mod cross_region_bus_target;
#[path = "all/dead_letter_condition_classification.rs"]
mod dead_letter_condition_classification;
#[path = "all/dead_letter_error_code.rs"]
mod dead_letter_error_code;
#[path = "all/encrypted_bus_key_state.rs"]
mod encrypted_bus_key_state;
#[path = "all/encrypted_queue_target_key_policy.rs"]
mod encrypted_queue_target_key_policy;
#[path = "all/encrypted_topic_target_key_policy.rs"]
mod encrypted_topic_target_key_policy;
#[path = "all/entry_level_semantics.rs"]
mod entry_level_semantics;
#[path = "all/envelope_time_precision.rs"]
mod envelope_time_precision;
#[path = "all/event_bus_configuration.rs"]
mod event_bus_configuration;
#[path = "all/event_bus_deletion.rs"]
mod event_bus_deletion;
#[path = "all/event_bus_key_policy.rs"]
mod event_bus_key_policy;
#[path = "all/event_bus_lifecycle.rs"]
mod event_bus_lifecycle;
#[path = "all/event_bus_write_semantics.rs"]
mod event_bus_write_semantics;
#[path = "all/fifo_queue_target_delivery.rs"]
mod fifo_queue_target_delivery;
#[path = "all/fixed_refusal_sentences.rs"]
mod fixed_refusal_sentences;
#[path = "all/function_target_grant_takes_effect_on_the_next_event.rs"]
mod function_target_grant_takes_effect_on_the_next_event;
#[path = "all/global_endpoint.rs"]
mod global_endpoint;
#[path = "all/global_endpoint_deletion.rs"]
mod global_endpoint_deletion;
#[path = "all/global_endpoint_identity.rs"]
mod global_endpoint_identity;
#[path = "all/glue_workflow_target_delivery.rs"]
mod glue_workflow_target_delivery;
#[path = "all/handler_execution_role_publication.rs"]
mod handler_execution_role_publication;
#[path = "all/implicit_default_bus.rs"]
mod implicit_default_bus;
#[path = "all/input_transformation.rs"]
mod input_transformation;
#[path = "all/input_validation.rs"]
mod input_validation;
#[path = "all/kinesis_partition_key_json_text.rs"]
mod kinesis_partition_key_json_text;
#[path = "all/kinesis_target_delivery.rs"]
mod kinesis_target_delivery;
#[path = "all/kinesis_target_shard_affinity.rs"]
mod kinesis_target_shard_affinity;
#[path = "all/kinesis_target_stream_consumption.rs"]
mod kinesis_target_stream_consumption;
#[path = "all/lambda_target_handler_records_the_invoking_rule.rs"]
mod lambda_target_handler_records_the_invoking_rule;
#[path = "all/lambda_target_handler_writes_the_event_detail.rs"]
mod lambda_target_handler_writes_the_event_detail;
#[path = "all/log_group_target_delivery.rs"]
mod log_group_target_delivery;
#[path = "all/missing_resource_wording.rs"]
mod missing_resource_wording;
#[path = "all/or_relationship_branches.rs"]
mod or_relationship_branches;
#[path = "all/partner_entry_rejection.rs"]
mod partner_entry_rejection;
#[path = "all/partner_event_source.rs"]
mod partner_event_source;
#[path = "all/pass_role_authorization.rs"]
mod pass_role_authorization;
#[path = "all/pattern_and_connection_filters.rs"]
mod pattern_and_connection_filters;
#[path = "all/pattern_preflight.rs"]
mod pattern_preflight;
#[path = "all/pattern_refusal_location.rs"]
mod pattern_refusal_location;
#[path = "all/pattern_refusal_wording.rs"]
mod pattern_refusal_wording;
#[path = "all/publication_identity_authorization.rs"]
mod publication_identity_authorization;
#[path = "all/queue_target_policy_authorization.rs"]
mod queue_target_policy_authorization;
#[path = "all/regional_bus_namespace.rs"]
mod regional_bus_namespace;
#[path = "all/replay_cancellation.rs"]
mod replay_cancellation;
#[path = "all/replay_filter_arns_scoping.rs"]
mod replay_filter_arns_scoping;
#[path = "all/replay_name_opt_out.rs"]
mod replay_name_opt_out;
#[path = "all/replay_progress_states.rs"]
mod replay_progress_states;
#[path = "all/reserved_event_source.rs"]
mod reserved_event_source;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/routing_counter_metric_reads.rs"]
mod routing_counter_metric_reads;
#[path = "all/rule_fan_out_and_target_repointing.rs"]
mod rule_fan_out_and_target_repointing;
#[path = "all/rule_invocation_metrics.rs"]
mod rule_invocation_metrics;
#[path = "all/rule_lifecycle.rs"]
mod rule_lifecycle;
#[path = "all/rule_match_metrics.rs"]
mod rule_match_metrics;
#[path = "all/rule_metric_dimension_sets.rs"]
mod rule_metric_dimension_sets;
#[path = "all/rule_state_control.rs"]
mod rule_state_control;
#[path = "all/rule_tag_scoped_authorization.rs"]
mod rule_tag_scoped_authorization;
#[path = "all/scheduled_rule_firing.rs"]
mod scheduled_rule_firing;
#[path = "all/state_machine_execution_naming.rs"]
mod state_machine_execution_naming;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/submitted_value_refusals.rs"]
mod submitted_value_refusals;
#[path = "all/target_arn_scoped_authorization.rs"]
mod target_arn_scoped_authorization;
#[path = "all/target_delivery.rs"]
mod target_delivery;
#[path = "all/target_dispatch.rs"]
mod target_dispatch;
#[path = "all/target_response_members.rs"]
mod target_response_members;
#[path = "all/target_role_assumption.rs"]
mod target_role_assumption;
#[path = "all/topic_target_policy_authorization.rs"]
mod topic_target_policy_authorization;
#[path = "all/trace_header_propagation.rs"]
mod trace_header_propagation;
#[path = "all/transformed_input_is_json.rs"]
mod transformed_input_is_json;
