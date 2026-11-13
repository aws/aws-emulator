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
//
// Declared here, AHEAD of the sorted list, on purpose: every fms fix that lands inserts
// its suite into that list, and a suite whose name sorts near the end of it collides with
// every other late-sorting suite landing in the same window — this fix lost several
// serialized squash merges to exactly that. Declaring it outside the contended region
// keeps it out of their way; the suite itself is an ordinary `tests/all/<name>.rs` file.
// Fold it into the sorted list whenever that list is next touched for another reason.
#[path = "all/third_party_firewall_firewall_policies.rs"]
mod third_party_firewall_firewall_policies;

// Declared here too, outside the sorted list below, for the reason stated above:
// inserting a `mod` into that list collides with every other fms suite landing
// in the same window. The suite itself is an ordinary `tests/all/<name>.rs` file.
#[path = "all/administrator_slot_not_found.rs"]
mod administrator_slot_not_found;

// Declared here too, outside the sorted list below, for the reason stated above:
// inserting a `mod` into that list collides with every other fms suite landing
// in the same window, and this suite's name sorts right into the stretch of it
// the late-sorting suites keep landing in. The suite itself is an ordinary
// `tests/all/<name>.rs` file.
#[path = "all/resource_set_in_use_by_policies.rs"]
mod resource_set_in_use_by_policies;

// Declared here too, outside the sorted list below, for the reason stated above:
// inserting a `mod` into that list collides with every other fms suite landing
// in the same window. The suite itself is an ordinary `tests/all/<name>.rs` file.
#[path = "all/account_page_token_scoping.rs"]
mod account_page_token_scoping;

// Declared beside it, for the same reason: this suite's name sorts right into the
// contended region of the list below. The suite itself is an ordinary
// `tests/all/<name>.rs` file.
#[path = "all/protection_status_policy_scope_page.rs"]
mod protection_status_policy_scope_page;

// Declared beside it, for the same reason: this suite's name sorts right into
// the contended region of the list below. The suite itself is an ordinary
// `tests/all/<name>.rs` file.
#[path = "all/narrowed_page_token_scoping.rs"]
mod narrowed_page_token_scoping;

// Declared beside it, and for the same reason: this suite's name sorts right
// into the contended region of the list below. The suite itself is an ordinary
// `tests/all/<name>.rs` file.
#[path = "all/protection_status_page_token_scoping.rs"]
mod protection_status_page_token_scoping;

// Declared beside it, and for the same reason: the sorted list below is the
// contended region every late-sorting fms suite lands in.
#[path = "all/third_party_firewall_association.rs"]
mod third_party_firewall_association;

// Declared beside them, for the same reason: this suite's name sorts into the
// contended region of the list below.
#[path = "all/tag_index_single_writer.rs"]
mod tag_index_single_writer;

// Declared beside them, for the same reason: this suite's name sorts past the
// end of the contended region of the list below.
#[path = "all/violation_details.rs"]
mod violation_details;

// Declared beside them, for the same reason: this suite's name sorts into the
// contended region of the list below.
#[path = "all/admin_disassociation_first_in_last_out.rs"]
mod admin_disassociation_first_in_last_out;

// Declared beside them, for the same reason: inserting a `mod` into the sorted
// list below conflicts with every other fms suite landing in the same window, and
// this suite's name sorts right into the stretch of that list the late-sorting
// suites keep landing in. The suite itself is an ordinary `tests/all/<name>.rs`.
#[path = "all/resource_set_mutation_admin_scope.rs"]
mod resource_set_mutation_admin_scope;

// Declared beside them, and for the same reason: the sorted list below is the
// contended region every fms suite landing in the same window inserts into, and
// this suite's name sorts right into it. The suite itself is an ordinary
// `tests/all/<name>.rs` file.
#[path = "all/resource_set_membership_last_update_time.rs"]
mod resource_set_membership_last_update_time;

// Declared beside them, for the same reason: this suite's name sorts right into
// the contended region of the list below. The suite itself is an ordinary
// `tests/all/<name>.rs` file.
#[path = "all/resource_set_disassociation_membership_invariants.rs"]
mod resource_set_disassociation_membership_invariants;

// Declared beside them, for the same reason: this suite's name sorts right into
// the contended region of the list below. The suite itself is an ordinary
// `tests/all/<name>.rs` file.
#[path = "all/pagination_token_decoding.rs"]
mod pagination_token_decoding;

// Declared beside them, for the same reason: this suite's name sorts right into
// the contended region of the list below. The suite itself is an ordinary
// `tests/all/<name>.rs` file.
#[path = "all/previous_list_version_archive.rs"]
mod previous_list_version_archive;

