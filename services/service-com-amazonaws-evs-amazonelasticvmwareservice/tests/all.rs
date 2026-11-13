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
#[path = "all/authorization.rs"]
mod authorization;
#[path = "all/check_impairment_onset.rs"]
mod check_impairment_onset;
#[path = "all/child_record_create_stamp.rs"]
mod child_record_create_stamp;
#[path = "all/connector_appliance_fqdn_case.rs"]
mod connector_appliance_fqdn_case;
#[path = "all/connector_create_while_deleting.rs"]
mod connector_create_while_deleting;
#[path = "all/connector_credential_secret.rs"]
mod connector_credential_secret;
#[path = "all/connector_deletion.rs"]
mod connector_deletion;
#[path = "all/connector_health_checks.rs"]
mod connector_health_checks;
#[path = "all/connector_secret_arn.rs"]
mod connector_secret_arn;
#[path = "all/connector_update_while_deleting.rs"]
mod connector_update_while_deleting;
#[path = "all/connectors.rs"]
mod connectors;
// SUPERSEDED and empty: mainline's `connectors.rs` replaced its connector half and
// `hosts.rs` its host half. Declared only because the layout check requires every file
// under tests/all/ to be declared, and this agent's tools cannot delete a file.
#[path = "all/connectors_and_hosts.rs"]
mod connectors_and_hosts;
#[path = "all/create_environment_network_membership.rs"]
mod create_environment_network_membership;
#[path = "all/create_environment_validation.rs"]
mod create_environment_validation;
#[path = "all/created_environment_hosts.rs"]
mod created_environment_hosts;
#[path = "all/delete_entitlement.rs"]
mod delete_entitlement;
#[path = "all/depot_url.rs"]
mod depot_url;
#[path = "all/deprecated_tagging_errors.rs"]
mod deprecated_tagging_errors;
#[path = "all/eip_allocation_existence.rs"]
mod eip_allocation_existence;
#[path = "all/eip_association_existence.rs"]
mod eip_association_existence;
#[path = "all/eip_ec2_propagation.rs"]
mod eip_ec2_propagation;
#[path = "all/eip_release_on_environment_deletion.rs"]
mod eip_release_on_environment_deletion;
#[path = "all/eip_vlan_gate.rs"]
mod eip_vlan_gate;
#[path = "all/entitlement_connector.rs"]
mod entitlement_connector;
#[path = "all/entitlement_stop_stamp.rs"]
mod entitlement_stop_stamp;
#[path = "all/entitlement_vm_id_identity.rs"]
mod entitlement_vm_id_identity;
#[path = "all/entitlements.rs"]
mod entitlements;
#[path = "all/environment_addressing_rule.rs"]
mod environment_addressing_rule;
#[path = "all/environment_check_input_recompute.rs"]
mod environment_check_input_recompute;
#[path = "all/environment_child_change_stamps.rs"]
mod environment_child_change_stamps;
#[path = "all/environment_connector_health.rs"]
mod environment_connector_health;
#[path = "all/environment_deletion_cascade.rs"]
mod environment_deletion_cascade;
#[path = "all/environment_deletion_hosts.rs"]
mod environment_deletion_hosts;
#[path = "all/environment_health_checks.rs"]
mod environment_health_checks;
#[path = "all/environment_lifecycle.rs"]
mod environment_lifecycle;
#[path = "all/error_wire_status.rs"]
mod error_wire_status;
#[path = "all/esx_build_catalogue.rs"]
mod esx_build_catalogue;
#[path = "all/finished_record_retention.rs"]
mod finished_record_retention;
#[path = "all/hcx_eip_associations.rs"]
mod hcx_eip_associations;
#[path = "all/hcx_network_acl_references.rs"]
mod hcx_network_acl_references;
#[path = "all/host_deletion.rs"]
mod host_deletion;
#[path = "all/host_ec2_instances.rs"]
mod host_ec2_instances;
#[path = "all/host_name_case.rs"]
mod host_name_case;
#[path = "all/host_placement_references.rs"]
mod host_placement_references;
#[path = "all/hosts.rs"]
mod hosts;
#[path = "all/identity_policy_sources.rs"]
mod identity_policy_sources;
#[path = "all/list_vm_entitlements_connector.rs"]
mod list_vm_entitlements_connector;
#[path = "all/list_while_initializing.rs"]
mod list_while_initializing;
#[path = "all/page_size_bound.rs"]
mod page_size_bound;
#[path = "all/page_token_binding.rs"]
mod page_token_binding;
#[path = "all/premium_support_entitlement.rs"]
mod premium_support_entitlement;
#[path = "all/public_vlan_netmask.rs"]
mod public_vlan_netmask;
#[path = "all/put_account_settings_due_transitions.rs"]
mod put_account_settings_due_transitions;
#[path = "all/record_stamp_backwards_guard.rs"]
mod record_stamp_backwards_guard;
#[path = "all/record_stamp_monotonicity.rs"]
mod record_stamp_monotonicity;
#[path = "all/refusal_wording.rs"]
mod refusal_wording;
#[path = "all/reserved_tag_keys.rs"]
mod reserved_tag_keys;
#[path = "all/route_server_peer_references.rs"]
mod route_server_peer_references;
#[path = "all/self_deployed_license_info_inertness.rs"]
mod self_deployed_license_info_inertness;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_resource_idempotent_stamp.rs"]
mod tag_resource_idempotent_stamp;
#[path = "all/tagging_not_found.rs"]
mod tagging_not_found;
#[path = "all/tagging_resource_arn_resolution.rs"]
mod tagging_resource_arn_resolution;
#[path = "all/validation_message_rendering.rs"]
mod validation_message_rendering;
#[path = "all/validation_reason_completeness.rs"]
mod validation_reason_completeness;
#[path = "all/vcf_hostname_uniqueness.rs"]
mod vcf_hostname_uniqueness;
#[path = "all/versions_catalog.rs"]
mod versions_catalog;
#[path = "all/vlan_cidr_reading.rs"]
mod vlan_cidr_reading;
#[path = "all/vlan_subnet_propagation.rs"]
mod vlan_subnet_propagation;
