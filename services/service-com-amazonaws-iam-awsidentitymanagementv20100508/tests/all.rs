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
#[path = "all/access_denied_message_wording.rs"]
mod access_denied_message_wording;
#[path = "all/access_key_deletion.rs"]
mod access_key_deletion;
#[path = "all/access_key_last_used.rs"]
mod access_key_last_used;
#[path = "all/access_key_listing.rs"]
mod access_key_listing;
#[path = "all/access_key_status.rs"]
mod access_key_status;
#[path = "all/access_key_status_authentication.rs"]
mod access_key_status_authentication;
#[path = "all/access_keys.rs"]
mod access_keys;
#[path = "all/account_aliases.rs"]
mod account_aliases;
#[path = "all/account_authorization_details.rs"]
mod account_authorization_details;
#[path = "all/account_password_policy.rs"]
mod account_password_policy;
#[path = "all/account_properties.rs"]
mod account_properties;
#[path = "all/account_singleton_authorization.rs"]
mod account_singleton_authorization;
#[path = "all/authentication_refusal_wording.rs"]
mod authentication_refusal_wording;
#[path = "all/aws_managed_policy_arn_resolution.rs"]
mod aws_managed_policy_arn_resolution;
#[path = "all/aws_managed_policy_attachability.rs"]
mod aws_managed_policy_attachability;
#[path = "all/aws_managed_policy_catalog.rs"]
mod aws_managed_policy_catalog;
#[path = "all/aws_reference_catalog_metadata.rs"]
mod aws_reference_catalog_metadata;
#[path = "all/boundary_and_provider_writes.rs"]
mod boundary_and_provider_writes;
#[path = "all/boundary_refusal_message_wording.rs"]
mod boundary_refusal_message_wording;
#[path = "all/caller_identity_resolution.rs"]
mod caller_identity_resolution;
#[path = "all/change_password.rs"]
mod change_password;
#[path = "all/composed_condition_context.rs"]
mod composed_condition_context;
#[path = "all/conflict_and_certificate_refusal_wording.rs"]
mod conflict_and_certificate_refusal_wording;
#[path = "all/credential_report_generation.rs"]
mod credential_report_generation;
#[path = "all/credential_report_reads.rs"]
mod credential_report_reads;
#[path = "all/custom_policy_context_keys.rs"]
mod custom_policy_context_keys;
#[path = "all/custom_policy_simulation.rs"]
mod custom_policy_simulation;
#[path = "all/delegation_request_acceptance.rs"]
mod delegation_request_acceptance;
#[path = "all/delegation_request_association.rs"]
mod delegation_request_association;
#[path = "all/delegation_request_error_envelope.rs"]
mod delegation_request_error_envelope;
#[path = "all/delegation_request_filing.rs"]
mod delegation_request_filing;
#[path = "all/delegation_request_listing.rs"]
mod delegation_request_listing;
#[path = "all/delegation_request_reads.rs"]
mod delegation_request_reads;
#[path = "all/delegation_request_rejection.rs"]
mod delegation_request_rejection;
#[path = "all/delegation_request_updates.rs"]
mod delegation_request_updates;
#[path = "all/entities_for_policy.rs"]
mod entities_for_policy;
#[path = "all/entity_name_path_prefix_refusal.rs"]
mod entity_name_path_prefix_refusal;
#[path = "all/entity_reference_and_namespace_refusals.rs"]
mod entity_reference_and_namespace_refusals;
#[path = "all/enum_refusal_wording.rs"]
mod enum_refusal_wording;
#[path = "all/error_envelope_request_id.rs"]
mod error_envelope_request_id;
#[path = "all/error_request_id_across_operations.rs"]
mod error_request_id_across_operations;
#[path = "all/fixed_refusal_wording.rs"]
mod fixed_refusal_wording;
#[path = "all/group_attached_policies.rs"]
mod group_attached_policies;
#[path = "all/group_attached_policy_listing.rs"]
mod group_attached_policy_listing;
#[path = "all/group_attached_policy_removal.rs"]
mod group_attached_policy_removal;
#[path = "all/group_creation.rs"]
mod group_creation;
#[path = "all/group_deletion.rs"]
mod group_deletion;
#[path = "all/group_inherited_authorization.rs"]
mod group_inherited_authorization;
#[path = "all/group_inline_policies.rs"]
mod group_inline_policies;
#[path = "all/group_inline_policy_removal.rs"]
mod group_inline_policy_removal;
#[path = "all/group_listing.rs"]
mod group_listing;
#[path = "all/group_membership.rs"]
mod group_membership;
#[path = "all/group_membership_removal.rs"]
mod group_membership_removal;
#[path = "all/group_membership_writes.rs"]
mod group_membership_writes;
#[path = "all/group_policy_listing.rs"]
mod group_policy_listing;
#[path = "all/group_renames.rs"]
mod group_renames;
#[path = "all/human_readable_summaries.rs"]
mod human_readable_summaries;
#[path = "all/identity_policy_enforcement.rs"]
mod identity_policy_enforcement;
#[path = "all/implicit_user_and_credential_pair_refusals.rs"]
mod implicit_user_and_credential_pair_refusals;
#[path = "all/inline_policy_size_quota.rs"]
mod inline_policy_size_quota;
#[path = "all/instance_profile_listing.rs"]
mod instance_profile_listing;
#[path = "all/instance_profile_role_binding.rs"]
mod instance_profile_role_binding;
#[path = "all/instance_profile_role_removal.rs"]
mod instance_profile_role_removal;
#[path = "all/instance_profile_roles.rs"]
mod instance_profile_roles;
#[path = "all/instance_profile_tag_listing.rs"]
mod instance_profile_tag_listing;
#[path = "all/instance_profile_tag_removal.rs"]
mod instance_profile_tag_removal;
#[path = "all/instance_profile_tags.rs"]
mod instance_profile_tags;
#[path = "all/instance_profiles.rs"]
mod instance_profiles;
#[path = "all/instance_profiles_for_role.rs"]
mod instance_profiles_for_role;
#[path = "all/login_profile_absence_wording.rs"]
mod login_profile_absence_wording;
#[path = "all/login_profile_deletion.rs"]
mod login_profile_deletion;
#[path = "all/login_profile_updates.rs"]
mod login_profile_updates;
#[path = "all/login_profiles.rs"]
mod login_profiles;
#[path = "all/managed_policies.rs"]
mod managed_policies;
#[path = "all/managed_policy_statement_grammar.rs"]
mod managed_policy_statement_grammar;
#[path = "all/management_event_recording.rs"]
mod management_event_recording;
#[path = "all/member_format_refusal_wording.rs"]
mod member_format_refusal_wording;
#[path = "all/mfa_device_reads.rs"]
mod mfa_device_reads;
#[path = "all/missing_entity_message_wording.rs"]
mod missing_entity_message_wording;
#[path = "all/nested_member_clause_wording.rs"]
mod nested_member_clause_wording;
#[path = "all/net_new_refusal_wording.rs"]
mod net_new_refusal_wording;
#[path = "all/nested_pattern_clause_wording.rs"]
mod nested_pattern_clause_wording;
#[path = "all/network_condition_authorization.rs"]
mod network_condition_authorization;
#[path = "all/open_id_connect_client_id_registration.rs"]
mod open_id_connect_client_id_registration;
#[path = "all/open_id_connect_client_ids.rs"]
mod open_id_connect_client_ids;
#[path = "all/open_id_connect_provider_absence_wording.rs"]
mod open_id_connect_provider_absence_wording;
#[path = "all/open_id_connect_provider_deletion.rs"]
mod open_id_connect_provider_deletion;
#[path = "all/open_id_connect_provider_listing.rs"]
mod open_id_connect_provider_listing;
#[path = "all/open_id_connect_provider_tag_absence_wording.rs"]
mod open_id_connect_provider_tag_absence_wording;
#[path = "all/open_id_connect_provider_tag_listing.rs"]
mod open_id_connect_provider_tag_listing;
#[path = "all/open_id_connect_provider_tag_removal.rs"]
mod open_id_connect_provider_tag_removal;
#[path = "all/open_id_connect_provider_tag_updates.rs"]
mod open_id_connect_provider_tag_updates;
#[path = "all/open_id_connect_provider_thumbprint_updates.rs"]
mod open_id_connect_provider_thumbprint_updates;
#[path = "all/open_id_connect_providers.rs"]
mod open_id_connect_providers;
#[path = "all/organization_root_access_features.rs"]
mod organization_root_access_features;
#[path = "all/organizations_access_report_generation.rs"]
mod organizations_access_report_generation;
#[path = "all/outbound_web_identity_federation.rs"]
mod outbound_web_identity_federation;
#[path = "all/own_api_authorization_coverage.rs"]
mod own_api_authorization_coverage;
#[path = "all/permissions_boundary_ceiling.rs"]
mod permissions_boundary_ceiling;
#[path = "all/policies_granting_service_access.rs"]
mod policies_granting_service_access;
#[path = "all/policy_arn_absence_wording.rs"]
mod policy_arn_absence_wording;
#[path = "all/policy_arn_condition_context.rs"]
mod policy_arn_condition_context;
#[path = "all/policy_default_version.rs"]
mod policy_default_version;
#[path = "all/policy_deletion.rs"]
mod policy_deletion;
#[path = "all/policy_document_encoding.rs"]
mod policy_document_encoding;
#[path = "all/policy_input_list_refusal_wording.rs"]
mod policy_input_list_refusal_wording;
#[path = "all/policy_language_values.rs"]
mod policy_language_values;
#[path = "all/policy_listing.rs"]
mod policy_listing;
#[path = "all/policy_refusal_wording.rs"]
mod policy_refusal_wording;
#[path = "all/policy_simulation.rs"]
mod policy_simulation;
#[path = "all/policy_tag_authorization.rs"]
mod policy_tag_authorization;
#[path = "all/policy_tag_listing.rs"]
mod policy_tag_listing;
#[path = "all/policy_tag_removal.rs"]
mod policy_tag_removal;
#[path = "all/policy_tags.rs"]
mod policy_tags;
#[path = "all/policy_variable_scoping.rs"]
mod policy_variable_scoping;
#[path = "all/policy_version_creation.rs"]
mod policy_version_creation;
#[path = "all/policy_version_creation_absence_wording.rs"]
mod policy_version_creation_absence_wording;
#[path = "all/policy_version_deletion.rs"]
mod policy_version_deletion;
#[path = "all/policy_version_listing.rs"]
mod policy_version_listing;
#[path = "all/principal_policy_context_keys.rs"]
mod principal_policy_context_keys;
#[path = "all/protected_role_refusals.rs"]
mod protected_role_refusals;
#[path = "all/published_managed_policy_reads.rs"]
mod published_managed_policy_reads;
#[path = "all/published_policy_families.rs"]
mod published_policy_families;
#[path = "all/published_role_template_acquisition.rs"]
mod published_role_template_acquisition;
#[path = "all/range_refusal_wording.rs"]
mod range_refusal_wording;
#[path = "all/refused_call_audit_parameters.rs"]
mod refused_call_audit_parameters;
#[path = "all/request_condition_context.rs"]
mod request_condition_context;
#[path = "all/reserved_tag_key_wording.rs"]
mod reserved_tag_key_wording;
#[path = "all/resource_tag_condition_context.rs"]
mod resource_tag_condition_context;
#[path = "all/role_attached_policies.rs"]
mod role_attached_policies;
#[path = "all/role_boundary_removal.rs"]
mod role_boundary_removal;
#[path = "all/role_deletion.rs"]
mod role_deletion;
#[path = "all/role_description.rs"]
mod role_description;
#[path = "all/role_inline_policies.rs"]
mod role_inline_policies;
#[path = "all/role_inline_policy_rotation.rs"]
mod role_inline_policy_rotation;
#[path = "all/role_lifecycle.rs"]
mod role_lifecycle;
#[path = "all/role_listing.rs"]
mod role_listing;
#[path = "all/role_permissions_boundary.rs"]
mod role_permissions_boundary;
#[path = "all/role_policy_listing.rs"]
mod role_policy_listing;
#[path = "all/role_policy_removal.rs"]
mod role_policy_removal;
#[path = "all/role_protection_and_refusal_messages.rs"]
mod role_protection_and_refusal_messages;
#[path = "all/role_settings.rs"]
mod role_settings;
#[path = "all/role_tag_listing.rs"]
mod role_tag_listing;
#[path = "all/role_tag_removal.rs"]
mod role_tag_removal;
#[path = "all/role_tags.rs"]
mod role_tags;
#[path = "all/role_template_acquisition.rs"]
mod role_template_acquisition;
#[path = "all/role_template_version_reads.rs"]
mod role_template_version_reads;
#[path = "all/role_trust_policy.rs"]
mod role_trust_policy;
#[path = "all/role_trust_policy_account_principals.rs"]
mod role_trust_policy_account_principals;
#[path = "all/saml_provider_deletion.rs"]
mod saml_provider_deletion;
#[path = "all/saml_provider_listing.rs"]
mod saml_provider_listing;
#[path = "all/saml_provider_tag_updates.rs"]
mod saml_provider_tag_updates;
#[path = "all/saml_provider_tags.rs"]
mod saml_provider_tags;
#[path = "all/saml_provider_updates.rs"]
mod saml_provider_updates;
#[path = "all/saml_providers.rs"]
mod saml_providers;
#[path = "all/server_certificate_deletion.rs"]
mod server_certificate_deletion;
#[path = "all/server_certificate_listing.rs"]
mod server_certificate_listing;
#[path = "all/server_certificate_renames.rs"]
mod server_certificate_renames;
#[path = "all/server_certificate_tag_listing.rs"]
mod server_certificate_tag_listing;
#[path = "all/server_certificate_tag_removal.rs"]
mod server_certificate_tag_removal;
#[path = "all/server_certificate_tag_updates.rs"]
mod server_certificate_tag_updates;
#[path = "all/server_certificate_untag_input_bounds.rs"]
mod server_certificate_untag_input_bounds;
#[path = "all/server_certificate_untag_listing.rs"]
mod server_certificate_untag_listing;
#[path = "all/server_certificates.rs"]
mod server_certificates;
#[path = "all/service_last_accessed_details.rs"]
mod service_last_accessed_details;
#[path = "all/service_linked_role_deletion.rs"]
mod service_linked_role_deletion;
#[path = "all/service_linked_role_published_names.rs"]
mod service_linked_role_published_names;
#[path = "all/service_linked_role_removal.rs"]
mod service_linked_role_removal;
#[path = "all/service_linked_roles.rs"]
mod service_linked_roles;
#[path = "all/service_provisioned_roles.rs"]
mod service_provisioned_roles;
#[path = "all/service_specific_credential_deletion.rs"]
mod service_specific_credential_deletion;
#[path = "all/service_specific_credential_reset.rs"]
mod service_specific_credential_reset;
#[path = "all/service_specific_credential_status.rs"]
mod service_specific_credential_status;
#[path = "all/service_specific_credentials.rs"]
mod service_specific_credentials;
#[path = "all/signing_certificates.rs"]
mod signing_certificates;
#[path = "all/ssh_public_key_fingerprints.rs"]
mod ssh_public_key_fingerprints;
#[path = "all/ssh_public_key_reads.rs"]
mod ssh_public_key_reads;
#[path = "all/ssh_public_keys.rs"]
mod ssh_public_keys;
#[path = "all/simulated_address_conditions.rs"]
mod simulated_address_conditions;
#[path = "all/simulated_decision_attribution.rs"]
mod simulated_decision_attribution;
#[path = "all/simulated_policy_composition.rs"]
mod simulated_policy_composition;
#[path = "all/simulated_principal_sources.rs"]
mod simulated_principal_sources;
#[path = "all/simulated_resource_names.rs"]
mod simulated_resource_names;
#[path = "all/simulated_resource_policy.rs"]
mod simulated_resource_policy;
#[path = "all/simulated_resource_policy_refusal.rs"]
mod simulated_resource_policy_refusal;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/unparseable_arn_refusal_wording.rs"]
mod unparseable_arn_refusal_wording;
#[path = "all/user_attached_policies.rs"]
mod user_attached_policies;
#[path = "all/user_attached_policy_listing.rs"]
mod user_attached_policy_listing;
#[path = "all/user_boundary_removal.rs"]
mod user_boundary_removal;
#[path = "all/user_group_listing.rs"]
mod user_group_listing;
#[path = "all/user_lifecycle.rs"]
mod user_lifecycle;
#[path = "all/user_permissions_boundary.rs"]
mod user_permissions_boundary;
#[path = "all/user_policy_listing.rs"]
mod user_policy_listing;
#[path = "all/user_record_authorization.rs"]
mod user_record_authorization;
#[path = "all/user_renames.rs"]
mod user_renames;
#[path = "all/user_tag_listing.rs"]
mod user_tag_listing;
#[path = "all/user_tag_removal.rs"]
mod user_tag_removal;
#[path = "all/user_tags.rs"]
mod user_tags;
#[path = "all/validation_error_clauses.rs"]
mod validation_error_clauses;
#[path = "all/virtual_mfa_device_enablement.rs"]
mod virtual_mfa_device_enablement;
#[path = "all/virtual_mfa_devices.rs"]
mod virtual_mfa_devices;
