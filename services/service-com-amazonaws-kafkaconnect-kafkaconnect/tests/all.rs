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
#[path = "all/absent_required_query_parameter.rs"]
mod absent_required_query_parameter;
#[path = "all/arn_label_refusals.rs"]
mod arn_label_refusals;
#[path = "all/capacity_rule_is_one_rule.rs"]
mod capacity_rule_is_one_rule;
#[path = "all/connector_capacity_and_worker_configuration.rs"]
mod connector_capacity_and_worker_configuration;
#[path = "all/connector_dependency_deletes.rs"]
mod connector_dependency_deletes;
#[path = "all/connector_not_running_is_one_rule.rs"]
mod connector_not_running_is_one_rule;
#[path = "all/connector_operation_arn_rule.rs"]
mod connector_operation_arn_rule;
#[path = "all/connector_operation_arn_spans_connector_names.rs"]
mod connector_operation_arn_spans_connector_names;
#[path = "all/connector_operation_minting_is_one_rule.rs"]
mod connector_operation_minting_is_one_rule;
#[path = "all/connector_operation_window.rs"]
mod connector_operation_window;
#[path = "all/connector_operations.rs"]
mod connector_operations;
#[path = "all/connector_update.rs"]
mod connector_update;
#[path = "all/custom_plugin_and_connector_lifecycle.rs"]
mod custom_plugin_and_connector_lifecycle;
#[path = "all/custom_plugin_bucket_arn_names_a_bucket.rs"]
mod custom_plugin_bucket_arn_names_a_bucket;
#[path = "all/custom_plugin_file_description.rs"]
mod custom_plugin_file_description;
#[path = "all/custom_plugin_file_location.rs"]
mod custom_plugin_file_location;
#[path = "all/describe_connector_operation.rs"]
mod describe_connector_operation;
#[path = "all/duplicate_name_conflict_message.rs"]
mod duplicate_name_conflict_message;
#[path = "all/iso8601_timestamps.rs"]
mod iso8601_timestamps;
#[path = "all/list_worker_configurations.rs"]
mod list_worker_configurations;
#[path = "all/page_token_belongs_to_one_listing.rs"]
mod page_token_belongs_to_one_listing;
#[path = "all/page_token_carries_the_filter_that_narrowed_it.rs"]
mod page_token_carries_the_filter_that_narrowed_it;
#[path = "all/region_scoping.rs"]
mod region_scoping;
#[path = "all/removal_start_is_one_rule.rs"]
mod removal_start_is_one_rule;
#[path = "all/required_query_parameter_routes.rs"]
mod required_query_parameter_routes;
#[path = "all/restart_connector_scope.rs"]
mod restart_connector_scope;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_length_bounds.rs"]
mod tag_length_bounds;
#[path = "all/tag_rules.rs"]
mod tag_rules;
#[path = "all/update_connector_version_gate.rs"]
mod update_connector_version_gate;
#[path = "all/validation_messages.rs"]
mod validation_messages;
#[path = "all/worker_configuration_create_and_describe.rs"]
mod worker_configuration_create_and_describe;
#[path = "all/worker_configuration_delete_and_tags.rs"]
mod worker_configuration_delete_and_tags;
#[path = "all/worker_configuration_properties_encoding.rs"]
mod worker_configuration_properties_encoding;
