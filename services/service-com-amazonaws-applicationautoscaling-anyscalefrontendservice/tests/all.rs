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
#[path = "all/aws_worded_client_errors.rs"]
mod aws_worded_client_errors;
#[path = "all/capacity_window_enforcement.rs"]
mod capacity_window_enforcement;
#[path = "all/concurrent_policy_alarm_consistency.rs"]
mod concurrent_policy_alarm_consistency;
#[path = "all/control_plane_rules.rs"]
mod control_plane_rules;
#[path = "all/customized_metric_alarms.rs"]
mod customized_metric_alarms;
#[path = "all/custom_resource_endpoint_targets.rs"]
mod custom_resource_endpoint_targets;
#[path = "all/emulator.rs"]
mod emulator;
#[path = "all/keyspaces_table_resolution.rs"]
mod keyspaces_table_resolution;
#[path = "all/lambda_provisioned_concurrency_window.rs"]
mod lambda_provisioned_concurrency_window;
#[path = "all/lambda_qualifier_resolution.rs"]
mod lambda_qualifier_resolution;
#[path = "all/metric_driven_ecs_scaling.rs"]
mod metric_driven_ecs_scaling;
#[path = "all/movement_fulfilment.rs"]
mod movement_fulfilment;
#[path = "all/not_scaled_activities.rs"]
mod not_scaled_activities;
#[path = "all/policy_alarm_association.rs"]
mod policy_alarm_association;
#[path = "all/policy_refusal_sentences.rs"]
mod policy_refusal_sentences;
#[path = "all/predictive_scaling_forecast.rs"]
mod predictive_scaling_forecast;
#[path = "all/published_breach_execution.rs"]
mod published_breach_execution;
#[path = "all/recorded_refusal_wording.rs"]
mod recorded_refusal_wording;
#[path = "all/refusal_messages.rs"]
mod refusal_messages;
#[path = "all/refusal_texts.rs"]
mod refusal_texts;
#[path = "all/registration_capacity_floor.rs"]
mod registration_capacity_floor;
#[path = "all/registration_preconditions.rs"]
mod registration_preconditions;
#[path = "all/request_authorization.rs"]
mod request_authorization;
#[path = "all/regional_scalable_targets.rs"]
mod regional_scalable_targets;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/scalable_target_lifecycle.rs"]
mod scalable_target_lifecycle;
#[path = "all/scalable_target_resolution.rs"]
mod scalable_target_resolution;
#[path = "all/scaling_activities.rs"]
mod scaling_activities;
#[path = "all/scaling_activity_status_message.rs"]
mod scaling_activity_status_message;
#[path = "all/scaling_policy_preconditions.rs"]
mod scaling_policy_preconditions;
#[path = "all/scheduled_action_activity_pair.rs"]
mod scheduled_action_activity_pair;
#[path = "all/scheduled_action_deletion.rs"]
mod scheduled_action_deletion;
#[path = "all/scheduled_action_dispatch.rs"]
mod scheduled_action_dispatch;
#[path = "all/scheduled_action_lifecycle.rs"]
mod scheduled_action_lifecycle;
#[path = "all/scheduled_actions.rs"]
mod scheduled_actions;
#[path = "all/scheduled_scaling_suspension.rs"]
mod scheduled_scaling_suspension;
#[path = "all/service_linked_role.rs"]
mod service_linked_role;
#[path = "all/simultaneous_policy_arbitration.rs"]
mod simultaneous_policy_arbitration;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/step_scaling_execution.rs"]
mod step_scaling_execution;
#[path = "all/step_scaling_window_reread.rs"]
mod step_scaling_window_reread;
#[path = "all/superseded_scaling_activities.rs"]
mod superseded_scaling_activities;
#[path = "all/tag_family_absent_target.rs"]
mod tag_family_absent_target;
#[path = "all/tag_mutation_refusal.rs"]
mod tag_mutation_refusal;
#[path = "all/tags.rs"]
mod tags;
#[path = "all/target_tracking_alarms.rs"]
mod target_tracking_alarms;
#[path = "all/target_tracking_execution.rs"]
mod target_tracking_execution;
#[path = "all/unregistered_target_refusals.rs"]
mod unregistered_target_refusals;
