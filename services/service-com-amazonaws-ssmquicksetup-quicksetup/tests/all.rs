// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! Put a new suite at `tests/all/<name>.rs` and declare it below, keeping the list
//! sorted.
//!
//! A suite that is not declared here compiles nowhere and its tests never run. A
//! suite left at the top level of `tests/` fails the build.
//!
//! `all/type_parameter_fixtures.rs` is declared the same way but carries no case of its
//! own: it is the one place the suites below take a parameter map completed for the
//! configuration type it deploys from.

// `tests/all.rs` is a test-target CRATE ROOT, so a bare `mod` resolves to a SIBLING
// (`tests/state_roundtrip.rs`) — which the layout check refuses. The `#[path]` is what
// points it into `tests/all/`.
#[path = "all/configuration_definition_id_lookup.rs"]
mod configuration_definition_id_lookup;
#[path = "all/configuration_definitions.rs"]
mod configuration_definitions;
#[path = "all/configuration_id_format.rs"]
mod configuration_id_format;
#[path = "all/configuration_manager_teardown.rs"]
mod configuration_manager_teardown;
#[path = "all/configuration_managers.rs"]
mod configuration_managers;
#[path = "all/configuration_token_region_scope.rs"]
mod configuration_token_region_scope;
#[path = "all/constraint_refusal_wording.rs"]
mod constraint_refusal_wording;
#[path = "all/date_time_timestamps.rs"]
mod date_time_timestamps;
#[path = "all/definition_refusal_wording.rs"]
mod definition_refusal_wording;
#[path = "all/deployment_target_count.rs"]
mod deployment_target_count;
#[path = "all/deployment_target_parameters.rs"]
mod deployment_target_parameters;
#[path = "all/deployment_target_required.rs"]
mod deployment_target_required;
#[path = "all/deployment_targets.rs"]
mod deployment_targets;
#[path = "all/first_class_parameters.rs"]
mod first_class_parameters;
#[path = "all/iam_role_arn_pattern.rs"]
mod iam_role_arn_pattern;
#[path = "all/idempotent_mutations.rs"]
mod idempotent_mutations;
#[path = "all/identity_policy_authorization.rs"]
mod identity_policy_authorization;
#[path = "all/list_tags_for_resource.rs"]
mod list_tags_for_resource;
#[path = "all/local_deployment_roles.rs"]
mod local_deployment_roles;
#[path = "all/manager_not_found_message.rs"]
mod manager_not_found_message;
#[path = "all/manager_region_scope.rs"]
mod manager_region_scope;
#[path = "all/not_found_message_echo_bound.rs"]
mod not_found_message_echo_bound;
#[path = "all/page_token_region_scope.rs"]
mod page_token_region_scope;
#[path = "all/pagination_tokens.rs"]
mod pagination_tokens;
#[path = "all/partial_parameter_updates.rs"]
mod partial_parameter_updates;
#[path = "all/published_type_catalog.rs"]
mod published_type_catalog;
#[path = "all/quick_setup_type_versions.rs"]
mod quick_setup_type_versions;
#[path = "all/region_scoping.rs"]
mod region_scoping;
#[path = "all/required_type_parameters.rs"]
mod required_type_parameters;
#[path = "all/reserved_tag_keys.rs"]
mod reserved_tag_keys;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/sensitive_refusal_messages.rs"]
mod sensitive_refusal_messages;
#[path = "all/service_settings_and_types.rs"]
mod service_settings_and_types;
#[path = "all/service_settings_read_path.rs"]
mod service_settings_read_path;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/status_summaries.rs"]
mod status_summaries;
#[path = "all/store_bounds_after_teardown.rs"]
mod store_bounds_after_teardown;
#[path = "all/target_authorization.rs"]
mod target_authorization;
#[path = "all/target_regions_required.rs"]
mod target_regions_required;
#[path = "all/type_parameter_fixtures.rs"]
mod type_parameter_fixtures;
#[path = "all/type_version_filter.rs"]
mod type_version_filter;
#[path = "all/unroutable_requests.rs"]
mod unroutable_requests;
