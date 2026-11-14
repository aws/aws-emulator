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
#[path = "all/activity_callbacks.rs"]
mod activity_callbacks;
#[path = "all/activity_create_admission.rs"]
mod activity_create_admission;
#[path = "all/activity_heartbeats.rs"]
mod activity_heartbeats;
#[path = "all/activity_lifecycle.rs"]
mod activity_lifecycle;
#[path = "all/activity_listing.rs"]
mod activity_listing;
#[path = "all/activity_task_retries.rs"]
mod activity_task_retries;
#[path = "all/activity_tasks.rs"]
mod activity_tasks;
#[path = "all/activity_timeout_late_answer.rs"]
mod activity_timeout_late_answer;
#[path = "all/activity_worker_hand_off.rs"]
mod activity_worker_hand_off;
#[path = "all/alias_order_and_redrive_context.rs"]
mod alias_order_and_redrive_context;
#[path = "all/alias_repoint_version_resolution.rs"]
mod alias_repoint_version_resolution;
#[path = "all/alias_traffic_split_series.rs"]
mod alias_traffic_split_series;
#[path = "all/arn_validation.rs"]
mod arn_validation;
#[path = "all/backup_task_integration.rs"]
mod backup_task_integration;
#[path = "all/bounded_activity_task_outcomes.rs"]
mod bounded_activity_task_outcomes;
#[path = "all/callback_service_integration.rs"]
mod callback_service_integration;
#[path = "all/caller_identity_authorization.rs"]
mod caller_identity_authorization;
#[path = "all/choice_and_wait_evaluation.rs"]
mod choice_and_wait_evaluation;
#[path = "all/client_error_wrappers.rs"]
mod client_error_wrappers;
#[path = "all/client_refusal_status.rs"]
mod client_refusal_status;
#[path = "all/completed_map_run_refusal.rs"]
mod completed_map_run_refusal;
#[path = "all/control_plane_pass_role.rs"]
mod control_plane_pass_role;
#[path = "all/control_plane_refusal_classes.rs"]
mod control_plane_refusal_classes;
#[path = "all/definition_decides_the_outcome.rs"]
mod definition_decides_the_outcome;
#[path = "all/definition_field_schema.rs"]
mod definition_field_schema;
#[path = "all/definition_finding_subjects.rs"]
mod definition_finding_subjects;
#[path = "all/definition_graph_diagnostics.rs"]
mod definition_graph_diagnostics;
#[path = "all/definition_path_diagnostics.rs"]
mod definition_path_diagnostics;
#[path = "all/definition_refusal_text.rs"]
mod definition_refusal_text;
#[path = "all/definition_state_rules.rs"]
mod definition_state_rules;
#[path = "all/definition_validation.rs"]
mod definition_validation;
#[path = "all/describe_state_machine_for_execution.rs"]
mod describe_state_machine_for_execution;
#[path = "all/distributed_map_children.rs"]
mod distributed_map_children;
#[path = "all/encrypted_execution_writes.rs"]
mod encrypted_execution_writes;
#[path = "all/encrypted_resource_key_configuration.rs"]
mod encrypted_resource_key_configuration;
#[path = "all/error_messages.rs"]
mod error_messages;
#[path = "all/execution_alias_routing.rs"]
mod execution_alias_routing;
#[path = "all/execution_data_encryption.rs"]
mod execution_data_encryption;
#[path = "all/execution_failure_causes.rs"]
mod execution_failure_causes;
#[path = "all/execution_history.rs"]
mod execution_history;
#[path = "all/execution_history_outcomes.rs"]
mod execution_history_outcomes;
#[path = "all/execution_history_state_lifecycle.rs"]
mod execution_history_state_lifecycle;
#[path = "all/execution_interpreter.rs"]
mod execution_interpreter;
#[path = "all/execution_key_state.rs"]
mod execution_key_state;
#[path = "all/execution_observability.rs"]
mod execution_observability;
#[path = "all/execution_redrive.rs"]
mod execution_redrive;
#[path = "all/execution_reports_publish_what_they_settle.rs"]
mod execution_reports_publish_what_they_settle;
#[path = "all/execution_run_key_authorization.rs"]
mod execution_run_key_authorization;
#[path = "all/execution_role_assumption.rs"]
mod execution_role_assumption;
#[path = "all/execution_role_trust_conditions.rs"]
mod execution_role_trust_conditions;
#[path = "all/execution_status_change_events.rs"]
mod execution_status_change_events;
#[path = "all/express_execution_arn_identity.rs"]
mod express_execution_arn_identity;
#[path = "all/express_run_a_job_refusal.rs"]
mod express_run_a_job_refusal;
#[path = "all/form_protocol_integration.rs"]
mod form_protocol_integration;
#[path = "all/history_event_provenance.rs"]
mod history_event_provenance;
#[path = "all/http_task_endpoint.rs"]
mod http_task_endpoint;
#[path = "all/identity_policy_authorization.rs"]
mod identity_policy_authorization;
#[path = "all/integration_failure_error_names.rs"]
mod integration_failure_error_names;
#[path = "all/integration_member_names.rs"]
mod integration_member_names;
#[path = "all/interpreter_data_fidelity.rs"]
mod interpreter_data_fidelity;
#[path = "all/intrinsic_functions.rs"]
mod intrinsic_functions;
#[path = "all/json_parse_diagnostics.rs"]
mod json_parse_diagnostics;
#[path = "all/jsonata_field_diagnostics.rs"]
mod jsonata_field_diagnostics;
#[path = "all/jsonata_states.rs"]
mod jsonata_states;
#[path = "all/lambda_callback_task_token.rs"]
mod lambda_callback_task_token;
#[path = "all/lambda_failure_source_routing.rs"]
mod lambda_failure_source_routing;
#[path = "all/lambda_function_timeout.rs"]
mod lambda_function_timeout;
#[path = "all/lambda_task_alias_version_and_payload.rs"]
mod lambda_task_alias_version_and_payload;
#[path = "all/lambda_task_control_flow.rs"]
mod lambda_task_control_flow;
#[path = "all/lambda_task_function_metrics.rs"]
mod lambda_task_function_metrics;
#[path = "all/list_state_machine_aliases_pages.rs"]
mod list_state_machine_aliases_pages;
#[path = "all/management_event_recording.rs"]
mod management_event_recording;
#[path = "all/map_item_batcher.rs"]
mod map_item_batcher;
#[path = "all/map_item_reader.rs"]
mod map_item_reader;
#[path = "all/map_result_writer.rs"]
mod map_result_writer;
#[path = "all/map_run_failure_budget.rs"]
mod map_run_failure_budget;
#[path = "all/map_run_updates.rs"]
mod map_run_updates;
#[path = "all/map_runs.rs"]
mod map_runs;
#[path = "all/nested_type_discriminator.rs"]
mod nested_type_discriminator;
#[path = "all/optimized_lambda_task_attribution.rs"]
mod optimized_lambda_task_attribution;
#[path = "all/parallel_branch_stop.rs"]
mod parallel_branch_stop;
#[path = "all/parallel_run_a_job.rs"]
mod parallel_run_a_job;
#[path = "all/payload_byte_limits.rs"]
mod payload_byte_limits;
#[path = "all/poller_choice_path_cause.rs"]
mod poller_choice_path_cause;
#[path = "all/refusal_message_fidelity.rs"]
mod refusal_message_fidelity;
#[path = "all/refusal_message_wording.rs"]
mod refusal_message_wording;
#[path = "all/refusal_request_value_templates.rs"]
mod refusal_request_value_templates;
#[path = "all/refusal_vocabulary.rs"]
mod refusal_vocabulary;
#[path = "all/refused_invocation_series.rs"]
mod refused_invocation_series;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/response_request_id.rs"]
mod response_request_id;
#[path = "all/role_log_and_key_authorization.rs"]
mod role_log_and_key_authorization;
#[path = "all/run_a_job_pattern_calls.rs"]
mod run_a_job_pattern_calls;
#[path = "all/run_a_job_target_authorization.rs"]
mod run_a_job_target_authorization;
#[path = "all/running_execution_history.rs"]
mod running_execution_history;
#[path = "all/s3_sdk_task_integration.rs"]
mod s3_sdk_task_integration;
#[path = "all/s3_sdk_task_object_lifecycle.rs"]
mod s3_sdk_task_object_lifecycle;
#[path = "all/saga_catch_error_output_routing.rs"]
mod saga_catch_error_output_routing;
#[path = "all/saga_compensation_on_task_failure.rs"]
mod saga_compensation_on_task_failure;
#[path = "all/sdk_response_envelope.rs"]
mod sdk_response_envelope;
#[path = "all/send_task_failure.rs"]
mod send_task_failure;
#[path = "all/sibling_abort_timing.rs"]
mod sibling_abort_timing;
#[path = "all/sns_publish_task_integration.rs"]
mod sns_publish_task_integration;
#[path = "all/state_data_in_history.rs"]
mod state_data_in_history;
#[path = "all/state_machine_aliases.rs"]
mod state_machine_aliases;
#[path = "all/state_machine_create_options.rs"]
mod state_machine_create_options;
#[path = "all/state_machine_lifecycle.rs"]
mod state_machine_lifecycle;
#[path = "all/state_machine_projection.rs"]
mod state_machine_projection;
#[path = "all/state_machine_version_delete.rs"]
mod state_machine_version_delete;
#[path = "all/state_machine_version_publish.rs"]
mod state_machine_version_publish;
#[path = "all/state_machine_versions.rs"]
mod state_machine_versions;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/states_metric_families.rs"]
mod states_metric_families;
#[path = "all/stuck_state_cause.rs"]
mod stuck_state_cause;
#[path = "all/sync_execution.rs"]
mod sync_execution;
// `sync_execution_data_events` pins the CLASS of a synchronous run: CloudTrail records it as a
// DATA event of the state machine it ran, delivered only into the bucket of a trail whose
// selectors admit it and never into the account's event history.
#[path = "all/sync_execution_data_events.rs"]
mod sync_execution_data_events;
#[path = "all/sync_execution_qualified_arn.rs"]
mod sync_execution_qualified_arn;
#[path = "all/sync_service_integration.rs"]
mod sync_service_integration;
#[path = "all/task_execution_role.rs"]
mod task_execution_role;
#[path = "all/task_integrations.rs"]
mod task_integrations;
#[path = "all/task_lifecycle_under_execution_role.rs"]
mod task_lifecycle_under_execution_role;
#[path = "all/task_refusal_access_denied_status.rs"]
mod task_refusal_access_denied_status;
#[path = "all/task_refusal_session_principal.rs"]
mod task_refusal_session_principal;
#[path = "all/task_role_over_dynamodb.rs"]
mod task_role_over_dynamodb;
#[path = "all/task_role_session_principal.rs"]
mod task_role_session_principal;
#[path = "all/task_target_authorization.rs"]
mod task_target_authorization;
#[path = "all/task_timed_out_closed_by_a_bound.rs"]
mod task_timed_out_closed_by_a_bound;
#[path = "all/task_timed_out_quotes_the_activity.rs"]
mod task_timed_out_quotes_the_activity;
#[path = "all/task_timed_out_wording.rs"]
mod task_timed_out_wording;
#[path = "all/task_work_event_families.rs"]
mod task_work_event_families;
#[path = "all/terminal_runtime_errors.rs"]
mod terminal_runtime_errors;
#[path = "all/test_state.rs"]
mod test_state;
#[path = "all/unhandled_handler_error_task_failure.rs"]
mod unhandled_handler_error_task_failure;
#[path = "all/unmade_integration_status_change_error.rs"]
mod unmade_integration_status_change_error;
#[path = "all/unresolved_path_cause.rs"]
mod unresolved_path_cause;
#[path = "all/unserved_request_refusals.rs"]
mod unserved_request_refusals;
#[path = "all/update_state_machine_alias_effects.rs"]
mod update_state_machine_alias_effects;
#[path = "all/vended_log_delivery_clock.rs"]
mod vended_log_delivery_clock;
#[path = "all/vended_log_event_members.rs"]
mod vended_log_event_members;
#[path = "all/zz_scratch.rs"]
mod zz_scratch;
