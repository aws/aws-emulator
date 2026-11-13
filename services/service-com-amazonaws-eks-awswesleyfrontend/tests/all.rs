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
#[path = "all/access_entries.rs"]
mod access_entries;
#[path = "all/access_entry_authentication_mode.rs"]
mod access_entry_authentication_mode;
#[path = "all/access_entry_listing.rs"]
mod access_entry_listing;
#[path = "all/access_entry_node_groups.rs"]
mod access_entry_node_groups;
#[path = "all/access_entry_updates.rs"]
mod access_entry_updates;
#[path = "all/access_policy_catalog.rs"]
mod access_policy_catalog;
#[path = "all/access_policy_revocation.rs"]
mod access_policy_revocation;
#[path = "all/addon_bootstrap_lifecycle.rs"]
mod addon_bootstrap_lifecycle;
#[path = "all/addon_catalog_coverage.rs"]
mod addon_catalog_coverage;
#[path = "all/addon_catalog_metadata.rs"]
mod addon_catalog_metadata;
#[path = "all/addon_catalog_publication.rs"]
mod addon_catalog_publication;
#[path = "all/addon_deletion.rs"]
mod addon_deletion;
#[path = "all/addon_install_outcome.rs"]
mod addon_install_outcome;
#[path = "all/addon_lifecycle.rs"]
mod addon_lifecycle;
#[path = "all/addon_pod_identity_catalog.rs"]
mod addon_pod_identity_catalog;
#[path = "all/addon_request_precedence.rs"]
mod addon_request_precedence;
#[path = "all/addon_request_validation.rs"]
mod addon_request_validation;
#[path = "all/addon_update.rs"]
mod addon_update;
#[path = "all/addon_version_records.rs"]
mod addon_version_records;
#[path = "all/addon_versions.rs"]
mod addon_versions;
#[path = "all/cancel_update.rs"]
mod cancel_update;
#[path = "all/capability_lifecycle.rs"]
mod capability_lifecycle;
#[path = "all/capability_request_refusals.rs"]
mod capability_request_refusals;
#[path = "all/capability_update.rs"]
mod capability_update;
#[path = "all/catalog_discovery.rs"]
mod catalog_discovery;
#[path = "all/catalog_reference_data.rs"]
mod catalog_reference_data;
#[path = "all/certificate_authority.rs"]
mod certificate_authority;
#[path = "all/certificate_authority_activation.rs"]
mod certificate_authority_activation;
#[path = "all/certificate_authority_deletion.rs"]
mod certificate_authority_deletion;
#[path = "all/certificate_authority_listing.rs"]
mod certificate_authority_listing;
#[path = "all/certificate_authority_material.rs"]
mod certificate_authority_material;
#[path = "all/certificate_authority_listing_order.rs"]
mod certificate_authority_listing_order;
#[path = "all/certificate_authority_read_refusal.rs"]
mod certificate_authority_read_refusal;
#[path = "all/certificate_authority_trust_list.rs"]
mod certificate_authority_trust_list;
#[path = "all/child_create_resolves_the_cluster_first.rs"]
mod child_create_resolves_the_cluster_first;
#[path = "all/child_resource_deletion.rs"]
mod child_resource_deletion;
#[path = "all/cluster_admission_rules.rs"]
mod cluster_admission_rules;
#[path = "all/cluster_config_update.rs"]
mod cluster_config_update;
#[path = "all/cluster_creator_access_entry.rs"]
mod cluster_creator_access_entry;
#[path = "all/cluster_endpoint_publication.rs"]
mod cluster_endpoint_publication;
#[path = "all/cluster_lifecycle.rs"]
mod cluster_lifecycle;
#[path = "all/cluster_payload_members.rs"]
mod cluster_payload_members;
#[path = "all/cluster_request_validation.rs"]
mod cluster_request_validation;
#[path = "all/cluster_scalar_coercion.rs"]
mod cluster_scalar_coercion;
#[path = "all/cluster_update_concurrency.rs"]
mod cluster_update_concurrency;
#[path = "all/cluster_version_catalog.rs"]
mod cluster_version_catalog;
#[path = "all/cluster_version_update.rs"]
mod cluster_version_update;
#[path = "all/cluster_versions.rs"]
mod cluster_versions;
#[path = "all/connected_cluster_gate.rs"]
mod connected_cluster_gate;
#[path = "all/connected_clusters.rs"]
mod connected_clusters;
#[path = "all/control_plane_log_export.rs"]
mod control_plane_log_export;
#[path = "all/control_plane_log_suppression.rs"]
mod control_plane_log_suppression;
#[path = "all/eks_anywhere_subscription_mutations.rs"]
mod eks_anywhere_subscription_mutations;
#[path = "all/eks_anywhere_subscriptions.rs"]
mod eks_anywhere_subscriptions;
#[path = "all/encryption_config_association.rs"]
mod encryption_config_association;
#[path = "all/error_class_selection.rs"]
mod error_class_selection;
#[path = "all/fargate_profiles.rs"]
mod fargate_profiles;
#[path = "all/iam_authorization.rs"]
mod iam_authorization;
#[path = "all/identity_provider_config.rs"]
mod identity_provider_config;
#[path = "all/identity_provider_config_not_found.rs"]
mod identity_provider_config_not_found;
#[path = "all/identity_provider_configs.rs"]
mod identity_provider_configs;
#[path = "all/identity_provider_disassociation.rs"]
mod identity_provider_disassociation;
#[path = "all/insights_listing.rs"]
mod insights_listing;
#[path = "all/insights_refresh.rs"]
mod insights_refresh;
#[path = "all/kubernetes_api.rs"]
mod kubernetes_api;
#[path = "all/list_pod_identity_associations.rs"]
mod list_pod_identity_associations;
#[path = "all/marketplace_addon_install.rs"]
mod marketplace_addon_install;
#[path = "all/nodegroup_config_update.rs"]
mod nodegroup_config_update;
#[path = "all/nodegroup_lifecycle.rs"]
mod nodegroup_lifecycle;
#[path = "all/nodegroup_scaling_capacity_refusals.rs"]
mod nodegroup_scaling_capacity_refusals;
#[path = "all/not_found_payload.rs"]
mod not_found_payload;
#[path = "all/page_token_plane_wording.rs"]
mod page_token_plane_wording;
#[path = "all/pass_role_authorization.rs"]
mod pass_role_authorization;
#[path = "all/peer_resolution_is_grounded.rs"]
mod peer_resolution_is_grounded;
#[path = "all/pod_identity_and_rotation_refusals.rs"]
mod pod_identity_and_rotation_refusals;
#[path = "all/pod_identity_associations.rs"]
mod pod_identity_associations;
#[path = "all/refusal_contract.rs"]
mod refusal_contract;
#[path = "all/refusal_message_wording.rs"]
mod refusal_message_wording;
#[path = "all/refusal_selection.rs"]
mod refusal_selection;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/subscription_not_found_uniformity.rs"]
mod subscription_not_found_uniformity;
#[path = "all/tag_contract.rs"]
mod tag_contract;
#[path = "all/tag_map_constraint_message.rs"]
mod tag_map_constraint_message;
#[path = "all/untag_resource.rs"]
mod untag_resource;
#[path = "all/update_listing.rs"]
mod update_listing;
#[path = "all/validation_precedence.rs"]
mod validation_precedence;
#[path = "all/wire.rs"]
mod wire;
