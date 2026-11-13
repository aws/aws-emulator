// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! Put a new suite at `tests/all/<name>.rs` and declare it below, keeping the list
//! sorted.
//! A suite that is not declared here compiles nowhere and its tests never run. A
//! suite left at the top level of `tests/` fails the build.
// `tests/all.rs` is a test-target CRATE ROOT, so a bare `mod` resolves to a SIBLING
// (`tests/state_roundtrip.rs`) — which the layout check refuses. The `#[path]` is what
// points it into `tests/all/`.
#[path = "all/access_lifecycle.rs"]
mod access_lifecycle;
#[path = "all/agreement_lifecycle.rs"]
mod agreement_lifecycle;
#[path = "all/as2_profile_listing.rs"]
mod as2_profile_listing;
#[path = "all/certificate_listing.rs"]
mod certificate_listing;
#[path = "all/connector_configuration.rs"]
mod connector_configuration;
#[path = "all/connector_connection_test.rs"]
mod connector_connection_test;
#[path = "all/connector_listing.rs"]
mod connector_listing;
#[path = "all/connector_updates.rs"]
mod connector_updates;
#[path = "all/connector_transfers.rs"]
mod connector_transfers;
#[path = "all/enum_value_set_listing.rs"]
mod enum_value_set_listing;
#[path = "all/error_message_rules.rs"]
mod error_message_rules;
// `workflow_step_contract` is declared HERE rather than in its alphabetical place at the
// end of this list, for the reason `service-com-amazonaws-iotsitewise-awsiotsitewise`
// records for its own out-of-place suite: a suite this crate gains is inserted into the
// sorted list or appended after it, so those are the lines a concurrent change also
// edits, and this declaration kept landing on top of one. Fold it into the list once the
// churn settles.
#[path = "all/workflow_step_contract.rs"]
mod workflow_step_contract;
#[path = "all/feature_probe.rs"]
mod feature_probe;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/host_key_material_formats.rs"]
mod host_key_material_formats;
#[path = "all/host_key_lifecycle.rs"]
mod host_key_lifecycle;
#[path = "all/host_key_listing.rs"]
mod host_key_listing;
#[path = "all/host_keys.rs"]
mod host_keys;
#[path = "all/identity_and_authorization.rs"]
mod identity_and_authorization;
#[path = "all/identity_provider_tests.rs"]
mod identity_provider_tests;
#[path = "all/input_validation.rs"]
mod input_validation;
#[path = "all/not_found_errors.rs"]
mod not_found_errors;
#[path = "all/profile_deletion.rs"]
mod profile_deletion;
#[path = "all/profile_lifecycle.rs"]
mod profile_lifecycle;
#[path = "all/read_collection_rendering.rs"]
mod read_collection_rendering;
#[path = "all/refusal_message_rules.rs"]
mod refusal_message_rules;
#[path = "all/resource_resolution_refusals.rs"]
mod resource_resolution_refusals;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/security_policy_catalog.rs"]
mod security_policy_catalog;
#[path = "all/security_policy_listing.rs"]
mod security_policy_listing;
#[path = "all/server_config_and_state.rs"]
mod server_config_and_state;
#[path = "all/server_host_key_resource.rs"]
mod server_host_key_resource;
#[path = "all/server_logging_configuration.rs"]
mod server_logging_configuration;
#[path = "all/server_transition_windows.rs"]
mod server_transition_windows;
#[path = "all/server_user_lifecycle.rs"]
mod server_user_lifecycle;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/user_properties_and_keys.rs"]
mod user_properties_and_keys;
#[path = "all/user_state_persistence.rs"]
mod user_state_persistence;
#[path = "all/web_app_customization.rs"]
mod web_app_customization;
#[path = "all/web_app_customization_conflict_message.rs"]
mod web_app_customization_conflict_message;
#[path = "all/web_app_customization_deletion.rs"]
mod web_app_customization_deletion;
#[path = "all/web_app_deletion.rs"]
mod web_app_deletion;
#[path = "all/web_app_lifecycle.rs"]
mod web_app_lifecycle;
#[path = "all/web_app_listing.rs"]
mod web_app_listing;
#[path = "all/web_app_updates.rs"]
mod web_app_updates;
#[path = "all/workflow_custom_step_function.rs"]
mod workflow_custom_step_function;
#[path = "all/workflow_execution_listing.rs"]
mod workflow_execution_listing;
#[path = "all/workflow_lifecycle.rs"]
mod workflow_lifecycle;
#[path = "all/write_validation_and_normalization.rs"]
mod write_validation_and_normalization;
// `connector_deletion` is declared HERE rather than in its alphabetical place beside the
// other `connector_*` suites, for the reason the `workflow_step_contract` note above
// records: the sorted list is where every concurrent change inserts, so those are the
// lines a concurrent change also edits, and this declaration kept colliding with a
// neighbouring insertion. Fold it into the list once the churn settles.
#[path = "all/connector_deletion.rs"]
mod connector_deletion;
// `certificate_import` is declared HERE rather than in its alphabetical place beside
// `certificate_listing`, for the reason the two notes above record: the sorted list is
// where every concurrent change inserts. Fold it into the list once the churn settles.
#[path = "all/certificate_import.rs"]
mod certificate_import;
// `connector_remote_operations` is declared HERE rather than in its alphabetical place
// beside the other `connector_*` suites, for the reason the notes above record: the
// sorted list is where every concurrent change inserts. Fold it into the list once the
#[path = "all/connector_remote_operations.rs"]
mod connector_remote_operations;
// `connector_plane`, likewise: the provisioning update that brings a connector into
// service.
#[path = "all/connector_plane.rs"]
mod connector_plane;
// `connector_gated_workflow` is declared HERE rather than beside the other
// `connector_*` suites, for the reason the notes above record: the sorted list is where
// every concurrent change inserts. Fold it into the list once the churn settles.
#[path = "all/connector_gated_workflow.rs"]
mod connector_gated_workflow;
// `agreement_updates`, likewise: what `UpdateAgreement` persists onto a stored
// agreement, and what the read that follows it reports.
#[path = "all/agreement_updates.rs"]
mod agreement_updates;
// `end_to_end_transfer_flow`, likewise appended rather than inserted: the whole documented
// connector flow, polled the way a client polls it.
#[path = "all/end_to_end_transfer_flow.rs"]
mod end_to_end_transfer_flow;
// The four suites below are appended rather than inserted into the sorted list, for the
// reason the notes above record: the sorted list is where every concurrent change
// inserts. Fold them into the list once the churn settles.
// `api_gateway_identity_provider_addressing`: the API an API_GATEWAY provider's Url
// names, in both addressings a deployed stage is published at.
#[path = "all/api_gateway_identity_provider_addressing.rs"]
mod api_gateway_identity_provider_addressing;
// `explicit_deny_resource_clause`: which resource an explicit-deny refusal was decided
// against, and what its sentence says.
#[path = "all/explicit_deny_resource_clause.rs"]
mod explicit_deny_resource_clause;
// `ssh_public_key_body_validation`: the algorithm and the payload the service accepts in
// an SshPublicKeyBody.
#[path = "all/ssh_public_key_body_validation.rs"]
mod ssh_public_key_body_validation;
// `web_app_customization_write_guard`: one writer of a web app's customization at a time.
#[path = "all/web_app_customization_write_guard.rs"]
mod web_app_customization_write_guard;
// The five suites below are appended for the same reason: the sorted list is where every
// concurrent change inserts.
// `agreement_deletion`: what removing an AS2 agreement leaves behind.
#[path = "all/agreement_deletion.rs"]
mod agreement_deletion;
// `connector_protocol_gate`: which connector protocol each operation of the connector
// plane can be asked of.
#[path = "all/connector_protocol_gate.rs"]
mod connector_protocol_gate;
// `identity_center_dependency`: the IAM Identity Center instance a web app's application
// is registered in.
#[path = "all/identity_center_dependency.rs"]
mod identity_center_dependency;
// `service_stated_refusals`: the refusals whose whole message is the service's own fixed
// text for the condition.
#[path = "all/service_stated_refusals.rs"]
mod service_stated_refusals;
// `tag_plane_resource_kinds`: which kind of resource the tag plane reports as missing.
#[path = "all/tag_plane_resource_kinds.rs"]
mod tag_plane_resource_kinds;
// `agreement_removal_propagation`, appended for the same reason: where an agreement the
// store no longer holds has gone, on every plane that could still answer for it.
#[path = "all/agreement_removal_propagation.rs"]
mod agreement_removal_propagation;
// `host_key_scope`, appended for the same reason: WHICH keys of a server the host-key
// readers report.
#[path = "all/host_key_scope.rs"]
mod host_key_scope;
// `security_policy_publication`, likewise: the published algorithm sets and their order,
// per policy.
#[path = "all/security_policy_publication.rs"]
mod security_policy_publication;
// `host_key_private_key_containers`, likewise: the private-key containers host key
// material may arrive in.
#[path = "all/host_key_private_key_containers.rs"]
mod host_key_private_key_containers;
// `home_directory_mapping_overlap`, likewise: the one rule a logical home directory's
// `Entry` paths have to satisfy, over every operation that writes them.
#[path = "all/home_directory_mapping_overlap.rs"]
mod home_directory_mapping_overlap;
// `server_vended_log_group`, likewise: the CloudWatch Logs group a server's logging
// configuration provisions.
#[path = "all/server_vended_log_group.rs"]
mod server_vended_log_group;
// `agreement_update_write_scope`, likewise appended: WHICH agreement one UpdateAgreement
// writes, and which refusal a request that breaks more than one rule at once gets.
#[path = "all/agreement_update_write_scope.rs"]
mod agreement_update_write_scope;
