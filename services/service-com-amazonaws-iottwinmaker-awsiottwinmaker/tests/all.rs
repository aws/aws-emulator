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
#[path = "all/abstract_component_type_listing.rs"]
mod abstract_component_type_listing;

#[path = "all/abstract_component_types.rs"]
mod abstract_component_types;

#[path = "all/batch_put_error_entries.rs"]
mod batch_put_error_entries;

#[path = "all/component_definition_refinement.rs"]
mod component_definition_refinement;

#[path = "all/component_inherited_schema.rs"]
mod component_inherited_schema;

#[path = "all/component_listing.rs"]
mod component_listing;

#[path = "all/component_path_resolution.rs"]
mod component_path_resolution;

#[path = "all/component_refined_definition.rs"]
mod component_refined_definition;

#[path = "all/component_tree_walk.rs"]
mod component_tree_walk;

#[path = "all/component_type_create_references.rs"]
mod component_type_create_references;

#[path = "all/component_type_delete.rs"]
mod component_type_delete;

#[path = "all/component_type_delete_composite_references.rs"]
mod component_type_delete_composite_references;

#[path = "all/component_type_delete_in_use_message_bound.rs"]
mod component_type_delete_in_use_message_bound;

#[path = "all/component_type_inherited_schema.rs"]
mod component_type_inherited_schema;

#[path = "all/component_type_instance_scope.rs"]
mod component_type_instance_scope;

#[path = "all/component_type_lifecycle.rs"]
mod component_type_lifecycle;

#[path = "all/component_type_listing.rs"]
mod component_type_listing;

#[path = "all/component_type_repoint.rs"]
mod component_type_repoint;

#[path = "all/component_type_untyped_property_definition.rs"]
mod component_type_untyped_property_definition;

#[path = "all/component_type_update.rs"]
mod component_type_update;

#[path = "all/component_type_update_references.rs"]
mod component_type_update_references;

#[path = "all/composite_component_inherited_declaration.rs"]
mod composite_component_inherited_declaration;

#[path = "all/composite_component_stranded_declaration.rs"]
mod composite_component_stranded_declaration;

#[path = "all/create_entity_untyped_definition_order.rs"]
mod create_entity_untyped_definition_order;

#[path = "all/cross_region_page_token.rs"]
mod cross_region_page_token;

#[path = "all/entity_component_type_filter.rs"]
mod entity_component_type_filter;

#[path = "all/entity_composite_components.rs"]
mod entity_composite_components;

#[path = "all/entity_external_id_filter.rs"]
mod entity_external_id_filter;

#[path = "all/entity_property_value_data_type.rs"]
mod entity_property_value_data_type;

#[path = "all/entity_update.rs"]
mod entity_update;

#[path = "all/error_message_bounds.rs"]
mod error_message_bounds;

#[path = "all/execute_query.rs"]
mod execute_query;

#[path = "all/execute_query_composite_components.rs"]
mod execute_query_composite_components;

#[path = "all/execute_query_hierarchy.rs"]
mod execute_query_hierarchy;

#[path = "all/execute_query_inherited_properties.rs"]
mod execute_query_inherited_properties;

#[path = "all/filtered_page_token_scope.rs"]
mod filtered_page_token_scope;

#[path = "all/harness.rs"]
mod harness;

#[path = "all/history_page_token_scope.rs"]
mod history_page_token_scope;

#[path = "all/history_refusal_order.rs"]
mod history_refusal_order;

#[path = "all/max_results_zero_page.rs"]
mod max_results_zero_page;

#[path = "all/metadata_transfer_job.rs"]
mod metadata_transfer_job;

#[path = "all/metadata_transfer_job_cancel.rs"]
mod metadata_transfer_job_cancel;

#[path = "all/metadata_transfer_job_listing.rs"]
mod metadata_transfer_job_listing;

#[path = "all/metadata_transfer_job_read.rs"]
mod metadata_transfer_job_read;

