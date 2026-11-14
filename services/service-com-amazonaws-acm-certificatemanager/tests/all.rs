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
#[path = "all/account_configuration.rs"]
mod account_configuration;
#[path = "all/account_configuration_authorization.rs"]
mod account_configuration_authorization;
#[path = "all/account_configuration_tokens.rs"]
mod account_configuration_tokens;
#[path = "all/acme_account_listing.rs"]
mod acme_account_listing;
#[path = "all/acme_account_registration.rs"]
mod acme_account_registration;
#[path = "all/acme_authorization.rs"]
mod acme_authorization;
#[path = "all/acme_cross_account_arn_envelope.rs"]
mod acme_cross_account_arn_envelope;
#[path = "all/acme_directory_endpoint.rs"]
mod acme_directory_endpoint;
#[path = "all/acme_domain_validation_delete.rs"]
mod acme_domain_validation_delete;
#[path = "all/acme_domain_validation_lifecycle.rs"]
mod acme_domain_validation_lifecycle;
#[path = "all/acme_domain_validation_listing.rs"]
mod acme_domain_validation_listing;
#[path = "all/acme_domain_validation_update.rs"]
mod acme_domain_validation_update;
#[path = "all/acme_endpoint_delete_cascade.rs"]
mod acme_endpoint_delete_cascade;
#[path = "all/acme_endpoint_lifecycle.rs"]
mod acme_endpoint_lifecycle;
#[path = "all/acme_endpoint_list.rs"]
mod acme_endpoint_list;
#[path = "all/acme_endpoint_update.rs"]
mod acme_endpoint_update;
#[path = "all/acme_endpoint_url_reaches_the_caller.rs"]
mod acme_endpoint_url_reaches_the_caller;
#[path = "all/acme_external_account_binding_credentials.rs"]
mod acme_external_account_binding_credentials;
#[path = "all/acme_external_account_binding_delete.rs"]
mod acme_external_account_binding_delete;
#[path = "all/acme_external_account_binding_lifecycle.rs"]
mod acme_external_account_binding_lifecycle;
#[path = "all/acme_external_account_binding_listing.rs"]
mod acme_external_account_binding_listing;
#[path = "all/acme_external_account_binding_revocation.rs"]
mod acme_external_account_binding_revocation;
#[path = "all/acme_external_account_binding_role_trust.rs"]
mod acme_external_account_binding_role_trust;
#[path = "all/acme_nonce_field_name.rs"]
mod acme_nonce_field_name;
#[path = "all/acme_refusal_message_rules.rs"]
mod acme_refusal_message_rules;
#[path = "all/acme_resource_deletion_authorization.rs"]
mod acme_resource_deletion_authorization;
#[path = "all/acme_surface_wire_values.rs"]
mod acme_surface_wire_values;
#[path = "all/certificate_detail_projection.rs"]
mod certificate_detail_projection;
#[path = "all/certificate_distinguished_names.rs"]
mod certificate_distinguished_names;
#[path = "all/certificate_domain_validations.rs"]
mod certificate_domain_validations;
#[path = "all/certificate_export.rs"]
mod certificate_export;
#[path = "all/certificate_idempotency_expiry.rs"]
mod certificate_idempotency_expiry;
#[path = "all/certificate_import.rs"]
mod certificate_import;
#[path = "all/certificate_import_material.rs"]
mod certificate_import_material;
#[path = "all/certificate_in_use_by_another_service.rs"]
mod certificate_in_use_by_another_service;
#[path = "all/certificate_lifecycle.rs"]
mod certificate_lifecycle;
#[path = "all/certificate_list_filters.rs"]
mod certificate_list_filters;
#[path = "all/certificate_not_found_message.rs"]
mod certificate_not_found_message;
#[path = "all/certificate_options_member_verdict.rs"]
mod certificate_options_member_verdict;
#[path = "all/certificate_options_not_issued_class.rs"]
mod certificate_options_not_issued_class;
#[path = "all/certificate_options_update.rs"]
mod certificate_options_update;
#[path = "all/certificate_pending_validation_window.rs"]
mod certificate_pending_validation_window;
#[path = "all/certificate_refusal_classes.rs"]
mod certificate_refusal_classes;
#[path = "all/certificate_retrieval.rs"]
mod certificate_retrieval;
#[path = "all/certificate_revocation.rs"]
mod certificate_revocation;
#[path = "all/certificate_search.rs"]
mod certificate_search;
#[path = "all/certificate_state_gated_refusals.rs"]
mod certificate_state_gated_refusals;
#[path = "all/certificate_tag_removal.rs"]
mod certificate_tag_removal;
#[path = "all/certificate_validation_state_machine.rs"]
mod certificate_validation_state_machine;
#[path = "all/certificate_wire_representation.rs"]
mod certificate_wire_representation;
#[path = "all/delegated_zone_certificate_validation.rs"]
mod delegated_zone_certificate_validation;
#[path = "all/dns_validation_published_record.rs"]
mod dns_validation_published_record;
#[path = "all/domain_validation_summary_members.rs"]
mod domain_validation_summary_members;
#[path = "all/email_validation_dispatch.rs"]
mod email_validation_dispatch;
#[path = "all/generic_tagging_certificate_refusal_text.rs"]
mod generic_tagging_certificate_refusal_text;
#[path = "all/imported_certificate_arn_is_a_reference.rs"]
mod imported_certificate_arn_is_a_reference;
#[path = "all/imported_certificate_issuer_and_key_purposes.rs"]
mod imported_certificate_issuer_and_key_purposes;
#[path = "all/imported_certificate_serves_an_https_listener.rs"]
mod imported_certificate_serves_an_https_listener;
#[path = "all/imported_certificate_serves_custom_domain.rs"]
mod imported_certificate_serves_custom_domain;
#[path = "all/list_certificates_next_token_message.rs"]
mod list_certificates_next_token_message;
#[path = "all/pending_validation_and_tag_envelopes.rs"]
mod pending_validation_and_tag_envelopes;
#[path = "all/refusal_codes_and_search_projection.rs"]
mod refusal_codes_and_search_projection;
#[path = "all/refusal_message_templates.rs"]
mod refusal_message_templates;
#[path = "all/renew_certificate_issued_target_set.rs"]
mod renew_certificate_issued_target_set;
#[path = "all/requested_certificate_transition_spans.rs"]
mod requested_certificate_transition_spans;
#[path = "all/resend_validation_email.rs"]
mod resend_validation_email;
#[path = "all/resource_tag_input_constraints.rs"]
mod resource_tag_input_constraints;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/stated_enum_value_sets.rs"]
mod stated_enum_value_sets;
#[path = "all/tag_surface_refusal_subject.rs"]
mod tag_surface_refusal_subject;
#[path = "all/validation_domain_superdomain.rs"]
mod validation_domain_superdomain;
#[path = "all/validation_mail_dispatch_span.rs"]
mod validation_mail_dispatch_span;
#[path = "all/validation_message_rendering.rs"]
mod validation_message_rendering;
