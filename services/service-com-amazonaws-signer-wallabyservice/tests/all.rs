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
#[path = "all/absent_required_query_parameter.rs"]
mod absent_required_query_parameter;

#[path = "all/arn_segment_shape.rs"]
mod arn_segment_shape;

#[path = "all/cancel_profile_status_transitions.rs"]
mod cancel_profile_status_transitions;

#[path = "all/code_signing_objects.rs"]
mod code_signing_objects;

#[path = "all/error_responses.rs"]
mod error_responses;

#[path = "all/job_invoker_is_recorded.rs"]
mod job_invoker_is_recorded;

#[path = "all/job_listing.rs"]
mod job_listing;

#[path = "all/modelled_bound_refusal_wording.rs"]
mod modelled_bound_refusal_wording;

#[path = "all/next_token_refusal_wording.rs"]
mod next_token_refusal_wording;

#[path = "all/no_content_class_is_framed_alike.rs"]
mod no_content_class_is_framed_alike;

#[path = "all/paginated_page_size.rs"]
mod paginated_page_size;

#[path = "all/payload_signing.rs"]
mod payload_signing;

#[path = "all/platform_id_character_class.rs"]
mod platform_id_character_class;

#[path = "all/platform_listing.rs"]
mod platform_listing;

#[path = "all/policy_document_size.rs"]
mod policy_document_size;

#[path = "all/profile_lifecycle_contract.rs"]
mod profile_lifecycle_contract;

#[path = "all/profile_listing_pagination.rs"]
mod profile_listing_pagination;

#[path = "all/profile_permission_principal_form.rs"]
mod profile_permission_principal_form;

#[path = "all/profile_permissions.rs"]
mod profile_permissions;

#[path = "all/profile_name_is_reserved.rs"]
mod profile_name_is_reserved;

#[path = "all/profile_revocation_and_identity.rs"]
mod profile_revocation_and_identity;

#[path = "all/profile_revocation_capability.rs"]
mod profile_revocation_capability;

#[path = "all/profile_revocation_is_recorded_once.rs"]
mod profile_revocation_is_recorded_once;

#[path = "all/profile_subresource_not_found_code.rs"]
mod profile_subresource_not_found_code;

#[path = "all/published_platform_rows.rs"]
mod published_platform_rows;

#[path = "all/put_profile_image_format_override.rs"]
mod put_profile_image_format_override;

#[path = "all/put_profile_platform_validation.rs"]
mod put_profile_platform_validation;

#[path = "all/refusal_contract_wording.rs"]
mod refusal_contract_wording;

#[path = "all/refusal_echo_is_bounded.rs"]
mod refusal_echo_is_bounded;

#[path = "all/region_scoping.rs"]
mod region_scoping;

#[path = "all/resource_tags.rs"]
mod resource_tags;

#[path = "all/revocation_status.rs"]
mod revocation_status;

#[path = "all/revocation_survives_a_reput.rs"]
mod revocation_survives_a_reput;

#[path = "all/revocation_writer_not_found_wording.rs"]
mod revocation_writer_not_found_wording;

#[path = "all/signature_expiry.rs"]
mod signature_expiry;

#[path = "all/signature_revocation.rs"]
mod signature_revocation;

#[path = "all/signature_timestamp_forms.rs"]
mod signature_timestamp_forms;

#[path = "all/signing_job_artifact.rs"]
mod signing_job_artifact;

#[path = "all/signing_platform_catalog.rs"]
mod signing_platform_catalog;

#[path = "all/signing_profile_lifecycle.rs"]
mod signing_profile_lifecycle;

#[path = "all/start_signing_job_platform_workflow.rs"]
mod start_signing_job_platform_workflow;

#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;

#[path = "all/supplied_nothing_refusal_wording.rs"]
mod supplied_nothing_refusal_wording;

#[path = "all/tag_keys_supplied_nothing_wording.rs"]
mod tag_keys_supplied_nothing_wording;

#[path = "all/tagging_arn_refusal_mapping.rs"]
mod tagging_arn_refusal_mapping;

#[path = "all/wire_probe.rs"]
mod wire_probe;
