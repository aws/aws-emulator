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
#[path = "all/account_authorization.rs"]
mod account_authorization;

#[path = "all/account_subscriptions_listing.rs"]
mod account_subscriptions_listing;

#[path = "all/arn_reference_refusals.rs"]
mod arn_reference_refusals;

#[path = "all/attribute_document_parse_refusal.rs"]
mod attribute_document_parse_refusal;

#[path = "all/confirm_subscription.rs"]
mod confirm_subscription;

#[path = "all/confirm_subscription_token_refusal.rs"]
mod confirm_subscription_token_refusal;

#[path = "all/create_platform_application.rs"]
mod create_platform_application;

#[path = "all/create_platform_endpoint.rs"]
mod create_platform_endpoint;

#[path = "all/data_protection_policy.rs"]
mod data_protection_policy;

#[path = "all/delivery_policy_and_message_branches.rs"]
mod delivery_policy_and_message_branches;

#[path = "all/delivery_policy_retry_bounds.rs"]
mod delivery_policy_retry_bounds;

#[path = "all/delivery_status_logging.rs"]
mod delivery_status_logging;

#[path = "all/empty_policy_attribute_value.rs"]
mod empty_policy_attribute_value;

#[path = "all/effective_delivery_policy_entry_layout.rs"]
mod effective_delivery_policy_entry_layout;

#[path = "all/error_envelope.rs"]
mod error_envelope;

#[path = "all/feedback_role_trust.rs"]
mod feedback_role_trust;

#[path = "all/fifo_archive_replay.rs"]
mod fifo_archive_replay;

#[path = "all/fifo_message_deduplication.rs"]
mod fifo_message_deduplication;

#[path = "all/fifo_pairing_and_subscribe_conflicts.rs"]
mod fifo_pairing_and_subscribe_conflicts;

#[path = "all/fifo_topic_deduplication_attribute.rs"]
mod fifo_topic_deduplication_attribute;

#[path = "all/firehose_subscription_delivery.rs"]
mod firehose_subscription_delivery;

#[path = "all/filter_policy_delivery.rs"]
mod filter_policy_delivery;

#[path = "all/filter_policy_match_budget.rs"]
mod filter_policy_match_budget;

#[path = "all/filter_policy_parse_diagnostic.rs"]
mod filter_policy_parse_diagnostic;

#[path = "all/filter_policy_wildcard_operator.rs"]
mod filter_policy_wildcard_operator;

#[path = "all/https_subscription_delivery.rs"]
mod https_subscription_delivery;

#[path = "all/invalid_next_token_sentence.rs"]
mod invalid_next_token_sentence;

#[path = "all/lambda_subscription_dead_letter.rs"]
mod lambda_subscription_dead_letter;

#[path = "all/lambda_subscription_delivery.rs"]
mod lambda_subscription_delivery;

#[path = "all/lambda_subscription_retries.rs"]
mod lambda_subscription_retries;
#[path = "all/lambda_subscription_grants.rs"]
mod lambda_subscription_grants;

#[path = "all/list_subscriptions.rs"]
mod list_subscriptions;

#[path = "all/notification_signature.rs"]
mod notification_signature;

#[path = "all/number_attribute_cast_refusal.rs"]
mod number_attribute_cast_refusal;

#[path = "all/origination_numbers_listing.rs"]
mod origination_numbers_listing;

#[path = "all/opt_out_check_reports_every_number.rs"]
mod opt_out_check_reports_every_number;

#[path = "all/phone_number_opt_in.rs"]
mod phone_number_opt_in;

#[path = "all/phone_number_opt_out_listing.rs"]
mod phone_number_opt_out_listing;

#[path = "all/platform_application_deletion.rs"]
mod platform_application_deletion;

#[path = "all/platform_application_reads.rs"]
mod platform_application_reads;

#[path = "all/platform_applications.rs"]
mod platform_applications;

