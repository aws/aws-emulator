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
#[path = "all/account_profile.rs"]
mod account_profile;
#[path = "all/active_names.rs"]
mod active_names;
#[path = "all/add_on_disable.rs"]
mod add_on_disable;
#[path = "all/add_on_next_snapshot_time.rs"]
mod add_on_next_snapshot_time;
#[path = "all/add_on_resource_resolution.rs"]
mod add_on_resource_resolution;
#[path = "all/add_on_snapshot_hour.rs"]
mod add_on_snapshot_hour;
#[path = "all/alarm_configuration.rs"]
mod alarm_configuration;
#[path = "all/alarm_load_balancer.rs"]
mod alarm_load_balancer;
#[path = "all/alarm_state_test.rs"]
mod alarm_state_test;
#[path = "all/auto_snapshot_add_on_delete.rs"]
mod auto_snapshot_add_on_delete;
#[path = "all/auto_snapshot_delete.rs"]
mod auto_snapshot_delete;
#[path = "all/auto_snapshots.rs"]
mod auto_snapshots;
#[path = "all/availability_zone_publication.rs"]
mod availability_zone_publication;
#[path = "all/blueprint_catalog.rs"]
mod blueprint_catalog;
#[path = "all/block_storage_region_scope.rs"]
mod block_storage_region_scope;
#[path = "all/bucket_access_keys.rs"]
mod bucket_access_keys;
#[path = "all/bucket_access_log_destination_delete.rs"]
mod bucket_access_log_destination_delete;
#[path = "all/bucket_bundle_update.rs"]
mod bucket_bundle_update;
#[path = "all/bucket_bundles.rs"]
mod bucket_bundles;
#[path = "all/bucket_configuration_update.rs"]
mod bucket_configuration_update;
#[path = "all/bucket_distribution_origin_delete.rs"]
mod bucket_distribution_origin_delete;
#[path = "all/bucket_error_envelope.rs"]
mod bucket_error_envelope;
#[path = "all/bucket_lifecycle.rs"]
mod bucket_lifecycle;
#[path = "all/bucket_metric_data.rs"]
mod bucket_metric_data;
#[path = "all/bucket_object_data_plane.rs"]
mod bucket_object_data_plane;
#[path = "all/bucket_resource_access.rs"]
mod bucket_resource_access;
#[path = "all/bundle_catalog.rs"]
mod bundle_catalog;
#[path = "all/certificate_alternate_domain_dedupe.rs"]
mod certificate_alternate_domain_dedupe;
#[path = "all/certificate_delete.rs"]
mod certificate_delete;
#[path = "all/certificate_in_use_resources.rs"]
mod certificate_in_use_resources;
#[path = "all/certificate_render_parity.rs"]
mod certificate_render_parity;
#[path = "all/certificate_request.rs"]
mod certificate_request;
#[path = "all/cloud_formation_stack.rs"]
mod cloud_formation_stack;
#[path = "all/cloud_formation_stack_source_kind.rs"]
mod cloud_formation_stack_source_kind;
#[path = "all/contact_method_notifications.rs"]
mod contact_method_notifications;
#[path = "all/contact_method_tag_bound.rs"]
mod contact_method_tag_bound;
#[path = "all/contact_method_verification.rs"]
mod contact_method_verification;
#[path = "all/container_api_metadata.rs"]
mod container_api_metadata;
#[path = "all/container_image_push_and_register.rs"]
mod container_image_push_and_register;
#[path = "all/container_image_registry.rs"]
mod container_image_registry;
#[path = "all/container_image_registry_login.rs"]
mod container_image_registry_login;
#[path = "all/container_service_delete.rs"]
mod container_service_delete;
#[path = "all/container_service_deployment.rs"]
mod container_service_deployment;
#[path = "all/container_service_deployment_history.rs"]
mod container_service_deployment_history;
#[path = "all/container_service_disabled_deployment.rs"]
mod container_service_disabled_deployment;
#[path = "all/container_service_health_check_success_codes.rs"]
mod container_service_health_check_success_codes;
#[path = "all/container_service_lifecycle.rs"]
mod container_service_lifecycle;
#[path = "all/container_service_log.rs"]
mod container_service_log;
#[path = "all/container_service_metric_data.rs"]
mod container_service_metric_data;
#[path = "all/container_service_operation_log.rs"]
mod container_service_operation_log;
#[path = "all/container_service_powers.rs"]
mod container_service_powers;
#[path = "all/container_service_public_domain_certificate.rs"]
mod container_service_public_domain_certificate;
#[path = "all/container_service_public_domain_name_rule.rs"]
mod container_service_public_domain_name_rule;
#[path = "all/container_service_public_endpoint.rs"]
mod container_service_public_endpoint;
#[path = "all/container_service_update.rs"]
mod container_service_update;
#[path = "all/cost_estimate.rs"]
mod cost_estimate;
#[path = "all/cost_estimate_region_restriction.rs"]
mod cost_estimate_region_restriction;
#[path = "all/create_time_tag_keys.rs"]
mod create_time_tag_keys;
#[path = "all/disk_attach_outstanding_settle.rs"]
mod disk_attach_outstanding_settle;
#[path = "all/disk_attachment.rs"]
mod disk_attachment;
#[path = "all/disk_block_storage.rs"]
mod disk_block_storage;
#[path = "all/disk_delete.rs"]
mod disk_delete;
#[path = "all/disk_detach.rs"]
mod disk_detach;
#[path = "all/disk_listing.rs"]
mod disk_listing;
#[path = "all/disk_listing_region_scope.rs"]
mod disk_listing_region_scope;
#[path = "all/disk_restore_from_auto_snapshot.rs"]
mod disk_restore_from_auto_snapshot;
#[path = "all/disk_restore_from_snapshot.rs"]
mod disk_restore_from_snapshot;
#[path = "all/disk_restore_snapshot_completeness.rs"]
mod disk_restore_snapshot_completeness;
#[path = "all/disk_snapshot.rs"]
mod disk_snapshot;
#[path = "all/disk_snapshot_delete.rs"]
mod disk_snapshot_delete;
#[path = "all/disk_snapshot_listing.rs"]
mod disk_snapshot_listing;
#[path = "all/disk_snapshot_listing_region_scope.rs"]
mod disk_snapshot_listing_region_scope;
#[path = "all/distribution_bundle_update.rs"]
mod distribution_bundle_update;
#[path = "all/distribution_bundles.rs"]
mod distribution_bundles;
#[path = "all/distribution_cache_behavior_settings.rs"]
mod distribution_cache_behavior_settings;
#[path = "all/distribution_cache_reset.rs"]
mod distribution_cache_reset;
#[path = "all/distribution_cache_reset_clock.rs"]
mod distribution_cache_reset_clock;
#[path = "all/distribution_cdn.rs"]
mod distribution_cdn;
#[path = "all/distribution_certificate_attachment.rs"]
mod distribution_certificate_attachment;
#[path = "all/distribution_certificate_detach.rs"]
mod distribution_certificate_detach;
#[path = "all/distribution_create_certificate_domains.rs"]
mod distribution_create_certificate_domains;
#[path = "all/distribution_create_certificate_validation.rs"]
mod distribution_create_certificate_validation;
#[path = "all/distribution_custom_error_page_path.rs"]
mod distribution_custom_error_page_path;
#[path = "all/distribution_custom_error_responses.rs"]
mod distribution_custom_error_responses;
#[path = "all/distribution_delete.rs"]
mod distribution_delete;
#[path = "all/distribution_ip_address_type_domain.rs"]
mod distribution_ip_address_type_domain;
#[path = "all/distribution_listing.rs"]
mod distribution_listing;
#[path = "all/distribution_metric_data.rs"]
mod distribution_metric_data;
#[path = "all/distribution_origin_resource_delete.rs"]
mod distribution_origin_resource_delete;
#[path = "all/distribution_update.rs"]
mod distribution_update;
#[path = "all/domain_delete.rs"]
mod domain_delete;
#[path = "all/domain_entry_delete.rs"]
mod domain_entry_delete;
#[path = "all/domain_entry_recordset.rs"]
mod domain_entry_recordset;
#[path = "all/domain_entry_update.rs"]
mod domain_entry_update;
#[path = "all/domain_listing.rs"]
mod domain_listing;
#[path = "all/domain_region_restriction.rs"]
mod domain_region_restriction;
#[path = "all/domain_zone.rs"]
mod domain_zone;
#[path = "all/export_snapshot_completeness.rs"]
mod export_snapshot_completeness;
#[path = "all/export_snapshot_records.rs"]
mod export_snapshot_records;
#[path = "all/export_snapshot_source_kinds.rs"]
mod export_snapshot_source_kinds;
#[path = "all/gui_session_access_details.rs"]
mod gui_session_access_details;
#[path = "all/gui_session_start.rs"]
mod gui_session_start;
#[path = "all/gui_session_stop.rs"]
mod gui_session_stop;
#[path = "all/instance_access_details.rs"]
mod instance_access_details;
#[path = "all/instance_delete_attachments.rs"]
mod instance_delete_attachments;
#[path = "all/instance_delete_bucket_access.rs"]
mod instance_delete_bucket_access;
#[path = "all/instance_error_envelope.rs"]
mod instance_error_envelope;
#[path = "all/instance_https_setup.rs"]
mod instance_https_setup;
#[path = "all/instance_lifecycle.rs"]
mod instance_lifecycle;
#[path = "all/instance_metadata_ipv6_region.rs"]
mod instance_metadata_ipv6_region;
#[path = "all/instance_metadata_options.rs"]
mod instance_metadata_options;
#[path = "all/instance_metric_data.rs"]
mod instance_metric_data;
#[path = "all/instance_public_ports.rs"]
mod instance_public_ports;
#[path = "all/instance_reboot.rs"]
mod instance_reboot;
#[path = "all/instance_restore_from_auto_snapshot.rs"]
mod instance_restore_from_auto_snapshot;
#[path = "all/instance_restore_from_snapshot.rs"]
mod instance_restore_from_snapshot;
#[path = "all/instance_snapshot_create.rs"]
mod instance_snapshot_create;
#[path = "all/instance_snapshot_delete.rs"]
mod instance_snapshot_delete;
#[path = "all/instance_snapshot_listing.rs"]
mod instance_snapshot_listing;
#[path = "all/instance_snapshot_read.rs"]
mod instance_snapshot_read;
#[path = "all/instance_start_addresses.rs"]
mod instance_start_addresses;
#[path = "all/inverted_time_window.rs"]
mod inverted_time_window;
#[path = "all/key_pair_create.rs"]
mod key_pair_create;
#[path = "all/key_pair_default_download.rs"]
mod key_pair_default_download;
#[path = "all/key_pair_delete.rs"]
mod key_pair_delete;
#[path = "all/key_pair_listing.rs"]
mod key_pair_listing;
#[path = "all/key_pair_read.rs"]
mod key_pair_read;
#[path = "all/known_host_keys_delete.rs"]
mod known_host_keys_delete;
#[path = "all/listing_page_token_scope.rs"]
mod listing_page_token_scope;
#[path = "all/listing_page_walk.rs"]
mod listing_page_walk;
#[path = "all/load_balancer_attribute_update.rs"]
mod load_balancer_attribute_update;
#[path = "all/load_balancer_certificate_domain_rule.rs"]
mod load_balancer_certificate_domain_rule;
#[path = "all/load_balancer_delete.rs"]
mod load_balancer_delete;
#[path = "all/load_balancer_instance_attachment.rs"]
mod load_balancer_instance_attachment;
#[path = "all/load_balancer_instance_detach.rs"]
mod load_balancer_instance_detach;
#[path = "all/load_balancer_listing.rs"]
mod load_balancer_listing;
#[path = "all/load_balancer_metric_data.rs"]
mod load_balancer_metric_data;
#[path = "all/load_balancer_provisioning.rs"]
mod load_balancer_provisioning;
#[path = "all/load_balancer_tls_certificate_attach.rs"]
mod load_balancer_tls_certificate_attach;
#[path = "all/load_balancer_tls_certificate_create.rs"]
mod load_balancer_tls_certificate_create;
#[path = "all/load_balancer_tls_certificate_delete.rs"]
mod load_balancer_tls_certificate_delete;
#[path = "all/load_balancer_tls_certificate_read.rs"]
mod load_balancer_tls_certificate_read;
#[path = "all/load_balancer_tls_certificate_tags.rs"]
mod load_balancer_tls_certificate_tags;
#[path = "all/load_balancer_tls_certificate_validation.rs"]
mod load_balancer_tls_certificate_validation;
#[path = "all/load_balancer_tls_policies.rs"]
mod load_balancer_tls_policies;
#[path = "all/monitored_resource_delete_alarms.rs"]
mod monitored_resource_delete_alarms;
#[path = "all/operation_log_naming.rs"]
mod operation_log_naming;
#[path = "all/operations_for_resource.rs"]
mod operations_for_resource;
#[path = "all/operations_log.rs"]
mod operations_log;
#[path = "all/paging.rs"]
mod paging;
#[path = "all/published_catalog_contents.rs"]
mod published_catalog_contents;
#[path = "all/published_catalog_reference_data.rs"]
mod published_catalog_reference_data;
#[path = "all/region_catalog.rs"]
mod region_catalog;
#[path = "all/relational_database_blueprints.rs"]
mod relational_database_blueprints;
#[path = "all/relational_database_bundles.rs"]
mod relational_database_bundles;
#[path = "all/relational_database_delete.rs"]
mod relational_database_delete;
#[path = "all/relational_database_events.rs"]
mod relational_database_events;
#[path = "all/relational_database_lifecycle.rs"]
mod relational_database_lifecycle;
#[path = "all/relational_database_listing.rs"]
mod relational_database_listing;
#[path = "all/relational_database_log_events.rs"]
mod relational_database_log_events;
#[path = "all/relational_database_log_streams.rs"]
mod relational_database_log_streams;
#[path = "all/relational_database_master_user_password.rs"]
mod relational_database_master_user_password;
#[path = "all/relational_database_metric_data.rs"]
mod relational_database_metric_data;
#[path = "all/relational_database_parameter_updates.rs"]
mod relational_database_parameter_updates;
#[path = "all/relational_database_parameters.rs"]
mod relational_database_parameters;
#[path = "all/relational_database_pre_stop_snapshot.rs"]
mod relational_database_pre_stop_snapshot;
#[path = "all/relational_database_reboot.rs"]
mod relational_database_reboot;
#[path = "all/relational_database_restore_from_snapshot.rs"]
mod relational_database_restore_from_snapshot;
#[path = "all/relational_database_snapshot_create.rs"]
mod relational_database_snapshot_create;
#[path = "all/relational_database_snapshot_delete.rs"]
mod relational_database_snapshot_delete;
#[path = "all/relational_database_snapshot_listing.rs"]
mod relational_database_snapshot_listing;
#[path = "all/relational_database_snapshot_read.rs"]
mod relational_database_snapshot_read;
#[path = "all/relational_database_start_stop.rs"]
mod relational_database_start_stop;
#[path = "all/relational_database_update.rs"]
mod relational_database_update;
#[path = "all/resource_add_ons.rs"]
mod resource_add_ons;
#[path = "all/resource_ip_address_type.rs"]
mod resource_ip_address_type;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/setup_history_reporting.rs"]
mod setup_history_reporting;
#[path = "all/snapshot_copy.rs"]
mod snapshot_copy;
#[path = "all/snapshot_name_namespace.rs"]
mod snapshot_name_namespace;
#[path = "all/snapshot_source_members.rs"]
mod snapshot_source_members;
#[path = "all/state_guard_refusal_messages.rs"]
mod state_guard_refusal_messages;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/static_ip_allocation.rs"]
mod static_ip_allocation;
#[path = "all/static_ip_attach.rs"]
mod static_ip_attach;
#[path = "all/static_ip_attach_region.rs"]
mod static_ip_attach_region;
#[path = "all/static_ip_detach.rs"]
mod static_ip_detach;
#[path = "all/static_ip_listing.rs"]
mod static_ip_listing;
#[path = "all/static_ip_release.rs"]
mod static_ip_release;
#[path = "all/tag_region_restriction.rs"]
mod tag_region_restriction;
#[path = "all/tagged_resource_coverage.rs"]
mod tagged_resource_coverage;
#[path = "all/vpc_peering.rs"]
mod vpc_peering;
#[path = "all/vpc_peering_operation_log.rs"]
mod vpc_peering_operation_log;
