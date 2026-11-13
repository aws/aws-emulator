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
#[path = "all/account_settings.rs"]
mod account_settings;
#[path = "all/application_group_identity.rs"]
mod application_group_identity;
#[path = "all/application_group_target.rs"]
mod application_group_target;
#[path = "all/boundary_refusal_message.rs"]
mod boundary_refusal_message;
#[path = "all/cancel_tag_sync_task.rs"]
mod cancel_tag_sync_task;
#[path = "all/delete_group_protection.rs"]
mod delete_group_protection;
#[path = "all/delete_group_tag_sync_tasks.rs"]
mod delete_group_tag_sync_tasks;
#[path = "all/filters_bound_rule.rs"]
mod filters_bound_rule;
#[path = "all/filters_bound_single_source.rs"]
mod filters_bound_single_source;
#[path = "all/get_tag_sync_task.rs"]
mod get_tag_sync_task;
#[path = "all/get_tags_identity_forms.rs"]
mod get_tags_identity_forms;
#[path = "all/group_configuration_projection.rs"]
mod group_configuration_projection;
#[path = "all/group_owner_configuration_and_update.rs"]
mod group_owner_configuration_and_update;
#[path = "all/group_query.rs"]
mod group_query;
#[path = "all/group_reported_additions.rs"]
mod group_reported_additions;
#[path = "all/group_resources_supported_configuration.rs"]
mod group_resources_supported_configuration;
#[path = "all/group_resources_supported_resource_types.rs"]
mod group_resources_supported_resource_types;
#[path = "all/group_tags.rs"]
mod group_tags;
#[path = "all/group_untag.rs"]
mod group_untag;
#[path = "all/grouping_statuses.rs"]
mod grouping_statuses;
#[path = "all/list_group_resources_filter_validation.rs"]
mod list_group_resources_filter_validation;
#[path = "all/list_group_resources_query_membership.rs"]
mod list_group_resources_query_membership;
#[path = "all/list_groups_filters.rs"]
mod list_groups_filters;
#[path = "all/list_tag_sync_tasks.rs"]
mod list_tag_sync_tasks;
#[path = "all/page_size_rule.rs"]
mod page_size_rule;
#[path = "all/put_group_configuration.rs"]
mod put_group_configuration;
#[path = "all/region_isolation.rs"]
mod region_isolation;
#[path = "all/region_scoped_pagination_tokens.rs"]
mod region_scoped_pagination_tokens;
#[path = "all/rejection_contract.rs"]
mod rejection_contract;
#[path = "all/reported_list_wire_form.rs"]
mod reported_list_wire_form;
#[path = "all/reserved_tag_keys.rs"]
mod reserved_tag_keys;
#[path = "all/resource_arns_rule.rs"]
mod resource_arns_rule;
#[path = "all/resource_group_lifecycle.rs"]
mod resource_group_lifecycle;
#[path = "all/search_resources.rs"]
mod search_resources;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_character_class.rs"]
mod tag_character_class;
#[path = "all/tag_sync_tasks.rs"]
mod tag_sync_tasks;
#[path = "all/ungroup_after_configuration_change.rs"]
mod ungroup_after_configuration_change;
#[path = "all/ungroup_reported_removals.rs"]
mod ungroup_reported_removals;
#[path = "all/ungroup_resources.rs"]
mod ungroup_resources;
#[path = "all/unsupported_target_message.rs"]
mod unsupported_target_message;
#[path = "all/update_group_query.rs"]
mod update_group_query;
