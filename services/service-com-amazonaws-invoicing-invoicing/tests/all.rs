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
#[path = "all/applied_repeat_is_an_ordinary_write.rs"]
mod applied_repeat_is_an_ordinary_write;
#[path = "all/client_token_identifies_one_request.rs"]
mod client_token_identifies_one_request;
#[path = "all/completed_validation_moves_the_plane_status.rs"]
mod completed_validation_moves_the_plane_status;
#[path = "all/create_path_tag_quota.rs"]
mod create_path_tag_quota;
#[path = "all/field_list_entry_message.rs"]
mod field_list_entry_message;
#[path = "all/fresh_validation_code_fresh_attempt_budget.rs"]
mod fresh_validation_code_fresh_attempt_budget;
#[path = "all/invoice_pdf_documents.rs"]
mod invoice_pdf_documents;
#[path = "all/invoice_profile_membership.rs"]
mod invoice_profile_membership;
#[path = "all/invoice_profiles.rs"]
mod invoice_profiles;
#[path = "all/invoice_summaries.rs"]
mod invoice_summaries;
#[path = "all/invoice_summary_period_rules.rs"]
mod invoice_summary_period_rules;
#[path = "all/invoice_unit_accounts_are_mutually_exclusive.rs"]
mod invoice_unit_accounts_are_mutually_exclusive;
#[path = "all/invoice_unit_lifecycle.rs"]
mod invoice_unit_lifecycle;
#[path = "all/marketplace_punch_out_config_follows_the_flag.rs"]
mod marketplace_punch_out_config_follows_the_flag;
#[path = "all/message_fits_the_modelled_bound.rs"]
mod message_fits_the_modelled_bound;
#[path = "all/modelled_enum_value_set.rs"]
mod modelled_enum_value_set;
#[path = "all/modelled_refusal_on_every_implemented_operation.rs"]
mod modelled_refusal_on_every_implemented_operation;
#[path = "all/own_account_and_request_derived_refusals.rs"]
mod own_account_and_request_derived_refusals;
#[path = "all/portal_endpoint_must_be_a_secure_url.rs"]
mod portal_endpoint_must_be_a_secure_url;
#[path = "all/portal_preference_conflict_and_delivery_refusals.rs"]
mod portal_preference_conflict_and_delivery_refusals;
#[path = "all/portal_validation_code_issue.rs"]
mod portal_validation_code_issue;
#[path = "all/procurement_portal_error_message_key.rs"]
mod procurement_portal_error_message_key;
#[path = "all/procurement_portal_preference_lifecycle.rs"]
mod procurement_portal_preference_lifecycle;
#[path = "all/procurement_portal_preference_plane_state.rs"]
mod procurement_portal_preference_plane_state;
#[path = "all/procurement_portal_preference_reads.rs"]
mod procurement_portal_preference_reads;
#[path = "all/procurement_portal_preference_status.rs"]
mod procurement_portal_preference_status;
#[path = "all/procurement_portal_preference_status_machine.rs"]
mod procurement_portal_preference_status_machine;
#[path = "all/procurement_portal_preferences.rs"]
mod procurement_portal_preferences;
#[path = "all/procurement_portal_supplier_page_tokens.rs"]
mod procurement_portal_supplier_page_tokens;
#[path = "all/procurement_portal_suppliers.rs"]
mod procurement_portal_suppliers;
#[path = "all/procurement_portals.rs"]
mod procurement_portals;
#[path = "all/punch_out_requirement_and_redirect_host.rs"]
mod punch_out_requirement_and_redirect_host;
#[path = "all/put_preference_replay_is_one_write.rs"]
mod put_preference_replay_is_one_write;
#[path = "all/refused_procurement_portal_writes.rs"]
mod refused_procurement_portal_writes;
#[path = "all/rejected_validation_code_keeps_the_version.rs"]
mod rejected_validation_code_keeps_the_version;
#[path = "all/rejected_validation_code_leaves_other_preferences_alone.rs"]
mod rejected_validation_code_leaves_other_preferences_alone;
#[path = "all/reported_arn_fits_the_modelled_bound.rs"]
mod reported_arn_fits_the_modelled_bound;
#[path = "all/request_derived_refusal_message_bound.rs"]
mod request_derived_refusal_message_bound;
#[path = "all/retired_units_and_portal_validation.rs"]
mod retired_units_and_portal_validation;
#[path = "all/shared_marketplace_punch_out_rule.rs"]
mod shared_marketplace_punch_out_rule;
#[path = "all/shared_page_walk.rs"]
mod shared_page_walk;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/status_update_needs_an_enabled_plane.rs"]
mod status_update_needs_an_enabled_plane;
#[path = "all/tag_write_quota_counts_distinct_keys.rs"]
mod tag_write_quota_counts_distinct_keys;
#[path = "all/the_requester_reads_its_own_account.rs"]
mod the_requester_reads_its_own_account;
#[path = "all/validation_code_is_unpredictable.rs"]
mod validation_code_is_unpredictable;
#[path = "all/validation_message_wording.rs"]
mod validation_message_wording;
#[path = "all/wire_envelope_and_error_detail.rs"]
mod wire_envelope_and_error_detail;
