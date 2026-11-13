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
#[path = "all/api_cache.rs"]
mod api_cache;
#[path = "all/api_inventory_listing_authorization.rs"]
mod api_inventory_listing_authorization;
#[path = "all/api_cache_create_rules.rs"]
mod api_cache_create_rules;
#[path = "all/api_cache_refusal_messages.rs"]
mod api_cache_refusal_messages;
#[path = "all/api_key_deletes_across_page_sizes.rs"]
mod api_key_deletes_across_page_sizes;
#[path = "all/api_key_deletes_drawn_expirations.rs"]
mod api_key_deletes_drawn_expirations;
#[path = "all/api_key_deletes_listing_agreement.rs"]
mod api_key_deletes_listing_agreement;
#[path = "all/api_key_deletes_page_cut_independence.rs"]
mod api_key_deletes_page_cut_independence;
#[path = "all/api_key_deletes_uniform_offset.rs"]
mod api_key_deletes_uniform_offset;
#[path = "all/api_key_deletes_unmoved_by_other_calls.rs"]
mod api_key_deletes_unmoved_by_other_calls;
#[path = "all/api_key_deletion_window.rs"]
mod api_key_deletion_window;
#[path = "all/api_key_update.rs"]
mod api_key_update;
#[path = "all/api_keys_across_api_families.rs"]
mod api_keys_across_api_families;
#[path = "all/association_refusal_messages.rs"]
mod association_refusal_messages;
#[path = "all/channel_namespaces.rs"]
mod channel_namespaces;
#[path = "all/code_definition_refusals.rs"]
mod code_definition_refusals;
#[path = "all/constraint_refusal_error_code.rs"]
mod constraint_refusal_error_code;
#[path = "all/custom_domain_api_association.rs"]
mod custom_domain_api_association;
#[path = "all/data_source_delete.rs"]
mod data_source_delete;
#[path = "all/data_source_introspection.rs"]
mod data_source_introspection;
#[path = "all/data_source_list.rs"]
mod data_source_list;
#[path = "all/data_source_service_role.rs"]
mod data_source_service_role;
#[path = "all/data_source_update.rs"]
mod data_source_update;
#[path = "all/data_sources.rs"]
mod data_sources;
#[path = "all/delete_api_cache.rs"]
mod delete_api_cache;
#[path = "all/disassociation_reference_refusals.rs"]
mod disassociation_reference_refusals;
#[path = "all/domain_certificate_and_api_gates.rs"]
mod domain_certificate_and_api_gates;
#[path = "all/domain_name_list.rs"]
mod domain_name_list;
#[path = "all/domain_name_update_delete.rs"]
mod domain_name_update_delete;
#[path = "all/environment_variables.rs"]
mod environment_variables;
#[path = "all/error_contract.rs"]
mod error_contract;
#[path = "all/evaluate_code.rs"]
mod evaluate_code;
#[path = "all/event_api.rs"]
mod event_api;
#[path = "all/event_api_authorization_gate.rs"]
mod event_api_authorization_gate;
#[path = "all/event_api_create_outcomes.rs"]
mod event_api_create_outcomes;
#[path = "all/event_api_data_plane.rs"]
mod event_api_data_plane;
#[path = "all/event_api_delete.rs"]
mod event_api_delete;
#[path = "all/event_api_gauge_sampling.rs"]
mod event_api_gauge_sampling;
#[path = "all/event_api_name_form.rs"]
mod event_api_name_form;
#[path = "all/event_api_update.rs"]
mod event_api_update;
#[path = "all/free_text_members.rs"]
mod free_text_members;
#[path = "all/function_delete.rs"]
mod function_delete;
#[path = "all/function_list.rs"]
mod function_list;
#[path = "all/function_update.rs"]
mod function_update;
#[path = "all/functions.rs"]
mod functions;
#[path = "all/graphql_api_create_rules.rs"]
mod graphql_api_create_rules;
#[path = "all/graphql_api_lifecycle.rs"]
mod graphql_api_lifecycle;
#[path = "all/graphql_api_realtime_endpoint.rs"]
mod graphql_api_realtime_endpoint;
#[path = "all/graphql_api_update.rs"]
mod graphql_api_update;
#[path = "all/graphql_types.rs"]
mod graphql_types;
#[path = "all/identity_authorization.rs"]
mod identity_authorization;
#[path = "all/introspection_schema.rs"]
mod introspection_schema;
#[path = "all/list_apis.rs"]
mod list_apis;
#[path = "all/merged_api_association_topology.rs"]
mod merged_api_association_topology;
#[path = "all/merged_api_disassociation.rs"]
mod merged_api_disassociation;
#[path = "all/plane_dependency_refusals.rs"]
mod plane_dependency_refusals;
#[path = "all/resolvers.rs"]
mod resolvers;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/schema_creation.rs"]
mod schema_creation;
#[path = "all/schema_creation_status_polling.rs"]
mod schema_creation_status_polling;
#[path = "all/source_api_association.rs"]
mod source_api_association;
#[path = "all/source_api_association_listing.rs"]
mod source_api_association_listing;
#[path = "all/source_api_association_update.rs"]
mod source_api_association_update;
#[path = "all/source_api_disassociation.rs"]
mod source_api_disassociation;
#[path = "all/source_graphql_api_association.rs"]
mod source_graphql_api_association;
#[path = "all/start_schema_merge.rs"]
mod start_schema_merge;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/api_key_version_and_tagging_refusals.rs"]
mod api_key_version_and_tagging_refusals;
#[path = "all/tag_resource.rs"]
mod tag_resource;
#[path = "all/tagging_arn_vendor_refusal.rs"]
mod tagging_arn_vendor_refusal;
#[path = "all/type_delete.rs"]
mod type_delete;
#[path = "all/type_json_rendering.rs"]
mod type_json_rendering;
#[path = "all/type_listing.rs"]
mod type_listing;
#[path = "all/type_sdl_rendering.rs"]
mod type_sdl_rendering;
#[path = "all/type_update.rs"]
mod type_update;
#[path = "all/untag_resource.rs"]
mod untag_resource;
#[path = "all/flush_api_cache.rs"]
mod flush_api_cache;
