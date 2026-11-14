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
#[path = "all/abac_attribute_configuration.rs"]
mod abac_attribute_configuration;
#[path = "all/account_assignment_creation_status.rs"]
mod account_assignment_creation_status;
#[path = "all/account_assignment_creation_status_list.rs"]
mod account_assignment_creation_status_list;
#[path = "all/account_assignment_creation_status_token_spelling.rs"]
mod account_assignment_creation_status_token_spelling;
#[path = "all/account_assignment_delete.rs"]
mod account_assignment_delete;
#[path = "all/account_assignment_deletion_status.rs"]
mod account_assignment_deletion_status;
#[path = "all/account_assignment_deletion_status_list.rs"]
mod account_assignment_deletion_status_list;
#[path = "all/account_assignment_deletion_status_withdrawal.rs"]
mod account_assignment_deletion_status_withdrawal;
#[path = "all/account_assignments_for_principal.rs"]
mod account_assignments_for_principal;
#[path = "all/account_instance_region_writes.rs"]
mod account_instance_region_writes;
#[path = "all/account_instance_restriction.rs"]
mod account_instance_restriction;
#[path = "all/accounts_for_provisioned_permission_set.rs"]
mod accounts_for_provisioned_permission_set;
#[path = "all/application_access_scope_delete.rs"]
mod application_access_scope_delete;
#[path = "all/application_access_scope_limits.rs"]
mod application_access_scope_limits;
#[path = "all/application_access_scopes.rs"]
mod application_access_scopes;
#[path = "all/application_assignment_configuration.rs"]
mod application_assignment_configuration;
#[path = "all/application_assignment_conflict.rs"]
mod application_assignment_conflict;
#[path = "all/application_assignment_delete.rs"]
mod application_assignment_delete;
#[path = "all/application_assignment_describe.rs"]
mod application_assignment_describe;
#[path = "all/application_assignment_repeat_grant.rs"]
mod application_assignment_repeat_grant;
#[path = "all/application_assignments.rs"]
mod application_assignments;
#[path = "all/application_assignments_for_principal.rs"]
mod application_assignments_for_principal;
#[path = "all/application_assignments_list.rs"]
mod application_assignments_list;
#[path = "all/application_authentication_methods.rs"]
mod application_authentication_methods;
#[path = "all/application_authentication_methods_list.rs"]
mod application_authentication_methods_list;
#[path = "all/application_create.rs"]
mod application_create;
#[path = "all/application_create_providers.rs"]
mod application_create_providers;
#[path = "all/application_delete.rs"]
mod application_delete;
#[path = "all/application_describe.rs"]
mod application_describe;
#[path = "all/application_grant_independence.rs"]
mod application_grant_independence;
#[path = "all/application_grants.rs"]
mod application_grants;
#[path = "all/application_grants_list.rs"]
mod application_grants_list;
#[path = "all/application_list.rs"]
mod application_list;
#[path = "all/application_provider_describe.rs"]
mod application_provider_describe;
#[path = "all/application_provider_display_data.rs"]
mod application_provider_display_data;
#[path = "all/application_provider_resolution.rs"]
mod application_provider_resolution;
#[path = "all/application_refusal_wording.rs"]
mod application_refusal_wording;
#[path = "all/application_providers.rs"]
mod application_providers;
#[path = "all/application_session_configuration.rs"]
mod application_session_configuration;
#[path = "all/application_update.rs"]
mod application_update;
#[path = "all/client_token_conflict_message.rs"]
mod client_token_conflict_message;
#[path = "all/create_tag_fingerprint.rs"]
mod create_tag_fingerprint;
#[path = "all/created_date_resolution_instant.rs"]
mod created_date_resolution_instant;
#[path = "all/customer_managed_policy_reference_independence.rs"]
mod customer_managed_policy_reference_independence;
#[path = "all/customer_managed_policy_reference_order.rs"]
mod customer_managed_policy_reference_order;
#[path = "all/customer_managed_policy_references.rs"]
mod customer_managed_policy_references;
#[path = "all/discovered_instance_surface.rs"]
mod discovered_instance_surface;
#[path = "all/enumeration_default_page_size.rs"]
mod enumeration_default_page_size;
#[path = "all/inline_policy.rs"]
mod inline_policy;
#[path = "all/inline_policy_delete.rs"]
mod inline_policy_delete;
#[path = "all/inline_policy_independence.rs"]
mod inline_policy_independence;
#[path = "all/instance_create.rs"]
mod instance_create;
#[path = "all/instance_create_client_token_fold.rs"]
mod instance_create_client_token_fold;
#[path = "all/instance_create_in_progress_reads.rs"]
mod instance_create_in_progress_reads;
#[path = "all/instance_create_in_progress_writes.rs"]
mod instance_create_in_progress_writes;
#[path = "all/instance_delete.rs"]
mod instance_delete;
#[path = "all/instance_describe.rs"]
mod instance_describe;
#[path = "all/instance_encryption_default.rs"]
mod instance_encryption_default;
#[path = "all/instance_envelope_view.rs"]
mod instance_envelope_view;
#[path = "all/instance_region_addition.rs"]
mod instance_region_addition;
#[path = "all/instance_region_removal.rs"]
mod instance_region_removal;
#[path = "all/instance_region_status.rs"]
mod instance_region_status;
#[path = "all/instance_regions.rs"]
mod instance_regions;
#[path = "all/instance_single_instant_read.rs"]
mod instance_single_instant_read;
#[path = "all/instance_state_view.rs"]
mod instance_state_view;
#[path = "all/instance_update.rs"]
mod instance_update;
#[path = "all/instance_write_stamp_instant.rs"]
mod instance_write_stamp_instant;
#[path = "all/list_application_access_scopes.rs"]
mod list_application_access_scopes;
#[path = "all/list_instances_pagination.rs"]
mod list_instances_pagination;
#[path = "all/listed_instance_projection_matches_describe.rs"]
mod listed_instance_projection_matches_describe;
#[path = "all/listed_instance_state_across_read_edge_cases.rs"]
mod listed_instance_state_across_read_edge_cases;
#[path = "all/listed_instance_state_after_instance_writes.rs"]
mod listed_instance_state_after_instance_writes;
#[path = "all/listed_instance_state_by_enablement_path.rs"]
mod listed_instance_state_by_enablement_path;
#[path = "all/listed_instance_state_by_region_count.rs"]
mod listed_instance_state_by_region_count;
#[path = "all/listed_instance_state_every_entry.rs"]
mod listed_instance_state_every_entry;
#[path = "all/listed_instance_state_generated_accounts.rs"]
mod listed_instance_state_generated_accounts;
#[path = "all/listed_instance_state_at_the_list_bound.rs"]
mod listed_instance_state_at_the_list_bound;
#[path = "all/listed_instance_state_source.rs"]
mod listed_instance_state_source;
#[path = "all/listed_instance_state_through_a_call_sequence.rs"]
mod listed_instance_state_through_a_call_sequence;
#[path = "all/listed_instance_state_under_any_name.rs"]
mod listed_instance_state_under_any_name;
#[path = "all/listed_instance_state_without_a_name.rs"]
mod listed_instance_state_without_a_name;
#[path = "all/managed_policies.rs"]
mod managed_policies;
#[path = "all/managed_policies_in_permission_set.rs"]
mod managed_policies_in_permission_set;
#[path = "all/managed_policy_attach_independence.rs"]
mod managed_policy_attach_independence;
#[path = "all/page_token_canonical_spelling.rs"]
mod page_token_canonical_spelling;
#[path = "all/permission_set_delete_owed_provisioning.rs"]
mod permission_set_delete_owed_provisioning;
#[path = "all/permission_set_provisioning.rs"]
mod permission_set_provisioning;
#[path = "all/permission_set_provisioning_status_list.rs"]
mod permission_set_provisioning_status_list;
#[path = "all/permission_set_provisioning_status_poll.rs"]
mod permission_set_provisioning_status_poll;
#[path = "all/permission_set_provisioning_targets.rs"]
mod permission_set_provisioning_targets;
#[path = "all/permission_set_surface_resolution.rs"]
mod permission_set_surface_resolution;
#[path = "all/permission_set_update.rs"]
mod permission_set_update;
#[path = "all/permission_sets_provisioned_to_account.rs"]
mod permission_sets_provisioned_to_account;
#[path = "all/permissions_boundary.rs"]
mod permissions_boundary;
#[path = "all/permissions_boundary_choice_refusal.rs"]
mod permissions_boundary_choice_refusal;
#[path = "all/permissions_boundary_delete.rs"]
mod permissions_boundary_delete;
#[path = "all/permissions_boundary_get.rs"]
mod permissions_boundary_get;
#[path = "all/policy_attach_conflict.rs"]
mod policy_attach_conflict;
#[path = "all/provisioned_copy_projections.rs"]
mod provisioned_copy_projections;
#[path = "all/provisioning_capture_order.rs"]
mod provisioning_capture_order;
#[path = "all/region_describe.rs"]
mod region_describe;
#[path = "all/region_list.rs"]
mod region_list;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/resource_untagging.rs"]
mod resource_untagging;
#[path = "all/singleton_child_delete.rs"]
mod singleton_child_delete;
#[path = "all/ssoadmin_permission_sets.rs"]
mod ssoadmin_permission_sets;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/status_filter_enum_membership.rs"]
mod status_filter_enum_membership;
#[path = "all/system_tag_mutations.rs"]
mod system_tag_mutations;
#[path = "all/tag_resource_resolution.rs"]
mod tag_resource_resolution;
#[path = "all/tags_from_creates.rs"]
mod tags_from_creates;
#[path = "all/test_target_manifest.rs"]
mod test_target_manifest;
#[path = "all/trusted_token_issuer_list.rs"]
mod trusted_token_issuer_list;
#[path = "all/trusted_token_issuer_update.rs"]
mod trusted_token_issuer_update;
#[path = "all/trusted_token_issuers.rs"]
mod trusted_token_issuers;
#[path = "all/unknown_application_provider_message.rs"]
mod unknown_application_provider_message;
#[path = "all/unknown_instance_channel.rs"]
mod unknown_instance_channel;
#[path = "all/unknown_instance_message.rs"]
mod unknown_instance_message;
#[path = "all/validation_message_wording.rs"]
mod validation_message_wording;
#[path = "all/withdrawn_assignment_copy_in_place.rs"]
mod withdrawn_assignment_copy_in_place;
// Every declaration stands in the one run above, beside its own name: that is what keeps a
// suite about to be declared twice visible as a repeated name, since a second `mod` item
// for a file already declared is a redefinition that stops this whole target compiling. A
// new suite goes into that run, never into a tail at the end of this file.
