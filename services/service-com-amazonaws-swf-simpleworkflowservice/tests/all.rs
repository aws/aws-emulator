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
#[path = "all/activity_task_result_payload.rs"]
mod activity_task_result_payload;
#[path = "all/activity_task_terminal_responses.rs"]
mod activity_task_terminal_responses;
#[path = "all/activity_type_lifecycle.rs"]
mod activity_type_lifecycle;
#[path = "all/activity_type_registration.rs"]
mod activity_type_registration;
#[path = "all/cooperative_cancellation.rs"]
mod cooperative_cancellation;
#[path = "all/count_open_workflow_executions.rs"]
mod count_open_workflow_executions;
#[path = "all/count_pending_decision_tasks.rs"]
mod count_pending_decision_tasks;
#[path = "all/cross_kind_task_token_refusals.rs"]
mod cross_kind_task_token_refusals;
#[path = "all/decision_task_poller_cap.rs"]
mod decision_task_poller_cap;
#[path = "all/delete_workflow_type.rs"]
mod delete_workflow_type;
#[path = "all/deprecate_activity_type.rs"]
mod deprecate_activity_type;
#[path = "all/deprecate_workflow_type.rs"]
mod deprecate_workflow_type;
#[path = "all/domain_deprecation.rs"]
mod domain_deprecation;
#[path = "all/domain_tag_resource.rs"]
mod domain_tag_resource;
#[path = "all/domain_tagging.rs"]
mod domain_tagging;
#[path = "all/domain_undeprecation.rs"]
mod domain_undeprecation;
#[path = "all/domain_untagging.rs"]
mod domain_untagging;
#[path = "all/duplicate_start_fault_identity.rs"]
mod duplicate_start_fault_identity;
#[path = "all/execution_record_projection.rs"]
mod execution_record_projection;
#[path = "all/execution_start_to_close_timeout.rs"]
mod execution_start_to_close_timeout;
#[path = "all/exclusive_filter_refusal_message.rs"]
mod exclusive_filter_refusal_message;
#[path = "all/fixed_fault_message_text.rs"]
mod fixed_fault_message_text;
#[path = "all/list_activity_types.rs"]
mod list_activity_types;
#[path = "all/list_closed_workflow_executions.rs"]
mod list_closed_workflow_executions;
#[path = "all/list_open_workflow_executions.rs"]
mod list_open_workflow_executions;
#[path = "all/pending_activity_task_count.rs"]
mod pending_activity_task_count;
#[path = "all/pending_task_count_unknown_domain.rs"]
mod pending_task_count_unknown_domain;
#[path = "all/poll_for_activity_task.rs"]
mod poll_for_activity_task;
#[path = "all/poll_for_decision_task.rs"]
mod poll_for_decision_task;
#[path = "all/poller_cap_limit_message.rs"]
mod poller_cap_limit_message;
#[path = "all/record_activity_task_heartbeat.rs"]
mod record_activity_task_heartbeat;
#[path = "all/request_cancel_workflow_execution.rs"]
mod request_cancel_workflow_execution;
#[path = "all/resource_tag_type_discriminator.rs"]
mod resource_tag_type_discriminator;
#[path = "all/respond_decision_task_completed.rs"]
mod respond_decision_task_completed;
#[path = "all/run_id_shape.rs"]
mod run_id_shape;
#[path = "all/signal_history_pagination.rs"]
mod signal_history_pagination;
#[path = "all/signal_workflow_execution.rs"]
mod signal_workflow_execution;
#[path = "all/stale_decision_task_token_refusals.rs"]
mod stale_decision_task_token_refusals;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_resource_value_fidelity.rs"]
mod tag_resource_value_fidelity;
#[path = "all/task_start_event_members.rs"]
mod task_start_event_members;
#[path = "all/unknown_resource_messages.rs"]
mod unknown_resource_messages;
#[path = "all/workflow_execution_lifecycle.rs"]
mod workflow_execution_lifecycle;
#[path = "all/workflow_type_undeprecation.rs"]
mod workflow_type_undeprecation;