#[path = "all/metadata_transfer_run.rs"]
mod metadata_transfer_run;

#[path = "all/minted_page_position.rs"]
mod minted_page_position;

#[path = "all/next_token_refusal.rs"]
mod next_token_refusal;

#[path = "all/not_found_constructions.rs"]
mod not_found_constructions;

#[path = "all/page_size_rule.rs"]
mod page_size_rule;

#[path = "all/page_token_spelling.rs"]
mod page_token_spelling;

#[path = "all/pricing_plan.rs"]
mod pricing_plan;

#[path = "all/pricing_plan_tier_and_pending.rs"]
mod pricing_plan_tier_and_pending;

#[path = "all/pricing_plan_update.rs"]
mod pricing_plan_update;

#[path = "all/property_definition_default_value_type.rs"]
mod property_definition_default_value_type;

#[path = "all/property_group_refinement.rs"]
mod property_group_refinement;

#[path = "all/property_listing.rs"]
mod property_listing;

#[path = "all/property_projection_sharing.rs"]
mod property_projection_sharing;

#[path = "all/property_value_data_type.rs"]
mod property_value_data_type;

#[path = "all/property_value_read.rs"]
mod property_value_read;

#[path = "all/property_value_refusal_order.rs"]
mod property_value_refusal_order;

#[path = "all/property_value_write_order.rs"]
mod property_value_write_order;

#[path = "all/refusal_classes_and_wording.rs"]
mod refusal_classes_and_wording;

#[path = "all/refusal_constructions.rs"]
mod refusal_constructions;

#[path = "all/region_ceiling_refusal_class.rs"]
mod region_ceiling_refusal_class;

#[path = "all/region_scoping.rs"]
mod region_scoping;

#[path = "all/reported_members.rs"]
mod reported_members;

#[path = "all/requested_map_refusal_order.rs"]
mod requested_map_refusal_order;

#[path = "all/reserved_tag_keys.rs"]
mod reserved_tag_keys;

#[path = "all/resource_tags.rs"]
mod resource_tags;

#[path = "all/scene_delete.rs"]
mod scene_delete;

#[path = "all/scene_lifecycle.rs"]
mod scene_lifecycle;

#[path = "all/scene_listing.rs"]
mod scene_listing;

#[path = "all/scene_read.rs"]
mod scene_read;

#[path = "all/scene_update.rs"]
mod scene_update;

#[path = "all/scope_resolution_order.rs"]
mod scope_resolution_order;

#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;

#[path = "all/sync_job.rs"]
mod sync_job;

#[path = "all/sync_job_delete.rs"]
mod sync_job_delete;

#[path = "all/sync_job_listing.rs"]
mod sync_job_listing;

#[path = "all/sync_job_role_trust.rs"]
mod sync_job_role_trust;

#[path = "all/sync_resource_listing.rs"]
mod sync_resource_listing;

#[path = "all/tabular_page_token_scope.rs"]
mod tabular_page_token_scope;

#[path = "all/tabular_property_sources.rs"]
mod tabular_property_sources;

#[path = "all/time_series_latest_value.rs"]
mod time_series_latest_value;

#[path = "all/time_series_property_refusal.rs"]
mod time_series_property_refusal;

#[path = "all/time_series_property_values.rs"]
mod time_series_property_values;

#[path = "all/unassumable_role_message.rs"]
mod unassumable_role_message;

#[path = "all/unbounded_refusal_message_bounds.rs"]
mod unbounded_refusal_message_bounds;

#[path = "all/workspace_built_in_component_types.rs"]
mod workspace_built_in_component_types;

#[path = "all/workspace_entity_lifecycle.rs"]
mod workspace_entity_lifecycle;

#[path = "all/workspace_listing.rs"]
mod workspace_listing;

#[path = "all/workspace_role_trust.rs"]
mod workspace_role_trust;

#[path = "all/workspace_update.rs"]
mod workspace_update;

#[path = "all/workspace_update_scope.rs"]
mod workspace_update_scope;
