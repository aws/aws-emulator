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
#[path = "all/broker_count_update.rs"]
mod broker_count_update;
#[path = "all/broker_reboot.rs"]
mod broker_reboot;
#[path = "all/broker_reboot_window.rs"]
mod broker_reboot_window;
#[path = "all/broker_storage_update.rs"]
mod broker_storage_update;
#[path = "all/broker_type_update.rs"]
mod broker_type_update;
#[path = "all/channel_admission.rs"]
mod channel_admission;
#[path = "all/channel_cluster_first_gate.rs"]
mod channel_cluster_first_gate;
#[path = "all/channel_deletion.rs"]
mod channel_deletion;
#[path = "all/channel_destination_update.rs"]
mod channel_destination_update;
#[path = "all/channel_lifecycle.rs"]
mod channel_lifecycle;
#[path = "all/channel_listing.rs"]
mod channel_listing;
#[path = "all/client_vpc_connection_listing.rs"]
mod client_vpc_connection_listing;
#[path = "all/client_vpc_connection_rejection.rs"]
mod client_vpc_connection_rejection;
#[path = "all/clock_owed_cluster_values.rs"]
mod clock_owed_cluster_values;
#[path = "all/cluster_arn_refusal.rs"]
mod cluster_arn_refusal;
#[path = "all/cluster_authorization.rs"]
mod cluster_authorization;
#[path = "all/cluster_configuration_update.rs"]
mod cluster_configuration_update;
#[path = "all/cluster_delete_state_guard.rs"]
mod cluster_delete_state_guard;
#[path = "all/cluster_lifecycle.rs"]
mod cluster_lifecycle;
#[path = "all/cluster_operation_listing.rs"]
mod cluster_operation_listing;
#[path = "all/cluster_operation_listing_v2.rs"]
mod cluster_operation_listing_v2;
#[path = "all/cluster_operation_v2.rs"]
mod cluster_operation_v2;
#[path = "all/cluster_policy.rs"]
mod cluster_policy;
#[path = "all/cluster_resolved_before_member_values.rs"]
mod cluster_resolved_before_member_values;
#[path = "all/compatible_kafka_versions.rs"]
mod compatible_kafka_versions;
#[path = "all/configuration_arn_refusal.rs"]
mod configuration_arn_refusal;
#[path = "all/configuration_deletion.rs"]
mod configuration_deletion;
#[path = "all/configuration_lifecycle.rs"]
mod configuration_lifecycle;
#[path = "all/configuration_name_character_set.rs"]
mod configuration_name_character_set;
#[path = "all/configuration_revision_listing.rs"]
mod configuration_revision_listing;
#[path = "all/configuration_revision_properties.rs"]
mod configuration_revision_properties;
#[path = "all/configuration_revision_refusal.rs"]
mod configuration_revision_refusal;
#[path = "all/configuration_revisions.rs"]
mod configuration_revisions;
#[path = "all/configuration_server_properties.rs"]
mod configuration_server_properties;
#[path = "all/configuration_state_reporting.rs"]
mod configuration_state_reporting;
#[path = "all/connectivity_update.rs"]
mod connectivity_update;
#[path = "all/create_and_update_input_gates.rs"]
mod create_and_update_input_gates;
#[path = "all/create_cluster_v2.rs"]
mod create_cluster_v2;
#[path = "all/delete_version_precondition_attribution.rs"]
mod delete_version_precondition_attribution;
#[path = "all/describe_cluster_v2.rs"]
mod describe_cluster_v2;
#[path = "all/identity_policy_gate.rs"]
mod identity_policy_gate;
#[path = "all/iso8601_timestamps.rs"]
mod iso8601_timestamps;
#[path = "all/kafka_harness.rs"]
mod kafka_harness;
#[path = "all/kafka_version_catalog_rows.rs"]
mod kafka_version_catalog_rows;
#[path = "all/kafka_version_lifecycle_rows.rs"]
mod kafka_version_lifecycle_rows;
#[path = "all/kafka_version_update.rs"]
mod kafka_version_update;
#[path = "all/kafka_versions.rs"]
mod kafka_versions;
#[path = "all/lifecycle_sequences_settle.rs"]
mod lifecycle_sequences_settle;
#[path = "all/list_clusters_v2.rs"]
mod list_clusters_v2;
#[path = "all/monitoring_update.rs"]
mod monitoring_update;
#[path = "all/node_listing.rs"]
mod node_listing;
#[path = "all/placement_subnet_resolution.rs"]
mod placement_subnet_resolution;
#[path = "all/rebalancing_update.rs"]
mod rebalancing_update;
#[path = "all/reference_only_replicator.rs"]
mod reference_only_replicator;
#[path = "all/refusal_text_fidelity.rs"]
mod refusal_text_fidelity;
#[path = "all/refusal_wording.rs"]
mod refusal_wording;
#[path = "all/replication_end_naming.rs"]
mod replication_end_naming;
#[path = "all/replication_info_update.rs"]
mod replication_info_update;
#[path = "all/replicator_input_cardinality.rs"]
mod replicator_input_cardinality;
#[path = "all/replicator_lifecycle.rs"]
mod replicator_lifecycle;
#[path = "all/replicator_listing.rs"]
mod replicator_listing;
#[path = "all/replicator_provisioning.rs"]
mod replicator_provisioning;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/scram_secret_admission.rs"]
mod scram_secret_admission;
#[path = "all/scram_secret_cluster_state.rs"]
mod scram_secret_cluster_state;
#[path = "all/scram_secrets.rs"]
mod scram_secrets;
#[path = "all/security_update.rs"]
mod security_update;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/storage_updates.rs"]
mod storage_updates;
#[path = "all/topic_apis_serverless_gate.rs"]
mod topic_apis_serverless_gate;
#[path = "all/topic_cluster_arn_authorization.rs"]
mod topic_cluster_arn_authorization;
#[path = "all/topic_lifecycle.rs"]
mod topic_lifecycle;
#[path = "all/topic_lookup_miss_refusal.rs"]
mod topic_lookup_miss_refusal;
#[path = "all/topic_partition_reporting.rs"]
mod topic_partition_reporting;
#[path = "all/topic_replication.rs"]
mod topic_replication;
#[path = "all/topic_update.rs"]
mod topic_update;
#[path = "all/version_catalog_consistency.rs"]
mod version_catalog_consistency;
#[path = "all/vpc_connection_admission.rs"]
mod vpc_connection_admission;
#[path = "all/vpc_connection_contention.rs"]
mod vpc_connection_contention;
#[path = "all/vpc_connection_lifecycle.rs"]
mod vpc_connection_lifecycle;
#[path = "all/vpc_connection_listing.rs"]
mod vpc_connection_listing;