#[path = "all/platform_endpoint_deletion.rs"]
mod platform_endpoint_deletion;

#[path = "all/platform_endpoint_registration_lifecycle.rs"]
mod platform_endpoint_registration_lifecycle;

#[path = "all/publish_authorization.rs"]
mod publish_authorization;

#[path = "all/publish_batch.rs"]
mod publish_batch;

#[path = "all/publish_destination_exclusivity.rs"]
mod publish_destination_exclusivity;

#[path = "all/put_data_protection_policy.rs"]
mod put_data_protection_policy;

#[path = "all/refusal_sentences.rs"]
mod refusal_sentences;

#[path = "all/region_isolation.rs"]
mod region_isolation;

#[path = "all/request_derived_refusals.rs"]
mod request_derived_refusals;

#[path = "all/regional_listings.rs"]
mod regional_listings;

#[path = "all/resource_tags.rs"]
mod resource_tags;

#[path = "all/set_endpoint_attributes.rs"]
mod set_endpoint_attributes;

#[path = "all/set_platform_application_attributes.rs"]
mod set_platform_application_attributes;

#[path = "all/set_subscription_attributes.rs"]
mod set_subscription_attributes;

#[path = "all/sms_attributes.rs"]
mod sms_attributes;

#[path = "all/sms_sandbox_account_status.rs"]
mod sms_sandbox_account_status;

#[path = "all/sms_sandbox_destination_numbers.rs"]
mod sms_sandbox_destination_numbers;

#[path = "all/sms_sandbox_phone_numbers.rs"]
mod sms_sandbox_phone_numbers;

#[path = "all/sms_sandbox_origination_entities.rs"]
mod sms_sandbox_origination_entities;

#[path = "all/sms_sandbox_verification.rs"]
mod sms_sandbox_verification;

#[path = "all/sqs_delivery_authorization.rs"]
mod sqs_delivery_authorization;

#[path = "all/sqs_delivery_queue_encryption.rs"]
mod sqs_delivery_queue_encryption;

#[path = "all/sqs_subscription_delivery.rs"]
mod sqs_subscription_delivery;

#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;

#[path = "all/subscribe_endpoint_form_refusals.rs"]
mod subscribe_endpoint_form_refusals;

#[path = "all/subscription_attributes.rs"]
mod subscription_attributes;

#[path = "all/subscription_dead_letter_queue.rs"]
mod subscription_dead_letter_queue;

#[path = "all/subscription_endpoint_rebinding.rs"]
mod subscription_endpoint_rebinding;

#[path = "all/subscription_filter_policy_delivery.rs"]
mod subscription_filter_policy_delivery;

#[path = "all/subscription_filter_policy_matching.rs"]
mod subscription_filter_policy_matching;

#[path = "all/subscription_pending_confirmation.rs"]
mod subscription_pending_confirmation;

#[path = "all/subscription_policy_attributes.rs"]
mod subscription_policy_attributes;

#[path = "all/topic_archive_window.rs"]
mod topic_archive_window;

#[path = "all/topic_attributes.rs"]
mod topic_attributes;

#[path = "all/topic_encryption_key_resolved_per_publish.rs"]
mod topic_encryption_key_resolved_per_publish;

#[path = "all/topic_encryption_key_state.rs"]
mod topic_encryption_key_state;

#[path = "all/topic_key_policy_authorization.rs"]
mod topic_key_policy_authorization;

#[path = "all/topic_notification_metrics.rs"]
mod topic_notification_metrics;

#[path = "all/topic_policy_authorization.rs"]
mod topic_policy_authorization;

#[path = "all/topic_policy_conditions.rs"]
mod topic_policy_conditions;

#[path = "all/topics_subscriptions_publish.rs"]
mod topics_subscriptions_publish;

#[path = "all/unsubscribe_stops_delivery.rs"]
mod unsubscribe_stops_delivery;

#[path = "all/validation_rules.rs"]
mod validation_rules;
