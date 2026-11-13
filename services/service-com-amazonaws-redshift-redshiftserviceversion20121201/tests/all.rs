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
#[path = "all/advisor_recommendations.rs"]
mod advisor_recommendations;
#[path = "all/authentication_profiles.rs"]
mod authentication_profiles;
#[path = "all/batch_delete_cluster_snapshots.rs"]
mod batch_delete_cluster_snapshots;
#[path = "all/batch_modify_cluster_snapshots.rs"]
mod batch_modify_cluster_snapshots;
#[path = "all/cluster_audit_logging.rs"]
mod cluster_audit_logging;
#[path = "all/cluster_credentials.rs"]
mod cluster_credentials;
#[path = "all/cluster_db_revisions.rs"]
mod cluster_db_revisions;
#[path = "all/cluster_endpoint_publication.rs"]
mod cluster_endpoint_publication;
#[path = "all/cluster_health_metrics.rs"]
mod cluster_health_metrics;
#[path = "all/cluster_iam_roles.rs"]
mod cluster_iam_roles;
#[path = "all/cluster_lifecycle.rs"]
mod cluster_lifecycle;
#[path = "all/cluster_not_found_messages.rs"]
mod cluster_not_found_messages;
#[path = "all/cluster_parameter_groups.rs"]
mod cluster_parameter_groups;
#[path = "all/cluster_parameters.rs"]
mod cluster_parameters;
#[path = "all/cluster_request_validation.rs"]
mod cluster_request_validation;
#[path = "all/cluster_resize_progress.rs"]
mod cluster_resize_progress;
#[path = "all/cluster_security_groups.rs"]
mod cluster_security_groups;
#[path = "all/cluster_structure.rs"]
mod cluster_structure;
#[path = "all/cluster_subnet_group_network.rs"]
mod cluster_subnet_group_network;
#[path = "all/cluster_transition_convergence.rs"]
mod cluster_transition_convergence;
#[path = "all/cluster_versions.rs"]
mod cluster_versions;
#[path = "all/cluster_vpc_identity.rs"]
mod cluster_vpc_identity;
#[path = "all/configuration_refusal_wording.rs"]
mod configuration_refusal_wording;
#[path = "all/copy_cluster_snapshot.rs"]
mod copy_cluster_snapshot;
#[path = "all/custom_domain_and_partners.rs"]
mod custom_domain_and_partners;
#[path = "all/custom_domain_association_refusal_wording.rs"]
mod custom_domain_association_refusal_wording;
#[path = "all/default_cluster_parameters.rs"]
mod default_cluster_parameters;
#[path = "all/delete_cluster_snapshot.rs"]
mod delete_cluster_snapshot;
#[path = "all/endpoint_access.rs"]
mod endpoint_access;
#[path = "all/endpoint_and_usage_limit_refusal_wording.rs"]
mod endpoint_and_usage_limit_refusal_wording;
#[path = "all/error_envelope.rs"]
mod error_envelope;
#[path = "all/event_categories.rs"]
mod event_categories;
#[path = "all/event_notifications.rs"]
mod event_notifications;
#[path = "all/event_subscriptions.rs"]
mod event_subscriptions;
#[path = "all/events.rs"]
mod events;
#[path = "all/existence_preconditions.rs"]
mod existence_preconditions;
#[path = "all/hsm_client_certificates.rs"]
mod hsm_client_certificates;
#[path = "all/hsm_configurations.rs"]
mod hsm_configurations;
#[path = "all/identity_center_auth_token.rs"]
mod identity_center_auth_token;
#[path = "all/lakehouse_configuration.rs"]
mod lakehouse_configuration;
#[path = "all/maintenance_track_refusal_wording.rs"]
mod maintenance_track_refusal_wording;
#[path = "all/maintenance_tracks.rs"]
mod maintenance_tracks;
#[path = "all/modify_cluster_db_revision.rs"]
mod modify_cluster_db_revision;
#[path = "all/modify_cluster_parameter_group.rs"]
mod modify_cluster_parameter_group;
#[path = "all/modify_cluster_snapshot.rs"]
mod modify_cluster_snapshot;
#[path = "all/multi_az_secondary.rs"]
mod multi_az_secondary;
#[path = "all/namespace_registration.rs"]
mod namespace_registration;
#[path = "all/namespace_registration_serverless.rs"]
mod namespace_registration_serverless;
#[path = "all/orderable_cluster_options.rs"]
mod orderable_cluster_options;
#[path = "all/pause_cluster.rs"]
mod pause_cluster;
#[path = "all/published_catalog_fidelity.rs"]
mod published_catalog_fidelity;
#[path = "all/published_catalog_values.rs"]
mod published_catalog_values;
#[path = "all/qev2_idc_applications.rs"]
mod qev2_idc_applications;
#[path = "all/query_protocol_refusals.rs"]
mod query_protocol_refusals;
#[path = "all/reboot_cluster.rs"]
mod reboot_cluster;
#[path = "all/redshift_idc_applications.rs"]
mod redshift_idc_applications;
#[path = "all/refusal_message_templates.rs"]
mod refusal_message_templates;
#[path = "all/refusal_payloads_use_aws_wording.rs"]
mod refusal_payloads_use_aws_wording;
#[path = "all/resource_policies.rs"]
mod resource_policies;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/reserved_node_exchange.rs"]
mod reserved_node_exchange;
#[path = "all/reserved_node_offerings.rs"]
mod reserved_node_offerings;
#[path = "all/reserved_node_refusal_wording.rs"]
mod reserved_node_refusal_wording;
#[path = "all/reserved_nodes.rs"]
mod reserved_nodes;
#[path = "all/reset_cluster_parameter_group.rs"]
mod reset_cluster_parameter_group;
#[path = "all/restored_cluster_independence.rs"]
mod restored_cluster_independence;
#[path = "all/scheduled_actions.rs"]
mod scheduled_actions;
#[path = "all/snapshot_access.rs"]
mod snapshot_access;
#[path = "all/snapshot_access_denied.rs"]
mod snapshot_access_denied;
#[path = "all/snapshot_access_revoke.rs"]
mod snapshot_access_revoke;
#[path = "all/snapshot_copy_configuration.rs"]
mod snapshot_copy_configuration;
#[path = "all/snapshot_copy_disable.rs"]
mod snapshot_copy_disable;
#[path = "all/snapshot_copy_grant_kms_keys.rs"]
mod snapshot_copy_grant_kms_keys;
#[path = "all/snapshot_copy_grants.rs"]
mod snapshot_copy_grants;
#[path = "all/snapshot_copy_retention_period.rs"]
mod snapshot_copy_retention_period;
#[path = "all/snapshot_data.rs"]
mod snapshot_data;
#[path = "all/snapshot_restore.rs"]
mod snapshot_restore;
#[path = "all/snapshot_schedules.rs"]
mod snapshot_schedules;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/static_catalog.rs"]
mod static_catalog;
#[path = "all/storage_summary.rs"]
mod storage_summary;
#[path = "all/usage_limits.rs"]
mod usage_limits;
#[path = "all/zero_etl_integration_sources.rs"]
mod zero_etl_integration_sources;
#[path = "all/zero_etl_integration_targets.rs"]
mod zero_etl_integration_targets;
#[path = "all/zero_etl_integrations.rs"]
mod zero_etl_integrations;
#[path = "all/zero_etl_source_decides_before_the_target.rs"]
mod zero_etl_source_decides_before_the_target;
