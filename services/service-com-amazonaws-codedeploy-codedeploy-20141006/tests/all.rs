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
#[path = "all/alarm_stopped_deployments.rs"]
mod alarm_stopped_deployments;
#[path = "all/application_revisions.rs"]
mod application_revisions;
#[path = "all/aws_refusal_sentences.rs"]
mod aws_refusal_sentences;
#[path = "all/batch_get_application_revisions.rs"]
mod batch_get_application_revisions;
#[path = "all/batch_get_applications.rs"]
mod batch_get_applications;
#[path = "all/batch_get_deployment_instances.rs"]
mod batch_get_deployment_instances;
#[path = "all/batch_get_deployments.rs"]
mod batch_get_deployments;
#[path = "all/client_error_messages.rs"]
mod client_error_messages;
#[path = "all/codedeploy_deployment_outcome.rs"]
mod codedeploy_deployment_outcome;
#[path = "all/codedeploy_lifecycle.rs"]
mod codedeploy_lifecycle;
#[path = "all/delete_deployment_config.rs"]
mod delete_deployment_config;
#[path = "all/deployment_configs.rs"]
mod deployment_configs;
#[path = "all/delete_deployment_group.rs"]
mod delete_deployment_group;
#[path = "all/deployment_group_alarm_counts.rs"]
mod deployment_group_alarm_counts;
#[path = "all/deployment_group_defaults.rs"]
mod deployment_group_defaults;
#[path = "all/deployment_group_update.rs"]
mod deployment_group_update;
#[path = "all/deployment_in_flight_window.rs"]
mod deployment_in_flight_window;
#[path = "all/deployment_info_members.rs"]
mod deployment_info_members;
#[path = "all/deployment_progress_refusal_sentences.rs"]
mod deployment_progress_refusal_sentences;
#[path = "all/deployment_target_record_readability.rs"]
mod deployment_target_record_readability;
#[path = "all/deployment_targets.rs"]
mod deployment_targets;
#[path = "all/deployment_trigger_notifications.rs"]
mod deployment_trigger_notifications;
#[path = "all/fixed_error_sentences.rs"]
mod fixed_error_sentences;
#[path = "all/get_deployment_instance.rs"]
mod get_deployment_instance;
#[path = "all/get_deployment_target.rs"]
mod get_deployment_target;
#[path = "all/git_hub_account_token_names.rs"]
mod git_hub_account_token_names;
#[path = "all/github_account_tokens.rs"]
mod github_account_tokens;
#[path = "all/lambda_traffic_shifting.rs"]
mod lambda_traffic_shifting;
#[path = "all/lambda_traffic_shift.rs"]
mod lambda_traffic_shift;
#[path = "all/lifecycle_event_hook_status.rs"]
mod lifecycle_event_hook_status;
#[path = "all/lifecycle_hook_validation.rs"]
mod lifecycle_hook_validation;
#[path = "all/list_deployment_configs.rs"]
mod list_deployment_configs;
#[path = "all/list_deployment_groups.rs"]
mod list_deployment_groups;
#[path = "all/list_deployment_instances.rs"]
mod list_deployment_instances;
#[path = "all/list_deployment_targets.rs"]
mod list_deployment_targets;
#[path = "all/list_deployments_filter_pair.rs"]
mod list_deployments_filter_pair;
#[path = "all/list_on_premises_instances.rs"]
mod list_on_premises_instances;
#[path = "all/on_premises_instances.rs"]
mod on_premises_instances;
#[path = "all/on_premises_revisions_and_blue_green.rs"]
mod on_premises_revisions_and_blue_green;
#[path = "all/on_premises_tag_removal.rs"]
mod on_premises_tag_removal;
#[path = "all/on_premises_tagging.rs"]
mod on_premises_tagging;
#[path = "all/predefined_traffic_shift_configs.rs"]
mod predefined_traffic_shift_configs;
#[path = "all/quotas_waits_and_host_names.rs"]
mod quotas_waits_and_host_names;
#[path = "all/read_response_collections.rs"]
mod read_response_collections;
#[path = "all/refusal_request_derived_identifiers.rs"]
mod refusal_request_derived_identifiers;
#[path = "all/refusal_sentence_templates.rs"]
mod refusal_sentence_templates;
#[path = "all/resource_tag_removal.rs"]
mod resource_tag_removal;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/revision_content_digest.rs"]
mod revision_content_digest;
#[path = "all/s3_revision_resolution.rs"]
mod s3_revision_resolution;
#[path = "all/service_role_permissions.rs"]
mod service_role_permissions;
#[path = "all/service_role_validation.rs"]
mod service_role_validation;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/stop_deployment.rs"]
mod stop_deployment;
#[path = "all/success_response_members.rs"]
mod success_response_members;
#[path = "all/tagging_arn_resource_types.rs"]
mod tagging_arn_resource_types;
#[path = "all/update_application.rs"]
mod update_application;
#[path = "all/zz_diag.rs"]
mod zz_diag;
