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
#[path = "all/absent_scope_listings.rs"]
mod absent_scope_listings;
#[path = "all/action_type_value_set_order.rs"]
mod action_type_value_set_order;
#[path = "all/advanced_security_configuration.rs"]
mod advanced_security_configuration;
#[path = "all/application_capabilities.rs"]
mod application_capabilities;
#[path = "all/application_create_refusals.rs"]
mod application_create_refusals;
#[path = "all/application_deletion.rs"]
mod application_deletion;
#[path = "all/application_listing.rs"]
mod application_listing;
#[path = "all/application_plane_identity.rs"]
mod application_plane_identity;
#[path = "all/application_updates.rs"]
mod application_updates;
#[path = "all/applications.rs"]
mod applications;
#[path = "all/authorized_principal_type.rs"]
mod authorized_principal_type;
#[path = "all/catalog_publication_order.rs"]
mod catalog_publication_order;
#[path = "all/change_progress.rs"]
mod change_progress;
#[path = "all/clock_owed_readiness.rs"]
mod clock_owed_readiness;
#[path = "all/cluster_apis.rs"]
mod cluster_apis;
#[path = "all/cluster_config_round_trip.rs"]
mod cluster_config_round_trip;
#[path = "all/cognito_dashboards_authentication.rs"]
mod cognito_dashboards_authentication;
#[path = "all/collection_element_refusals.rs"]
mod collection_element_refusals;
#[path = "all/compatible_versions.rs"]
mod compatible_versions;
#[path = "all/config_change_cancellation.rs"]
mod config_change_cancellation;
#[path = "all/connection_decisions.rs"]
mod connection_decisions;
#[path = "all/connection_records_at_rest.rs"]
mod connection_records_at_rest;
#[path = "all/connection_status_refusal.rs"]
mod connection_status_refusal;
#[path = "all/constraint_refusal_messages.rs"]
mod constraint_refusal_messages;
#[path = "all/control_plane_refusal_wording.rs"]
mod control_plane_refusal_wording;
#[path = "all/cross_cluster_connections.rs"]
mod cross_cluster_connections;
#[path = "all/cross_cluster_search.rs"]
mod cross_cluster_search;
#[path = "all/current_generation_instance_types.rs"]
mod current_generation_instance_types;
#[path = "all/custom_endpoint_certificate.rs"]
mod custom_endpoint_certificate;
#[path = "all/data_plane_iam_authorization.rs"]
mod data_plane_iam_authorization;
#[path = "all/data_plane_identity_evidence.rs"]
mod data_plane_identity_evidence;
#[path = "all/data_plane_identity_grant_evidence.rs"]
mod data_plane_identity_grant_evidence;
#[path = "all/data_plane_unreadable_identity_grant.rs"]
mod data_plane_unreadable_identity_grant;
#[path = "all/data_source_attachment_authorization.rs"]
mod data_source_attachment_authorization;
#[path = "all/data_source_attachments.rs"]
mod data_source_attachments;
#[path = "all/data_source_create_rules.rs"]
mod data_source_create_rules;
#[path = "all/data_source_kinds.rs"]
mod data_source_kinds;
#[path = "all/data_source_registry.rs"]
mod data_source_registry;
#[path = "all/data_source_updates.rs"]
mod data_source_updates;
#[path = "all/default_application_setting.rs"]
mod default_application_setting;
#[path = "all/direct_query_data_source_deletion.rs"]
mod direct_query_data_source_deletion;
#[path = "all/direct_query_data_source_inventory.rs"]
mod direct_query_data_source_inventory;
#[path = "all/direct_query_data_source_updates.rs"]
mod direct_query_data_source_updates;
#[path = "all/direct_query_data_sources.rs"]
mod direct_query_data_sources;
#[path = "all/domain_activity_gates.rs"]
mod domain_activity_gates;
#[path = "all/domain_analysis.rs"]
mod domain_analysis;
#[path = "all/domain_auto_tunes.rs"]
mod domain_auto_tunes;
#[path = "all/domain_batch_describe.rs"]
mod domain_batch_describe;
#[path = "all/domain_configuration_rules.rs"]
mod domain_configuration_rules;
#[path = "all/domain_descriptor_projection.rs"]
mod domain_descriptor_projection;
#[path = "all/domain_endpoint_publication.rs"]
mod domain_endpoint_publication;
#[path = "all/domain_health.rs"]
mod domain_health;
#[path = "all/domain_input_validation.rs"]
mod domain_input_validation;
#[path = "all/domain_lifecycle.rs"]
mod domain_lifecycle;
#[path = "all/domain_maintenance.rs"]
mod domain_maintenance;
#[path = "all/domain_nodes.rs"]
mod domain_nodes;
#[path = "all/domain_package_listing_cursor.rs"]
mod domain_package_listing_cursor;
#[path = "all/domain_rest_bulk_and_index_apis.rs"]
mod domain_rest_bulk_and_index_apis;
#[path = "all/domain_rest_data_plane.rs"]
mod domain_rest_data_plane;
#[path = "all/domain_server_defaults.rs"]
mod domain_server_defaults;
#[path = "all/domain_vpc_placement.rs"]
mod domain_vpc_placement;
#[path = "all/dry_run_progress.rs"]
mod dry_run_progress;
#[path = "all/ebs_volume_size_limit.rs"]
mod ebs_volume_size_limit;
#[path = "all/encryption_at_rest_key_resolution.rs"]
mod encryption_at_rest_key_resolution;
#[path = "all/engine_version_resolution.rs"]
mod engine_version_resolution;
#[path = "all/fgac_master_role_probe.rs"]
mod fgac_master_role_probe;
#[path = "all/fixed_sentence_refusals.rs"]
mod fixed_sentence_refusals;
#[path = "all/inbound_connection_acceptance.rs"]
mod inbound_connection_acceptance;
#[path = "all/inbound_connections.rs"]
mod inbound_connections;
#[path = "all/index_analyzer_resolution.rs"]
mod index_analyzer_resolution;
#[path = "all/index_authorization.rs"]
mod index_authorization;
#[path = "all/index_deletion.rs"]
mod index_deletion;
#[path = "all/index_document_writes.rs"]
mod index_document_writes;
#[path = "all/index_modelled_answers.rs"]
mod index_modelled_answers;
#[path = "all/index_registry.rs"]
mod index_registry;
#[path = "all/index_schema_updates.rs"]
mod index_schema_updates;
#[path = "all/index_settings_api.rs"]
mod index_settings_api;
#[path = "all/insight_detail_answerability.rs"]
mod insight_detail_answerability;
#[path = "all/insight_details.rs"]
mod insight_details;
#[path = "all/insight_feedback.rs"]
mod insight_feedback;
#[path = "all/insights.rs"]
mod insights;
#[path = "all/instance_type_catalog.rs"]
mod instance_type_catalog;
#[path = "all/instance_type_engine_refusal.rs"]
mod instance_type_engine_refusal;
#[path = "all/instance_type_limits.rs"]
mod instance_type_limits;
#[path = "all/log_publishing_delivery.rs"]
mod log_publishing_delivery;
#[path = "all/log_publishing_destinations.rs"]
mod log_publishing_destinations;
#[path = "all/log_type_key_constraint.rs"]
mod log_type_key_constraint;
#[path = "all/migration_listing.rs"]
mod migration_listing;
#[path = "all/migration_owner_resolution.rs"]
mod migration_owner_resolution;
#[path = "all/migration_source_constraint.rs"]
mod migration_source_constraint;
#[path = "all/missing_offering_refusals.rs"]
mod missing_offering_refusals;
#[path = "all/missing_resource_refusals.rs"]
mod missing_resource_refusals;
#[path = "all/missing_resource_status.rs"]
mod missing_resource_status;
#[path = "all/package_association_listings.rs"]
mod package_association_listings;
#[path = "all/package_associations.rs"]
mod package_associations;
#[path = "all/package_deletion.rs"]
mod package_deletion;
#[path = "all/package_dependency_lifecycle.rs"]
mod package_dependency_lifecycle;
#[path = "all/package_dissociation.rs"]
mod package_dissociation;
#[path = "all/package_dissociation_batches.rs"]
mod package_dissociation_batches;
#[path = "all/package_dissociation_on_missing_domain.rs"]
mod package_dissociation_on_missing_domain;
#[path = "all/package_listing_for_missing_domain.rs"]
mod package_listing_for_missing_domain;
#[path = "all/package_listing_page_bounds.rs"]
mod package_listing_page_bounds;
#[path = "all/package_registry.rs"]
mod package_registry;
#[path = "all/package_scope.rs"]
mod package_scope;
#[path = "all/package_version_history.rs"]
mod package_version_history;
#[path = "all/package_versions.rs"]
mod package_versions;
#[path = "all/pagination_contract.rs"]
mod pagination_contract;
#[path = "all/published_catalog_order.rs"]
mod published_catalog_order;
#[path = "all/published_domain_address.rs"]
mod published_domain_address;
#[path = "all/reference_catalogs.rs"]
mod reference_catalogs;
#[path = "all/reserved_instance_offerings.rs"]
mod reserved_instance_offerings;
#[path = "all/reserved_instances.rs"]
mod reserved_instances;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/saved_object_migrations.rs"]
mod saved_object_migrations;
#[path = "all/scheduled_action_updates.rs"]
mod scheduled_action_updates;
#[path = "all/scheduled_actions.rs"]
mod scheduled_actions;
#[path = "all/search_ordering_and_highlighting.rs"]
mod search_ordering_and_highlighting;
#[path = "all/service_software_rollback.rs"]
mod service_software_rollback;
#[path = "all/service_software_update.rs"]
mod service_software_update;
#[path = "all/shared_es_signing_name_claim.rs"]
mod shared_es_signing_name_claim;
#[path = "all/single_package_association.rs"]
mod single_package_association;
#[path = "all/single_package_dissociation.rs"]
mod single_package_dissociation;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/successive_domain_transitions.rs"]
mod successive_domain_transitions;
#[path = "all/tag_accumulation.rs"]
mod tag_accumulation;
#[path = "all/unreadable_request_values.rs"]
mod unreadable_request_values;
#[path = "all/unresolvable_package_id.rs"]
mod unresolvable_package_id;
#[path = "all/upgrade_eligibility_checks.rs"]
mod upgrade_eligibility_checks;
#[path = "all/upgrade_history.rs"]
mod upgrade_history;
#[path = "all/upgrade_progress_convergence.rs"]
mod upgrade_progress_convergence;
#[path = "all/upgrade_status.rs"]
mod upgrade_status;
#[path = "all/upgrade_status_envelope.rs"]
mod upgrade_status_envelope;
#[path = "all/upgrade_status_reporting.rs"]
mod upgrade_status_reporting;
#[path = "all/upgrade_target_and_maintenance_outcome.rs"]
mod upgrade_target_and_maintenance_outcome;
#[path = "all/version_catalog.rs"]
mod version_catalog;
#[path = "all/vpc_access_endpoints.rs"]
mod vpc_access_endpoints;
#[path = "all/vpc_endpoint_access.rs"]
mod vpc_endpoint_access;
#[path = "all/vpc_endpoint_access_on_absent_domain.rs"]
mod vpc_endpoint_access_on_absent_domain;
#[path = "all/vpc_endpoint_access_revocation.rs"]
mod vpc_endpoint_access_revocation;
#[path = "all/vpc_endpoint_transient_statuses.rs"]
mod vpc_endpoint_transient_statuses;
#[path = "all/vpc_endpoint_updates.rs"]
mod vpc_endpoint_updates;
#[path = "all/vpc_endpoints.rs"]
mod vpc_endpoints;
#[path = "all/vpc_endpoints_for_domain.rs"]
mod vpc_endpoints_for_domain;
#[path = "all/vpc_endpoints_for_domain_status.rs"]
mod vpc_endpoints_for_domain_status;
#[path = "all/zz_probe.rs"]
mod zz_probe;
