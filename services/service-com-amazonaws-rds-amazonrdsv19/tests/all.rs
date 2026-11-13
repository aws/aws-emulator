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
#[path = "all/account_attributes.rs"]
mod account_attributes;
#[path = "all/already_exists_refusal_sentences.rs"]
mod already_exists_refusal_sentences;
#[path = "all/aurora_engine_parameterised_row_locks.rs"]
mod aurora_engine_parameterised_row_locks;
#[path = "all/aurora_engine_row_locks.rs"]
mod aurora_engine_row_locks;
#[path = "all/aws_managed_group_catalog.rs"]
mod aws_managed_group_catalog;
#[path = "all/ca_certificate_material.rs"]
mod ca_certificate_material;
#[path = "all/certificate_override.rs"]
mod certificate_override;
#[path = "all/cluster_create_refusal_wording.rs"]
mod cluster_create_refusal_wording;
#[path = "all/cluster_member_deletion.rs"]
mod cluster_member_deletion;
#[path = "all/cluster_parameter_groups.rs"]
mod cluster_parameter_groups;
#[path = "all/cluster_parameter_value_range.rs"]
mod cluster_parameter_value_range;
#[path = "all/cluster_role_and_export_key_refusals.rs"]
mod cluster_role_and_export_key_refusals;
#[path = "all/control_plane_operations.rs"]
mod control_plane_operations;
#[path = "all/create_path_refusal_wording.rs"]
mod create_path_refusal_wording;
#[path = "all/create_resolved_defaults.rs"]
mod create_resolved_defaults;
#[path = "all/custom_db_engine_versions.rs"]
mod custom_db_engine_versions;
#[path = "all/custom_engine_version_engine_availability.rs"]
mod custom_engine_version_engine_availability;
#[path = "all/data_api_http_endpoint.rs"]
mod data_api_http_endpoint;
#[path = "all/db_cluster_create_record.rs"]
mod db_cluster_create_record;
#[path = "all/db_cluster_deletion.rs"]
mod db_cluster_deletion;
#[path = "all/db_cluster_engine_endpoint.rs"]
mod db_cluster_engine_endpoint;
#[path = "all/db_cluster_envelope_members.rs"]
mod db_cluster_envelope_members;
#[path = "all/db_cluster_snapshot_envelope_members.rs"]
mod db_cluster_snapshot_envelope_members;
#[path = "all/db_cluster_snapshot_restore.rs"]
mod db_cluster_snapshot_restore;
#[path = "all/db_cluster_read_replica_promotion.rs"]
mod db_cluster_read_replica_promotion;
#[path = "all/db_cluster_snapshots.rs"]
mod db_cluster_snapshots;
#[path = "all/db_cluster_subject_operations.rs"]
mod db_cluster_subject_operations;
#[path = "all/db_clusters.rs"]
mod db_clusters;
#[path = "all/db_engine_version_catalog.rs"]
mod db_engine_version_catalog;
#[path = "all/db_engine_version_engine_selection.rs"]
mod db_engine_version_engine_selection;
#[path = "all/db_engine_version_wire_members.rs"]
mod db_engine_version_wire_members;
#[path = "all/db_engine_versions.rs"]
mod db_engine_versions;
#[path = "all/db_instance_deletion.rs"]
mod db_instance_deletion;
#[path = "all/db_instance_engine_endpoint.rs"]
mod db_instance_engine_endpoint;
#[path = "all/db_instance_envelope.rs"]
mod db_instance_envelope;
#[path = "all/db_instance_read_replica_promotion.rs"]
mod db_instance_read_replica_promotion;
#[path = "all/db_instance_read_replicas.rs"]
mod db_instance_read_replicas;
#[path = "all/db_instance_stop.rs"]
mod db_instance_stop;
#[path = "all/db_instance_subnet_group_projection.rs"]
mod db_instance_subnet_group_projection;
#[path = "all/db_instances.rs"]
mod db_instances;
#[path = "all/db_major_engine_versions.rs"]
mod db_major_engine_versions;
#[path = "all/db_parameter_group_copy.rs"]
mod db_parameter_group_copy;
#[path = "all/db_parameter_group_deletion.rs"]
mod db_parameter_group_deletion;
#[path = "all/db_parameter_group_provisioning.rs"]
mod db_parameter_group_provisioning;
#[path = "all/db_parameter_group_reset.rs"]
mod db_parameter_group_reset;
#[path = "all/db_parameter_groups.rs"]
mod db_parameter_groups;
#[path = "all/db_parameter_metadata.rs"]
mod db_parameter_metadata;
#[path = "all/db_parameters.rs"]
mod db_parameters;
#[path = "all/db_proxy_lifecycle.rs"]
mod db_proxy_lifecycle;
#[path = "all/db_proxy_modification.rs"]
mod db_proxy_modification;
#[path = "all/db_proxy_target_group_readiness.rs"]
mod db_proxy_target_group_readiness;
#[path = "all/db_proxy_target_groups.rs"]
mod db_proxy_target_groups;
#[path = "all/db_proxy_target_registration.rs"]
mod db_proxy_target_registration;
#[path = "all/db_recommendations.rs"]
mod db_recommendations;
#[path = "all/db_security_group_deletion.rs"]
mod db_security_group_deletion;
#[path = "all/db_security_group_ingress.rs"]
mod db_security_group_ingress;
#[path = "all/db_security_group_ingress_concurrency.rs"]
mod db_security_group_ingress_concurrency;
#[path = "all/db_security_groups.rs"]
mod db_security_groups;
#[path = "all/db_shard_group_primary_cluster.rs"]
mod db_shard_group_primary_cluster;
#[path = "all/db_shard_groups.rs"]
mod db_shard_groups;
#[path = "all/db_snapshot_deletion.rs"]
mod db_snapshot_deletion;
#[path = "all/db_snapshot_restore.rs"]
mod db_snapshot_restore;
#[path = "all/db_subnet_group_deletion.rs"]
mod db_subnet_group_deletion;
#[path = "all/db_subnet_group_discovered_zone_coverage.rs"]
mod db_subnet_group_discovered_zone_coverage;
#[path = "all/db_subnet_group_ec2_zones.rs"]
mod db_subnet_group_ec2_zones;
#[path = "all/db_subnet_group_in_use_wording.rs"]
mod db_subnet_group_in_use_wording;
#[path = "all/db_subnet_group_not_found_wording.rs"]
mod db_subnet_group_not_found_wording;
#[path = "all/db_subnet_group_resolution.rs"]
mod db_subnet_group_resolution;
#[path = "all/db_subnet_group_subnet_existence.rs"]
mod db_subnet_group_subnet_existence;
#[path = "all/delete_custom_db_engine_version.rs"]
mod delete_custom_db_engine_version;
#[path = "all/docdb_query_surface.rs"]
mod docdb_query_surface;
#[path = "all/encryption_key_accessibility.rs"]
mod encryption_key_accessibility;
#[path = "all/engine_capability_reports.rs"]
mod engine_capability_reports;
#[path = "all/engine_default_catalog_head.rs"]
mod engine_default_catalog_head;
#[path = "all/engine_default_cluster_parameters.rs"]
mod engine_default_cluster_parameters;
#[path = "all/engine_default_parameters.rs"]
mod engine_default_parameters;
#[path = "all/engine_result_column_types.rs"]
mod engine_result_column_types;
#[path = "all/engine_role_grants.rs"]
mod engine_role_grants;
#[path = "all/engine_row_lock_statement_bound.rs"]
mod engine_row_lock_statement_bound;
#[path = "all/engine_server_surface.rs"]
mod engine_server_surface;
#[path = "all/engine_session_isolation.rs"]
mod engine_session_isolation;
#[path = "all/engine_statement_classes.rs"]
mod engine_statement_classes;
#[path = "all/engine_version_families.rs"]
mod engine_version_families;
#[path = "all/engine_version_page_tokens.rs"]
mod engine_version_page_tokens;
#[path = "all/engine_version_release_lines.rs"]
mod engine_version_release_lines;
#[path = "all/event_subscription_envelope.rs"]
mod event_subscription_envelope;
#[path = "all/event_subscription_source_not_found_wording.rs"]
mod event_subscription_source_not_found_wording;
#[path = "all/event_subscription_source_removal.rs"]
mod event_subscription_source_removal;
#[path = "all/event_subscription_sources.rs"]
mod event_subscription_sources;
#[path = "all/event_subscriptions.rs"]
mod event_subscriptions;
#[path = "all/events.rs"]
mod events;
#[path = "all/existence_and_parameter_guards.rs"]
mod existence_and_parameter_guards;
#[path = "all/export_tasks.rs"]
mod export_tasks;
#[path = "all/global_cluster_member_resource_id.rs"]
mod global_cluster_member_resource_id;
#[path = "all/global_cluster_membership.rs"]
mod global_cluster_membership;
#[path = "all/global_cluster_modification.rs"]
mod global_cluster_modification;
#[path = "all/global_clusters.rs"]
mod global_clusters;
#[path = "all/global_database_published_versions.rs"]
mod global_database_published_versions;
#[path = "all/guard_order_and_refusal_classes.rs"]
mod guard_order_and_refusal_classes;
#[path = "all/iam_role_association_refusals.rs"]
mod iam_role_association_refusals;
#[path = "all/instance_configuration_guards.rs"]
mod instance_configuration_guards;
#[path = "all/instance_placement_and_backup_not_found_sentences.rs"]
mod instance_placement_and_backup_not_found_sentences;
#[path = "all/integration_deletion.rs"]
mod integration_deletion;
#[path = "all/lifecycle_poll_loops.rs"]
mod lifecycle_poll_loops;
#[path = "all/maintenance_action_resource_guard.rs"]
mod maintenance_action_resource_guard;
#[path = "all/managed_master_secret_cluster_creation.rs"]
mod managed_master_secret_cluster_creation;
#[path = "all/managed_master_user_credentials.rs"]
mod managed_master_user_credentials;
#[path = "all/managed_master_secret_key_bound.rs"]
mod managed_master_secret_key_bound;
#[path = "all/managed_master_user_password.rs"]
mod managed_master_user_password;
#[path = "all/managed_master_user_secret.rs"]
mod managed_master_user_secret;
#[path = "all/managed_secret_direct_write_refusal.rs"]
mod managed_secret_direct_write_refusal;
#[path = "all/managed_secret_service_ownership.rs"]
mod managed_secret_service_ownership;
#[path = "all/management_event_recording.rs"]
mod management_event_recording;
#[path = "all/modification_and_final_snapshot_refusal_sentences.rs"]
mod modification_and_final_snapshot_refusal_sentences;
#[path = "all/modify_custom_db_engine_version.rs"]
mod modify_custom_db_engine_version;
#[path = "all/modify_db_cluster_state_refusal.rs"]
mod modify_db_cluster_state_refusal;
#[path = "all/modify_db_shard_group.rs"]
mod modify_db_shard_group;
#[path = "all/modify_event_subscription.rs"]
mod modify_event_subscription;
#[path = "all/mysql_s3_load.rs"]
mod mysql_s3_load;
#[path = "all/neptune_create_gates.rs"]
mod neptune_create_gates;
#[path = "all/neptune_engine_catalog.rs"]
mod neptune_engine_catalog;
#[path = "all/neptune_probe.rs"]
mod neptune_probe;
#[path = "all/neptune_reference_catalogs.rs"]
mod neptune_reference_catalogs;
#[path = "all/not_found_paths.rs"]
mod not_found_paths;
#[path = "all/not_found_refusal_sentences.rs"]
mod not_found_refusal_sentences;
#[path = "all/option_group_copy.rs"]
mod option_group_copy;
#[path = "all/option_group_deletion.rs"]
mod option_group_deletion;
#[path = "all/option_group_options.rs"]
mod option_group_options;
#[path = "all/option_group_setting_catalog.rs"]
mod option_group_setting_catalog;
#[path = "all/option_groups.rs"]
mod option_groups;
#[path = "all/orderable_db_instance_options.rs"]
mod orderable_db_instance_options;
#[path = "all/pagination_marker_positions.rs"]
mod pagination_marker_positions;
#[path = "all/parameter_group_and_snapshot_copies.rs"]
mod parameter_group_and_snapshot_copies;
#[path = "all/parameter_group_apply_method.rs"]
mod parameter_group_apply_method;
#[path = "all/parameter_refusal_codes.rs"]
mod parameter_refusal_codes;
#[path = "all/postgres_rds_namespace_parameters.rs"]
mod postgres_rds_namespace_parameters;
#[path = "all/published_catalog_creates.rs"]
mod published_catalog_creates;
#[path = "all/query_catalog_reads.rs"]
mod query_catalog_reads;
#[path = "all/query_error_envelope.rs"]
mod query_error_envelope;
#[path = "all/rds_s3_integration.rs"]
mod rds_s3_integration;
#[path = "all/read_only_engine_endpoints.rs"]
mod read_only_engine_endpoints;
#[path = "all/reboot_db_shard_group.rs"]
mod reboot_db_shard_group;
#[path = "all/refusal_class_by_cause.rs"]
mod refusal_class_by_cause;
#[path = "all/refusal_wording_and_input_guards.rs"]
mod refusal_wording_and_input_guards;
#[path = "all/requests_aws_refuses.rs"]
mod requests_aws_refuses;
#[path = "all/resource_member_projection.rs"]
mod resource_member_projection;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/scratch_aurora_truth.rs"]
mod scratch_aurora_truth;
#[path = "all/serverless_v2_capacity_range.rs"]
mod serverless_v2_capacity_range;
#[path = "all/sibling_control_plane_clusters.rs"]
mod sibling_control_plane_clusters;
// Declared because the build enforces that every file under `tests/all/` is: the suite this
// names is an INERT placeholder (see its own docs) whose subject — `StartExportTask` and the
// objects it writes — is driven end to end by `rds_s3_integration` beside it.
#[path = "all/snapshot_export_to_s3.rs"]
mod snapshot_export_to_s3;
#[path = "all/state_precondition_refusal_sentences.rs"]
mod state_precondition_refusal_sentences;
#[path = "all/statement_budget_bounds.rs"]
mod statement_budget_bounds;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/static_reference_catalogs.rs"]
mod static_reference_catalogs;
#[path = "all/valid_db_instance_modifications.rs"]
mod valid_db_instance_modifications;
#[path = "all/waiting_statement_budget.rs"]
mod waiting_statement_budget;
#[path = "all/wire_decimal_doubles.rs"]
mod wire_decimal_doubles;
// Declared because the build enforces that every file under `tests/all/` is: the suite this
// names is an INERT placeholder (see its own docs) whose subject — the whole `DBCluster`
// envelope — is asserted member by member by `db_cluster_envelope_members`.
#[path = "all/zz_envelope_dump.rs"]
mod zz_envelope_dump;