// Declared beside them, for the same reason: this suite's name sorts right into
// the contended region of the list below. The suite itself is an ordinary
// `tests/all/<name>.rs` file.
#[path = "all/pending_transition_settling.rs"]
mod pending_transition_settling;

// Declared beside them, for the same reason: this suite's name sorts right into
// the contended region of the list below. The suite itself is an ordinary
// `tests/all/<name>.rs` file.
#[path = "all/policy_document_defaults.rs"]
mod policy_document_defaults;

// Declared beside them, for the same reason: inserting a `mod` into the sorted
// list below conflicts with every other fms suite landing in the same window,
// and this suite's name sorts right into the stretch of that list the
// late-sorting suites keep landing in. The suite itself is an ordinary
// `tests/all/<name>.rs` file.
#[path = "all/member_account_policy_scope.rs"]
mod member_account_policy_scope;

// Declared beside them, for the same reason: this suite's name sorts right into
// the contended region of the list below. The suite itself is an ordinary
// `tests/all/<name>.rs` file.
#[path = "all/administrator_gate_clock_reading.rs"]
mod administrator_gate_clock_reading;

// Declared beside them, for the same reason: this suite's name sorts right into
// the contended region of the list below. The suite itself is an ordinary
// `tests/all/<name>.rs` file.
#[path = "all/administrator_offboarding.rs"]
mod administrator_offboarding;

// Declared beside them, for the same reason: this suite's name sorts right into
// the contended region of the list below. The suite itself is an ordinary
// `tests/all/<name>.rs` file.
#[path = "all/notification_channel_regions.rs"]
mod notification_channel_regions;

// Declared beside them, for the same reason: this suite's name sorts right into
// the contended region of the list below. The suite itself is an ordinary
// `tests/all/<name>.rs` file.
#[path = "all/resource_set_batch_basis.rs"]
mod resource_set_batch_basis;

// Declared beside them, for the same reason: this suite's name sorts right into
// the contended region of the list below. The suite itself is an ordinary
// `tests/all/<name>.rs` file.
#[path = "all/put_admin_account_during_offboarding.rs"]
mod put_admin_account_during_offboarding;

// Declared beside them, for the same reason: this suite's name sorts right into
// the contended region of the list below. The suite itself is an ordinary
// `tests/all/<name>.rs` file.
#[path = "all/resource_set_item_service_field.rs"]
mod resource_set_item_service_field;

// Declared beside them, for the same reason: this suite's name sorts right into
// the contended region of the list below. The suite itself is an ordinary
// `tests/all/<name>.rs` file.
#[path = "all/list_namespace_page_token_scoping.rs"]
mod list_namespace_page_token_scoping;

// Declared beside them, for the same reason: this suite's name sorts right into
// the contended region of the list below. The suite itself is an ordinary
// `tests/all/<name>.rs` file.
#[path = "all/namespaced_list_page_single_rule.rs"]
mod namespaced_list_page_single_rule;

// Declared beside them, for the same reason: this suite's name sorts right into
// the contended region of the list below. The suite itself is an ordinary
// `tests/all/<name>.rs` file.
#[path = "all/served_namespaced_list_token_scoping.rs"]
mod served_namespaced_list_token_scoping;

// Declared beside them, for the same reason: this suite's name sorts right into
// the contended region of the list below. The suite itself is an ordinary
// `tests/all/<name>.rs` file.
#[path = "all/organization_roster_clock_reading.rs"]
mod organization_roster_clock_reading;

// Declared beside them, for the same reason: this suite's name sorts right into
// the contended region of the list below. The suite itself is an ordinary
// `tests/all/<name>.rs` file.
#[path = "all/discovered_resources_page_token_scoping.rs"]
mod discovered_resources_page_token_scoping;

// Declared beside them, for the same reason: this suite's name sorts right into
// the contended region of the list below. The suite itself is an ordinary
// `tests/all/<name>.rs` file.
#[path = "all/collection_scoped_page_tokens.rs"]
mod collection_scoped_page_tokens;

// Declared beside them, for the same reason: this suite's name sorts right into
// the contended region of the list below. The suite itself is an ordinary
// `tests/all/<name>.rs` file.
#[path = "all/notification_channel_arn_form.rs"]
mod notification_channel_arn_form;

// Declared here too, outside the sorted list below, for the reason stated above:
// inserting a `mod` into that list collides with every other fms suite landing
// in the same window. The suite itself is an ordinary `tests/all/<name>.rs` file.
#[path = "all/admin_scope_in_force_after_onboarding.rs"]
mod admin_scope_in_force_after_onboarding;

