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
#[path = "all/account_settings_document.rs"]
mod account_settings_document;
#[path = "all/appconfig_workflow.rs"]
mod appconfig_workflow;
#[path = "all/application_deletion_gate.rs"]
mod application_deletion_gate;
#[path = "all/application_experiment_definition_gate.rs"]
mod application_experiment_definition_gate;
#[path = "all/application_name_uniqueness.rs"]
mod application_name_uniqueness;
#[path = "all/aws_authored_extensions.rs"]
mod aws_authored_extensions;
#[path = "all/configuration_profile_deletion_protection.rs"]
mod configuration_profile_deletion_protection;
#[path = "all/configuration_profile_immutable_type.rs"]
mod configuration_profile_immutable_type;
#[path = "all/configuration_profile_lookup_subject.rs"]
mod configuration_profile_lookup_subject;
#[path = "all/configuration_profile_type.rs"]
mod configuration_profile_type;
#[path = "all/configuration_profile_validators.rs"]
mod configuration_profile_validators;
#[path = "all/control_plane_fidelity.rs"]
mod control_plane_fidelity;
#[path = "all/create_deployment_strategy.rs"]
mod create_deployment_strategy;
#[path = "all/delete_application.rs"]
mod delete_application;
#[path = "all/delete_configuration_profile.rs"]
mod delete_configuration_profile;
#[path = "all/delete_experiment_definition.rs"]
mod delete_experiment_definition;
#[path = "all/delete_extension_association.rs"]
mod delete_extension_association;
#[path = "all/delete_hosted_configuration_version.rs"]
mod delete_hosted_configuration_version;
#[path = "all/deployment_event_notifications_to_sqs.rs"]
mod deployment_event_notifications_to_sqs;
#[path = "all/deployment_not_found_message.rs"]
mod deployment_not_found_message;
#[path = "all/deployment_phases_follow_the_strategy.rs"]
mod deployment_phases_follow_the_strategy;
#[path = "all/deployment_settles_on_its_own.rs"]
mod deployment_settles_on_its_own;
#[path = "all/deployment_strategy_patch_paces_deployments.rs"]
mod deployment_strategy_patch_paces_deployments;
#[path = "all/deployment_tick_directive.rs"]
mod deployment_tick_directive;
#[path = "all/deployment_wire_fidelity.rs"]
mod deployment_wire_fidelity;
#[path = "all/deprecated_get_configuration_refusals.rs"]
mod deprecated_get_configuration_refusals;
#[path = "all/environment_state_wire_form.rs"]
mod environment_state_wire_form;
#[path = "all/environment_update.rs"]
mod environment_update;
#[path = "all/experiment_definition_profile_type.rs"]
mod experiment_definition_profile_type;
#[path = "all/experiment_definition_wire_fidelity.rs"]
mod experiment_definition_wire_fidelity;
#[path = "all/experiment_definitions.rs"]
mod experiment_definitions;
#[path = "all/experiment_run_refusal_messages.rs"]
mod experiment_run_refusal_messages;
#[path = "all/experiment_run_timeline.rs"]
mod experiment_run_timeline;
#[path = "all/experiment_run_treatment_assignment.rs"]
mod experiment_run_treatment_assignment;
#[path = "all/experiment_run_wire_fidelity.rs"]
mod experiment_run_wire_fidelity;
#[path = "all/extension_action_points.rs"]
mod extension_action_points;
#[path = "all/extension_association_refusal_messages.rs"]
mod extension_association_refusal_messages;
#[path = "all/extension_associations.rs"]
mod extension_associations;
#[path = "all/extensions.rs"]
mod extensions;
#[path = "all/feature_flag_attribute_constraints.rs"]
mod feature_flag_attribute_constraints;
#[path = "all/feature_flag_metadata.rs"]
mod feature_flag_metadata;
#[path = "all/feature_flags_extension_association.rs"]
mod feature_flags_extension_association;
#[path = "all/get_configuration.rs"]
mod get_configuration;
#[path = "all/hosted_configuration_version_content_reuse.rs"]
mod hosted_configuration_version_content_reuse;
#[path = "all/json_schema_validator_gate.rs"]
mod json_schema_validator_gate;
#[path = "all/list_applications.rs"]
mod list_applications;
#[path = "all/list_applications_order.rs"]
mod list_applications_order;
#[path = "all/list_configuration_profiles.rs"]
mod list_configuration_profiles;
#[path = "all/list_deployment_strategies.rs"]
mod list_deployment_strategies;
#[path = "all/list_deployments.rs"]
mod list_deployments;
#[path = "all/list_deployments_configuration_sources.rs"]
mod list_deployments_configuration_sources;
#[path = "all/list_environments.rs"]
mod list_environments;
#[path = "all/list_experiment_definitions.rs"]
mod list_experiment_definitions;
#[path = "all/list_experiment_run_events.rs"]
mod list_experiment_run_events;
#[path = "all/list_experiment_runs.rs"]
mod list_experiment_runs;
#[path = "all/list_extension_associations.rs"]
mod list_extension_associations;
#[path = "all/list_extensions.rs"]
mod list_extensions;
#[path = "all/list_hosted_configuration_versions.rs"]
mod list_hosted_configuration_versions;
#[path = "all/listing_item_instant_wire_form.rs"]
mod listing_item_instant_wire_form;
#[path = "all/omitted_required_bindings.rs"]
mod omitted_required_bindings;
#[path = "all/pattern_refusal_quoting.rs"]
mod pattern_refusal_quoting;
#[path = "all/refusal_class_and_rendering_rules.rs"]
mod refusal_class_and_rendering_rules;
#[path = "all/refusal_message_templates.rs"]
mod refusal_message_templates;
#[path = "all/request_derived_refusal_templates.rs"]
mod request_derived_refusal_templates;
#[path = "all/resource_deletes.rs"]
mod resource_deletes;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/rolled_back_deployment_completion_time.rs"]
mod rolled_back_deployment_completion_time;
#[path = "all/rollout_advances_over_the_wire.rs"]
mod rollout_advances_over_the_wire;
#[path = "all/rollout_percentage_gates_serving.rs"]
mod rollout_percentage_gates_serving;
#[path = "all/start_deployment_configuration_version.rs"]
mod start_deployment_configuration_version;
#[path = "all/start_deployment_latest_deployment_number.rs"]
mod start_deployment_latest_deployment_number;
#[path = "all/start_deployment_resource_identifiers.rs"]
mod start_deployment_resource_identifiers;
#[path = "all/start_experiment_run.rs"]
mod start_experiment_run;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/stop_deployment.rs"]
mod stop_deployment;
#[path = "all/stop_deployment_without_its_environment.rs"]
mod stop_deployment_without_its_environment;
#[path = "all/stop_experiment_run.rs"]
mod stop_experiment_run;
#[path = "all/stop_operations_settle_terminal_outcomes.rs"]
mod stop_operations_settle_terminal_outcomes;
#[path = "all/update_application.rs"]
mod update_application;
#[path = "all/update_configuration_profile.rs"]
mod update_configuration_profile;
#[path = "all/update_deployment_strategy.rs"]
mod update_deployment_strategy;
#[path = "all/update_experiment_definition.rs"]
mod update_experiment_definition;
#[path = "all/update_experiment_run.rs"]
mod update_experiment_run;
#[path = "all/update_extension.rs"]
mod update_extension;
#[path = "all/update_extension_association.rs"]
mod update_extension_association;
#[path = "all/update_extension_visibility.rs"]
mod update_extension_visibility;
#[path = "all/validate_configuration_resource_resolution.rs"]
mod validate_configuration_resource_resolution;
#[path = "all/validate_configuration_verdict.rs"]
mod validate_configuration_verdict;
