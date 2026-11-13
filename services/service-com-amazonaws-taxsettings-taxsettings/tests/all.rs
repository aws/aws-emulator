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
#[path = "all/accepted_registration_verifies.rs"]
mod accepted_registration_verifies;
#[path = "all/account_meta_data.rs"]
mod account_meta_data;
#[path = "all/additional_info_blocks.rs"]
mod additional_info_blocks;
#[path = "all/address_country_requirements.rs"]
mod address_country_requirements;
#[path = "all/address_member_jurisdictions.rs"]
mod address_member_jurisdictions;
#[path = "all/batch_get_tax_exemptions_failed_accounts.rs"]
mod batch_get_tax_exemptions_failed_accounts;
#[path = "all/batch_put_mixed_row_codes.rs"]
mod batch_put_mixed_row_codes;
#[path = "all/batch_refusal_rows.rs"]
mod batch_refusal_rows;
#[path = "all/batch_row_error_codes.rs"]
mod batch_row_error_codes;
#[path = "all/batch_tax_registration.rs"]
mod batch_tax_registration;
#[path = "all/conflict_refusal_messages.rs"]
mod conflict_refusal_messages;
#[path = "all/country_requirements.rs"]
mod country_requirements;
#[path = "all/country_rules.rs"]
mod country_rules;
#[path = "all/declared_member_defaults.rs"]
mod declared_member_defaults;
#[path = "all/derived_address_precedence.rs"]
mod derived_address_precedence;
#[path = "all/derived_registration_members.rs"]
mod derived_registration_members;
#[path = "all/document_destination_location.rs"]
mod document_destination_location;
#[path = "all/document_token_validity.rs"]
mod document_token_validity;
#[path = "all/exemption_access_record.rs"]
mod exemption_access_record;
#[path = "all/exemption_authority_support.rs"]
mod exemption_authority_support;
#[path = "all/exemption_identity_padding.rs"]
mod exemption_identity_padding;
#[path = "all/exemption_support_case.rs"]
mod exemption_support_case;
#[path = "all/exemption_type_admission.rs"]
mod exemption_type_admission;
#[path = "all/fidelity_rules.rs"]
mod fidelity_rules;
#[path = "all/listing_page_bounds.rs"]
mod listing_page_bounds;
#[path = "all/missing_resource_errors.rs"]
mod missing_resource_errors;
#[path = "all/non_api_delete_jurisdictions.rs"]
mod non_api_delete_jurisdictions;
#[path = "all/organization_management_standing.rs"]
mod organization_management_standing;
#[path = "all/pending_trn_conflict.rs"]
mod pending_trn_conflict;
#[path = "all/put_tax_exemption_state.rs"]
mod put_tax_exemption_state;
#[path = "all/registration_type_refusals.rs"]
mod registration_type_refusals;
#[path = "all/repeated_account_ids.rs"]
mod repeated_account_ids;
#[path = "all/retired_document_tokens.rs"]
mod retired_document_tokens;
#[path = "all/sector_registration_types.rs"]
mod sector_registration_types;
#[path = "all/shared_distinct_account_ids.rs"]
mod shared_distinct_account_ids;
#[path = "all/shared_page_builder.rs"]
mod shared_page_builder;
#[path = "all/shared_registration_delete.rs"]
mod shared_registration_delete;
#[path = "all/shared_supplemental_lifecycle.rs"]
mod shared_supplemental_lifecycle;
#[path = "all/shared_trn_settle.rs"]
mod shared_trn_settle;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/supplemental_tax_registrations.rs"]
mod supplemental_tax_registrations;
#[path = "all/supplemental_trn_conflict.rs"]
mod supplemental_trn_conflict;
#[path = "all/supplemental_trn_spelling_identity.rs"]
mod supplemental_trn_spelling_identity;
#[path = "all/tax_document_delivery.rs"]
mod tax_document_delivery;
#[path = "all/tax_exemption_types.rs"]
mod tax_exemption_types;
#[path = "all/tax_exemptions.rs"]
mod tax_exemptions;
#[path = "all/tax_inheritance_election.rs"]
mod tax_inheritance_election;
#[path = "all/tax_registration_document.rs"]
mod tax_registration_document;
#[path = "all/tax_registration_lifecycle.rs"]
mod tax_registration_lifecycle;
#[path = "all/trn_validation_outcome.rs"]
mod trn_validation_outcome;
