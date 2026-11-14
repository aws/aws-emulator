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
#[path = "all/acknowledge_third_party_job.rs"]
mod acknowledge_third_party_job;
#[path = "all/action_artifact_lists.rs"]
mod action_artifact_lists;
#[path = "all/action_execution_history_order.rs"]
mod action_execution_history_order;
#[path = "all/action_executions.rs"]
mod action_executions;
#[path = "all/action_revisions.rs"]
mod action_revisions;
#[path = "all/action_type_catalog.rs"]
mod action_type_catalog;
#[path = "all/action_type_declarations.rs"]
mod action_type_declarations;
#[path = "all/action_type_definitions.rs"]
mod action_type_definitions;
#[path = "all/action_type_provider_entitlement.rs"]
mod action_type_provider_entitlement;
#[path = "all/action_type_wire_form.rs"]
mod action_type_wire_form;
#[path = "all/approval_result_messages.rs"]
mod approval_result_messages;
#[path = "all/approval_reviewer.rs"]
mod approval_reviewer;
#[path = "all/artifact_store_encryption.rs"]
mod artifact_store_encryption;
#[path = "all/built_in_execution_variables.rs"]
mod built_in_execution_variables;
#[path = "all/codecommit_source_actions.rs"]
mod codecommit_source_actions;
#[path = "all/image_and_build_actions.rs"]
mod image_and_build_actions;
#[path = "all/custom_action_job_results.rs"]
mod custom_action_job_results;
#[path = "all/custom_action_jobs.rs"]
mod custom_action_jobs;
#[path = "all/custom_action_types.rs"]
mod custom_action_types;
#[path = "all/declaration_fault_classification.rs"]
mod declaration_fault_classification;
#[path = "all/declaration_action_type_rules.rs"]
mod declaration_action_type_rules;
#[path = "all/declaration_structure_rules.rs"]
mod declaration_structure_rules;
#[path = "all/delete_custom_action_type.rs"]
mod delete_custom_action_type;
// The suite of the stage-ENTRY and supersession rules (its file name is the one it was
// added under).
#[path = "all/diagnose.rs"]
mod diagnose;
#[path = "all/error_message_rules.rs"]
mod error_message_rules;
#[path = "all/execution_engine.rs"]
mod execution_engine;
#[path = "all/execution_stage_locking.rs"]
mod execution_stage_locking;
#[path = "all/job_result_state_statements.rs"]
mod job_result_state_statements;
#[path = "all/job_worker_refusals_and_poll_results.rs"]
mod job_worker_refusals_and_poll_results;
#[path = "all/management_event_recording.rs"]
mod management_event_recording;
#[path = "all/pipeline_declaration_wire_form.rs"]
mod pipeline_declaration_wire_form;
#[path = "all/pipeline_lifecycle.rs"]
mod pipeline_lifecycle;
#[path = "all/pipeline_not_found_wording.rs"]
mod pipeline_not_found_wording;
#[path = "all/pipeline_state.rs"]
mod pipeline_state;
#[path = "all/probe.rs"]
mod probe;
#[path = "all/refusal_statements.rs"]
mod refusal_statements;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/rollback_stage.rs"]
mod rollback_stage;
#[path = "all/rule_executions.rs"]
mod rule_executions;
#[path = "all/run_order_concurrency.rs"]
mod run_order_concurrency;
#[path = "all/s3_deploy_object_key.rs"]
mod s3_deploy_object_key;
#[path = "all/source_revision_failures.rs"]
mod source_revision_failures;
#[path = "all/source_revision_overrides.rs"]
mod source_revision_overrides;
#[path = "all/stage_condition_overrides.rs"]
mod stage_condition_overrides;
#[path = "all/stage_condition_rules.rs"]
mod stage_condition_rules;
#[path = "all/stage_retry_refusal_wording.rs"]
mod stage_retry_refusal_wording;
#[path = "all/stage_transition_authorization.rs"]
mod stage_transition_authorization;
#[path = "all/stage_transitions.rs"]
mod stage_transitions;
#[path = "all/tag_removal_and_refusal_wording.rs"]
mod tag_removal_and_refusal_wording;
#[path = "all/state_change_events.rs"]
mod state_change_events;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/stop_pipeline_execution.rs"]
mod stop_pipeline_execution;
#[path = "all/terminal_state_refusals.rs"]
mod terminal_state_refusals;
#[path = "all/third_party_job_details.rs"]
mod third_party_job_details;
#[path = "all/third_party_job_failure_results.rs"]
mod third_party_job_failure_results;
#[path = "all/third_party_job_results.rs"]
mod third_party_job_results;
#[path = "all/third_party_jobs.rs"]
mod third_party_jobs;
#[path = "all/third_party_webhooks.rs"]
mod third_party_webhooks;
#[path = "all/unattended_execution.rs"]
mod unattended_execution;
#[path = "all/update_pipeline.rs"]
mod update_pipeline;
#[path = "all/validation_and_messages.rs"]
mod validation_and_messages;
#[path = "all/webhook_definition_rules.rs"]
mod webhook_definition_rules;
#[path = "all/webhook_filter_pattern_wording.rs"]
mod webhook_filter_pattern_wording;
#[path = "all/webhook_trigger.rs"]
mod webhook_trigger;
#[path = "all/webhooks.rs"]
mod webhooks;
