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
#[path = "all/absent_identifier_namespaces.rs"]
mod absent_identifier_namespaces;
#[path = "all/admission_preconditions.rs"]
mod admission_preconditions;
#[path = "all/alias_viewer_certificate.rs"]
mod alias_viewer_certificate;
#[path = "all/announced_location_uri.rs"]
mod announced_location_uri;
#[path = "all/anycast_ip_list_lifecycle.rs"]
mod anycast_ip_list_lifecycle;
#[path = "all/anycast_ip_list_list.rs"]
mod anycast_ip_list_list;
#[path = "all/anycast_ip_list_update.rs"]
mod anycast_ip_list_update;
#[path = "all/by_policy_listing_refusal.rs"]
mod by_policy_listing_refusal;
#[path = "all/cache_policy_lifecycle.rs"]
mod cache_policy_lifecycle;
#[path = "all/cache_policy_list.rs"]
mod cache_policy_list;
#[path = "all/cache_policy_update.rs"]
mod cache_policy_update;
#[path = "all/caller_reference_conflict_wording.rs"]
mod caller_reference_conflict_wording;
#[path = "all/caller_reference_immutable.rs"]
mod caller_reference_immutable;
#[path = "all/caller_reference_replay_window.rs"]
mod caller_reference_replay_window;
#[path = "all/connection_function_code.rs"]
mod connection_function_code;
#[path = "all/connection_function_delete.rs"]
mod connection_function_delete;
#[path = "all/connection_function_lifecycle.rs"]
mod connection_function_lifecycle;
#[path = "all/connection_function_list.rs"]
mod connection_function_list;
#[path = "all/connection_function_not_found_message.rs"]
mod connection_function_not_found_message;
#[path = "all/connection_function_publish.rs"]
mod connection_function_publish;
#[path = "all/connection_group_by_routing_endpoint.rs"]
mod connection_group_by_routing_endpoint;
#[path = "all/connection_group_delete.rs"]
mod connection_group_delete;
#[path = "all/connection_group_tags.rs"]
mod connection_group_tags;
#[path = "all/connection_group_update.rs"]
mod connection_group_update;
#[path = "all/connection_groups.rs"]
mod connection_groups;
#[path = "all/connection_mode_immutable.rs"]
mod connection_mode_immutable;
#[path = "all/continuous_deployment_policies.rs"]
mod continuous_deployment_policies;
#[path = "all/continuous_deployment_policy_config_read.rs"]
mod continuous_deployment_policy_config_read;
#[path = "all/continuous_deployment_policy_delete.rs"]
mod continuous_deployment_policy_delete;
#[path = "all/continuous_deployment_policy_update.rs"]
mod continuous_deployment_policy_update;
#[path = "all/create_location_header.rs"]
mod create_location_header;
#[path = "all/delete_conflict_refusals.rs"]
mod delete_conflict_refusals;
#[path = "all/distribution_config_defaults.rs"]
mod distribution_config_defaults;
#[path = "all/distribution_config_read.rs"]
mod distribution_config_read;
#[path = "all/distribution_copy.rs"]
mod distribution_copy;
#[path = "all/distribution_delete_gate.rs"]
mod distribution_delete_gate;
#[path = "all/distribution_lifecycle.rs"]
mod distribution_lifecycle;
#[path = "all/distribution_tags.rs"]
mod distribution_tags;
#[path = "all/distribution_tenant_delete.rs"]
mod distribution_tenant_delete;
#[path = "all/distribution_tenant_delete_scope.rs"]
mod distribution_tenant_delete_scope;
#[path = "all/distribution_tenant_filter_matches_nothing.rs"]
mod distribution_tenant_filter_matches_nothing;
#[path = "all/distribution_tenant_lifecycle.rs"]
mod distribution_tenant_lifecycle;
#[path = "all/distribution_tenant_update.rs"]
mod distribution_tenant_update;
#[path = "all/distribution_tenants_by_customization.rs"]
mod distribution_tenants_by_customization;
#[path = "all/distribution_tenants_list.rs"]
mod distribution_tenants_list;
#[path = "all/distribution_tenants_unfiltered_listing.rs"]
mod distribution_tenants_unfiltered_listing;
#[path = "all/distribution_web_acl_association.rs"]
mod distribution_web_acl_association;
#[path = "all/distributions_by_cache_policy_id.rs"]
mod distributions_by_cache_policy_id;
#[path = "all/distributions_by_connection_function.rs"]
mod distributions_by_connection_function;
#[path = "all/distributions_by_connection_mode.rs"]
mod distributions_by_connection_mode;
#[path = "all/distributions_by_key_group.rs"]
mod distributions_by_key_group;
#[path = "all/distributions_by_origin_request_policy_id.rs"]
mod distributions_by_origin_request_policy_id;
#[path = "all/distributions_by_realtime_log_config.rs"]
mod distributions_by_realtime_log_config;
#[path = "all/distributions_by_response_headers_policy_id.rs"]
mod distributions_by_response_headers_policy_id;
#[path = "all/distributions_by_web_acl_id.rs"]
mod distributions_by_web_acl_id;
#[path = "all/domain_conflicts.rs"]
mod domain_conflicts;
#[path = "all/duplicate_name_conflict_wording.rs"]
mod duplicate_name_conflict_wording;
#[path = "all/edge_additional_metrics.rs"]
mod edge_additional_metrics;
#[path = "all/edge_data_plane.rs"]
mod edge_data_plane;
#[path = "all/edge_geo_restriction.rs"]
mod edge_geo_restriction;
#[path = "all/edge_realtime_logs.rs"]
mod edge_realtime_logs;
#[path = "all/edge_request_metrics.rs"]
mod edge_request_metrics;
#[path = "all/edge_request_shaping.rs"]
mod edge_request_shaping;
#[path = "all/edge_signed_access.rs"]
mod edge_signed_access;
#[path = "all/edge_web_acl.rs"]
mod edge_web_acl;
#[path = "all/enum_value_set_message.rs"]
mod enum_value_set_message;
#[path = "all/error_envelope.rs"]
mod error_envelope;
#[path = "all/field_level_encryption_config_delete.rs"]
mod field_level_encryption_config_delete;
#[path = "all/field_level_encryption_config_lifecycle.rs"]
mod field_level_encryption_config_lifecycle;
#[path = "all/field_level_encryption_config_list.rs"]
mod field_level_encryption_config_list;
#[path = "all/field_level_encryption_config_list_after_delete.rs"]
mod field_level_encryption_config_list_after_delete;
#[path = "all/field_level_encryption_config_update.rs"]
mod field_level_encryption_config_update;
#[path = "all/field_level_encryption_identifier_format.rs"]
mod field_level_encryption_identifier_format;
#[path = "all/field_level_encryption_profile_delete.rs"]
mod field_level_encryption_profile_delete;
#[path = "all/field_level_encryption_profile_lifecycle.rs"]
mod field_level_encryption_profile_lifecycle;
#[path = "all/field_level_encryption_profile_list.rs"]
mod field_level_encryption_profile_list;
#[path = "all/field_level_encryption_profile_refusal_message.rs"]
mod field_level_encryption_profile_refusal_message;
#[path = "all/field_level_encryption_profile_update.rs"]
mod field_level_encryption_profile_update;
#[path = "all/function_delete.rs"]
mod function_delete;
#[path = "all/function_lifecycle.rs"]
mod function_lifecycle;
#[path = "all/function_list.rs"]
mod function_list;
#[path = "all/function_publish.rs"]
mod function_publish;
#[path = "all/function_test_not_found_message.rs"]
mod function_test_not_found_message;
#[path = "all/function_test_run.rs"]
mod function_test_run;
#[path = "all/function_update.rs"]
mod function_update;
#[path = "all/generated_identifier_shapes.rs"]
mod generated_identifier_shapes;
#[path = "all/invalidation_authorization.rs"]
mod invalidation_authorization;
#[path = "all/invalidation_list.rs"]
mod invalidation_list;
#[path = "all/invalidation_path_set.rs"]
mod invalidation_path_set;
#[path = "all/key_group_config_read.rs"]
mod key_group_config_read;
#[path = "all/key_group_delete.rs"]
mod key_group_delete;
#[path = "all/key_group_lifecycle.rs"]
mod key_group_lifecycle;
#[path = "all/key_group_list.rs"]
mod key_group_list;
#[path = "all/key_group_refusal_message_source.rs"]
mod key_group_refusal_message_source;
#[path = "all/key_group_update.rs"]
mod key_group_update;
#[path = "all/key_value_store_delete.rs"]
mod key_value_store_delete;
#[path = "all/key_value_store_lifecycle.rs"]
mod key_value_store_lifecycle;
#[path = "all/key_value_store_list.rs"]
mod key_value_store_list;
#[path = "all/key_value_store_update.rs"]
mod key_value_store_update;
#[path = "all/lambda_edge_association.rs"]
mod lambda_edge_association;
#[path = "all/lambda_edge_association_message.rs"]
mod lambda_edge_association_message;
#[path = "all/lambda_edge_config_round_trip.rs"]
mod lambda_edge_config_round_trip;
#[path = "all/lambda_edge_execution_role_trust.rs"]
mod lambda_edge_execution_role_trust;
#[path = "all/managed_cache_policy_catalog.rs"]
mod managed_cache_policy_catalog;
#[path = "all/managed_cache_policy_delete_gate.rs"]
mod managed_cache_policy_delete_gate;
#[path = "all/managed_catalogue_order_and_list_defaults.rs"]
mod managed_catalogue_order_and_list_defaults;
#[path = "all/managed_certificate_details.rs"]
mod managed_certificate_details;
#[path = "all/managed_certificate_domain_ownership.rs"]
mod managed_certificate_domain_ownership;
#[path = "all/managed_origin_request_policy_reference.rs"]
mod managed_origin_request_policy_reference;
#[path = "all/managed_policy_catalogs.rs"]
mod managed_policy_catalogs;
#[path = "all/management_event_recording.rs"]
mod management_event_recording;
#[path = "all/monitoring_subscription.rs"]
mod monitoring_subscription;
#[path = "all/origin_access_control_config_read.rs"]
mod origin_access_control_config_read;
#[path = "all/origin_access_control_delete.rs"]
mod origin_access_control_delete;
#[path = "all/origin_access_control_lifecycle.rs"]
mod origin_access_control_lifecycle;
#[path = "all/origin_access_control_reference.rs"]
mod origin_access_control_reference;
#[path = "all/origin_access_control_update.rs"]
mod origin_access_control_update;
#[path = "all/origin_access_identity_delete.rs"]
mod origin_access_identity_delete;
#[path = "all/origin_access_identity_lifecycle.rs"]
mod origin_access_identity_lifecycle;
#[path = "all/origin_access_identity_list.rs"]
mod origin_access_identity_list;
#[path = "all/origin_access_identity_update.rs"]
mod origin_access_identity_update;
#[path = "all/origin_request_policy_config_read.rs"]
mod origin_request_policy_config_read;
#[path = "all/origin_request_policy_delete.rs"]
mod origin_request_policy_delete;
#[path = "all/origin_request_policy_lifecycle.rs"]
mod origin_request_policy_lifecycle;
#[path = "all/origin_request_policy_list.rs"]
mod origin_request_policy_list;
#[path = "all/origin_request_policy_update.rs"]
mod origin_request_policy_update;
#[path = "all/path_identifier_shape.rs"]
mod path_identifier_shape;
#[path = "all/precondition_family_wording.rs"]
mod precondition_family_wording;
#[path = "all/published_address_every_mint.rs"]
mod published_address_every_mint;
#[path = "all/published_edge_address.rs"]
mod published_edge_address;
#[path = "all/published_routing_endpoint.rs"]
mod published_routing_endpoint;
#[path = "all/public_key_delete.rs"]
mod public_key_delete;
#[path = "all/public_key_id_is_resolved_against_the_store.rs"]
mod public_key_id_is_resolved_against_the_store;
#[path = "all/public_key_lifecycle.rs"]
mod public_key_lifecycle;
#[path = "all/public_key_update.rs"]
mod public_key_update;
#[path = "all/realtime_log_config_lifecycle.rs"]
mod realtime_log_config_lifecycle;
#[path = "all/realtime_log_config_list.rs"]
mod realtime_log_config_list;
#[path = "all/refusal_contract.rs"]
mod refusal_contract;
#[path = "all/refusal_message_fidelity.rs"]
mod refusal_message_fidelity;
#[path = "all/request_supplied_order_and_values.rs"]
mod request_supplied_order_and_values;
#[path = "all/request_validation_limits.rs"]
mod request_validation_limits;
#[path = "all/resource_policy_absent_resource.rs"]
mod resource_policy_absent_resource;
#[path = "all/resource_policy_delete.rs"]
mod resource_policy_delete;
#[path = "all/resource_policy_lifecycle.rs"]
mod resource_policy_lifecycle;
#[path = "all/response_headers_policy_config_read.rs"]
mod response_headers_policy_config_read;
#[path = "all/response_headers_policy_delete.rs"]
mod response_headers_policy_delete;
#[path = "all/response_headers_policy_lifecycle.rs"]
mod response_headers_policy_lifecycle;
#[path = "all/response_headers_policy_refusal_message.rs"]
mod response_headers_policy_refusal_message;
#[path = "all/response_headers_policy_security_headers.rs"]
mod response_headers_policy_security_headers;
#[path = "all/response_headers_policy_update.rs"]
mod response_headers_policy_update;
#[path = "all/server_supplied_config_members.rs"]
mod server_supplied_config_members;
#[path = "all/staging_config_promotion.rs"]
mod staging_config_promotion;
#[path = "all/standard_logging_bucket.rs"]
mod standard_logging_bucket;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/stored_config_round_trip.rs"]
mod stored_config_round_trip;
#[path = "all/streaming_distribution_list.rs"]
mod streaming_distribution_list;
#[path = "all/tag_resource.rs"]
mod tag_resource;
#[path = "all/tagging_resource_types.rs"]
mod tagging_resource_types;
#[path = "all/tenant_distribution_and_method_order.rs"]
mod tenant_distribution_and_method_order;
#[path = "all/tenant_domain_admission.rs"]
mod tenant_domain_admission;
#[path = "all/tenant_domain_certificate_gate.rs"]
mod tenant_domain_certificate_gate;
#[path = "all/tenant_invalidation_batch_quantity.rs"]
mod tenant_invalidation_batch_quantity;
#[path = "all/tenant_invalidations.rs"]
mod tenant_invalidations;
#[path = "all/tenant_parameter_references.rs"]
mod tenant_parameter_references;
#[path = "all/tenant_unvalidated_domain_claim.rs"]
mod tenant_unvalidated_domain_claim;
#[path = "all/tenant_web_acl_association.rs"]
mod tenant_web_acl_association;
#[path = "all/tenant_web_acl_association_scope.rs"]
mod tenant_web_acl_association_scope;
#[path = "all/tenant_web_acl_disassociation_scope.rs"]
mod tenant_web_acl_disassociation_scope;
#[path = "all/trust_store_lifecycle.rs"]
mod trust_store_lifecycle;
#[path = "all/untag_resource.rs"]
mod untag_resource;
#[path = "all/verify_dns_configuration.rs"]
mod verify_dns_configuration;
#[path = "all/vpc_origin_lifecycle.rs"]
mod vpc_origin_lifecycle;
#[path = "all/web_acl_association.rs"]
mod web_acl_association;
#[path = "all/web_acl_scope_admission.rs"]
mod web_acl_scope_admission;
#[path = "all/wire_instant_ordering.rs"]
mod wire_instant_ordering;