// Declared beside them, for the same reason: inserting a `mod` into the sorted
// list below collides with every other fms suite landing in the same window, and
// this suite's name sorts right into the stretch of it the late-sorting suites
// keep landing in. The suite itself is an ordinary `tests/all/<name>.rs` file.
#[path = "all/admin_slot_write_clock_reading.rs"]
mod admin_slot_write_clock_reading;

// Declared beside them, for the same reason: this suite's name sorts right into
// the contended region of the list below. The suite itself is an ordinary
// `tests/all/<name>.rs` file.
#[path = "all/page_token_canonical_spelling.rs"]
mod page_token_canonical_spelling;

// Declared beside them, for the same reason: this suite's name sorts right into
// the contended region of the list below. The suite itself is an ordinary
// `tests/all/<name>.rs` file.
#[path = "all/member_resource_owner_account.rs"]
mod member_resource_owner_account;

#[path = "all/admin_account_disassociation.rs"]
mod admin_account_disassociation;
#[path = "all/admin_account_scopes.rs"]
mod admin_account_scopes;
#[path = "all/admin_accounts_for_organization.rs"]
mod admin_accounts_for_organization;
#[path = "all/administrator_gated_operations.rs"]
mod administrator_gated_operations;
#[path = "all/apps_list_lifecycle.rs"]
mod apps_list_lifecycle;
#[path = "all/apps_list_reporting.rs"]
mod apps_list_reporting;
#[path = "all/apps_lists_enumeration.rs"]
mod apps_lists_enumeration;
#[path = "all/arn_form_single_rule.rs"]
mod arn_form_single_rule;
#[path = "all/associable_discovered_resources.rs"]
mod associable_discovered_resources;
#[path = "all/compliance_detail.rs"]
mod compliance_detail;
#[path = "all/compliance_status.rs"]
mod compliance_status;
#[path = "all/discovered_resources.rs"]
mod discovered_resources;
#[path = "all/error_message_members.rs"]
mod error_message_members;
#[path = "all/get_admin_scope_lifecycle_status.rs"]
mod get_admin_scope_lifecycle_status;
#[path = "all/list_tags_across_administrator_states.rs"]
mod list_tags_across_administrator_states;
#[path = "all/managed_list_deletion.rs"]
mod managed_list_deletion;
#[path = "all/member_accounts.rs"]
mod member_accounts;
#[path = "all/notification_channel_deletion.rs"]
mod notification_channel_deletion;
#[path = "all/page_size_resolution.rs"]
mod page_size_resolution;
#[path = "all/policy_administrator_gate.rs"]
mod policy_administrator_gate;
#[path = "all/policy_document_validation.rs"]
mod policy_document_validation;
#[path = "all/policy_identifier_lookup.rs"]
mod policy_identifier_lookup;
#[path = "all/policy_lifecycle.rs"]
mod policy_lifecycle;
#[path = "all/policy_mutation_admin_scope.rs"]
mod policy_mutation_admin_scope;
#[path = "all/policy_resource_cleanup.rs"]
mod policy_resource_cleanup;
#[path = "all/policy_status_derivation.rs"]
mod policy_status_derivation;
#[path = "all/protection_status.rs"]
mod protection_status;
#[path = "all/protocols_list_lifecycle.rs"]
mod protocols_list_lifecycle;
#[path = "all/protocols_lists_enumeration.rs"]
mod protocols_lists_enumeration;
#[path = "all/resource_set_deletion.rs"]
mod resource_set_deletion;
#[path = "all/resource_set_disassociation.rs"]
mod resource_set_disassociation;
#[path = "all/resource_set_listing.rs"]
mod resource_set_listing;
#[path = "all/resource_set_resources.rs"]
mod resource_set_resources;
#[path = "all/resource_set_status_derivation.rs"]
mod resource_set_status_derivation;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_list_duplicate_keys_on_writers.rs"]
mod tag_list_duplicate_keys_on_writers;
#[path = "all/tag_removal.rs"]
mod tag_removal;
#[path = "all/tagging_across_resource_kinds.rs"]
mod tagging_across_resource_kinds;
#[path = "all/tagging_administrator_gate.rs"]
mod tagging_administrator_gate;
#[path = "all/tagging_and_managing_admins.rs"]
mod tagging_and_managing_admins;
#[path = "all/tagging_refusals_across_resource_kinds.rs"]
mod tagging_refusals_across_resource_kinds;
