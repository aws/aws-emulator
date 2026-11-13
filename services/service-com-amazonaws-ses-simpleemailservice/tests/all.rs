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
#[path = "all/configuration_set_delete.rs"]
mod configuration_set_delete;
#[path = "all/configuration_set_tracking_options.rs"]
mod configuration_set_tracking_options;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/identity_mail_from_domain.rs"]
mod identity_mail_from_domain;
#[path = "all/identity_notification_topics.rs"]
mod identity_notification_topics;
#[path = "all/ses_account_sending_and_legacy_addresses.rs"]
mod ses_account_sending_and_legacy_addresses;
#[path = "all/ses_active_receipt_rule_set.rs"]
mod ses_active_receipt_rule_set;
#[path = "all/ses_add_header_action_charset.rs"]
mod ses_add_header_action_charset;
#[path = "all/ses_configuration_set_creation.rs"]
mod ses_configuration_set_creation;
#[path = "all/ses_configuration_set_delivery_options.rs"]
mod ses_configuration_set_delivery_options;
#[path = "all/ses_configuration_set_event_destinations.rs"]
mod ses_configuration_set_event_destinations;
#[path = "all/ses_configuration_set_listing.rs"]
mod ses_configuration_set_listing;
#[path = "all/ses_configuration_set_reputation_metrics.rs"]
mod ses_configuration_set_reputation_metrics;
#[path = "all/ses_configuration_set_sending_state.rs"]
mod ses_configuration_set_sending_state;
#[path = "all/ses_create_receipt_rule.rs"]
mod ses_create_receipt_rule;
#[path = "all/ses_custom_verification_email_send.rs"]
mod ses_custom_verification_email_send;
#[path = "all/ses_custom_verification_email_template_creation.rs"]
mod ses_custom_verification_email_template_creation;
#[path = "all/ses_custom_verification_email_template_listing.rs"]
mod ses_custom_verification_email_template_listing;
#[path = "all/ses_custom_verification_email_template_update.rs"]
mod ses_custom_verification_email_template_update;
#[path = "all/ses_custom_verification_email_templates.rs"]
mod ses_custom_verification_email_templates;
#[path = "all/ses_custom_verification_template_sender_gate.rs"]
mod ses_custom_verification_template_sender_gate;
#[path = "all/ses_delete_configuration_set_event_destination.rs"]
mod ses_delete_configuration_set_event_destination;
#[path = "all/ses_delete_receipt_rule.rs"]
mod ses_delete_receipt_rule;
#[path = "all/ses_event_destination_sns_topic_existence.rs"]
mod ses_event_destination_sns_topic_existence;
#[path = "all/ses_identity_dkim.rs"]
mod ses_identity_dkim;
#[path = "all/ses_identity_domain_case.rs"]
mod ses_identity_domain_case;
#[path = "all/ses_identity_policies.rs"]
mod ses_identity_policies;
#[path = "all/ses_identity_policy_delete.rs"]
mod ses_identity_policy_delete;
#[path = "all/ses_identity_send_loop.rs"]
mod ses_identity_send_loop;
#[path = "all/ses_identity_verification_token.rs"]
mod ses_identity_verification_token;
#[path = "all/ses_inbound_receipt_rule_delivery.rs"]
mod ses_inbound_receipt_rule_delivery;
#[path = "all/ses_list_identities_cursor.rs"]
mod ses_list_identities_cursor;
#[path = "all/ses_missing_custom_verification_template_wording.rs"]
mod ses_missing_custom_verification_template_wording;
#[path = "all/ses_missing_event_destination_wording.rs"]
mod ses_missing_event_destination_wording;
#[path = "all/ses_production_access_gate_wording.rs"]
mod ses_production_access_gate_wording;
#[path = "all/ses_query_error_envelope.rs"]
mod ses_query_error_envelope;
#[path = "all/ses_receipt_filters.rs"]
mod ses_receipt_filters;
#[path = "all/ses_receipt_rule_destination_preflight.rs"]
mod ses_receipt_rule_destination_preflight;
#[path = "all/ses_receipt_rule_s3_destination_authorization.rs"]
mod ses_receipt_rule_s3_destination_authorization;
#[path = "all/ses_receipt_rule_set_creation.rs"]
mod ses_receipt_rule_set_creation;
#[path = "all/ses_receipt_rule_set_deletion.rs"]
mod ses_receipt_rule_set_deletion;
#[path = "all/ses_receipt_rule_set_listing.rs"]
mod ses_receipt_rule_set_listing;
#[path = "all/ses_receipt_rule_sets.rs"]
mod ses_receipt_rule_sets;
#[path = "all/ses_receipt_rule_stored_defaults.rs"]
mod ses_receipt_rule_stored_defaults;
#[path = "all/ses_refusal_names_the_refused_request.rs"]
mod ses_refusal_names_the_refused_request;
#[path = "all/ses_refused_calls_name_the_request.rs"]
mod ses_refused_calls_name_the_request;
#[path = "all/ses_reorder_receipt_rule_set.rs"]
mod ses_reorder_receipt_rule_set;
#[path = "all/ses_send_bulk_templated_email.rs"]
mod ses_send_bulk_templated_email;
#[path = "all/ses_send_raw_email.rs"]
mod ses_send_raw_email;
#[path = "all/ses_send_request_authorization.rs"]
mod ses_send_request_authorization;
#[path = "all/ses_send_templated_email.rs"]
mod ses_send_templated_email;
#[path = "all/ses_set_receipt_rule_position.rs"]
mod ses_set_receipt_rule_position;
#[path = "all/ses_source_identity_gate.rs"]
mod ses_source_identity_gate;
#[path = "all/ses_store_bounds.rs"]
mod ses_store_bounds;
#[path = "all/ses_template_creation.rs"]
mod ses_template_creation;
#[path = "all/ses_template_delete.rs"]
mod ses_template_delete;
#[path = "all/ses_template_deletion.rs"]
mod ses_template_deletion;
#[path = "all/ses_template_listing.rs"]
mod ses_template_listing;
#[path = "all/ses_template_update.rs"]
mod ses_template_update;
#[path = "all/ses_test_render_template.rs"]
mod ses_test_render_template;
#[path = "all/ses_tracking_options_conflict_refusal.rs"]
mod ses_tracking_options_conflict_refusal;
#[path = "all/ses_tracking_options_refusal_message.rs"]
mod ses_tracking_options_refusal_message;
#[path = "all/ses_unverified_from_address_wording.rs"]
mod ses_unverified_from_address_wording;
#[path = "all/ses_unverified_sender_refusal.rs"]
mod ses_unverified_sender_refusal;
#[path = "all/ses_unverified_sender_region.rs"]
mod ses_unverified_sender_region;
#[path = "all/ses_update_configuration_set_event_destination.rs"]
mod ses_update_configuration_set_event_destination;
#[path = "all/ses_update_receipt_rule.rs"]
mod ses_update_receipt_rule;
#[path = "all/ses_validation_refusal_wording.rs"]
mod ses_validation_refusal_wording;
#[path = "all/ses_verified_identity_discovery.rs"]
mod ses_verified_identity_discovery;
#[path = "all/ses_verify_email_address.rs"]
mod ses_verify_email_address;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
// The `z`-prefixed service-wide suites sort after the per-operation ones;
// `tests/all.rs` lists suites in file order.
#[path = "all/zprobe.rs"]
mod zprobe;
#[path = "all/zz_dump.rs"]
mod zz_dump;
