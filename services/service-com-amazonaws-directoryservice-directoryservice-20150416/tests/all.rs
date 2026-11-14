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
#[path = "all/ad_assessment.rs"]
mod ad_assessment;
#[path = "all/ad_connector.rs"]
mod ad_connector;
#[path = "all/alias_reassignment.rs"]
mod alias_reassignment;
#[path = "all/aws_worded_diagnostics.rs"]
mod aws_worded_diagnostics;
#[path = "all/ca_enrollment_policy.rs"]
mod ca_enrollment_policy;
#[path = "all/certificate_absence_message.rs"]
mod certificate_absence_message;
#[path = "all/certificate_deregistration.rs"]
mod certificate_deregistration;
#[path = "all/certificate_functions.rs"]
mod certificate_functions;
#[path = "all/certificate_listing.rs"]
mod certificate_listing;
#[path = "all/certificate_writes_during_a_build.rs"]
mod certificate_writes_during_a_build;
#[path = "all/certificates.rs"]
mod certificates;
#[path = "all/computer_account_unavailable_wording.rs"]
mod computer_account_unavailable_wording;
#[path = "all/computer_attributes.rs"]
mod computer_attributes;
#[path = "all/computer_conflict_diagnostic.rs"]
mod computer_conflict_diagnostic;
#[path = "all/computers.rs"]
mod computers;
#[path = "all/conditional_forwarder_deletion.rs"]
mod conditional_forwarder_deletion;
#[path = "all/conditional_forwarder_update.rs"]
mod conditional_forwarder_update;
#[path = "all/conditional_forwarders.rs"]
mod conditional_forwarders;
#[path = "all/directory_build_stages.rs"]
mod directory_build_stages;
#[path = "all/directory_data_access.rs"]
mod directory_data_access;
#[path = "all/directory_data_access_disable.rs"]
mod directory_data_access_disable;
#[path = "all/directory_deletion.rs"]
mod directory_deletion;
#[path = "all/directory_dns_addresses.rs"]
mod directory_dns_addresses;
#[path = "all/directory_lifecycle.rs"]
mod directory_lifecycle;
#[path = "all/directory_limits.rs"]
mod directory_limits;
#[path = "all/directory_networking_and_settings.rs"]
mod directory_networking_and_settings;
#[path = "all/directory_os_version.rs"]
mod directory_os_version;
#[path = "all/directory_security_group.rs"]
mod directory_security_group;
#[path = "all/directory_settings.rs"]
mod directory_settings;
#[path = "all/directory_unsharing.rs"]
mod directory_unsharing;
#[path = "all/directory_update_reads.rs"]
mod directory_update_reads;
#[path = "all/domain_controller_resize.rs"]
mod domain_controller_resize;
#[path = "all/domain_controllers.rs"]
mod domain_controllers;
#[path = "all/error_envelopes.rs"]
mod error_envelopes;
#[path = "all/event_topic_deregistration.rs"]
mod event_topic_deregistration;
#[path = "all/event_topics.rs"]
mod event_topics;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/ip_route_removal.rs"]
mod ip_route_removal;
#[path = "all/ip_route_update_serialization.rs"]
mod ip_route_update_serialization;
#[path = "all/ldaps.rs"]
mod ldaps;
#[path = "all/log_subscription_forwarding.rs"]
mod log_subscription_forwarding;
#[path = "all/log_subscriptions.rs"]
mod log_subscriptions;
#[path = "all/microsoft_ad.rs"]
mod microsoft_ad;
#[path = "all/pagination_token_refusal.rs"]
mod pagination_token_refusal;
#[path = "all/pattern_constraint_refusals.rs"]
mod pattern_constraint_refusals;
#[path = "all/polled_lifecycles.rs"]
mod polled_lifecycles;
#[path = "all/refusal_classification_rules.rs"]
mod refusal_classification_rules;
#[path = "all/refusal_texts.rs"]
mod refusal_texts;
#[path = "all/refusal_wording.rs"]
mod refusal_wording;
#[path = "all/region_replication.rs"]
mod region_replication;
#[path = "all/request_id_header.rs"]
mod request_id_header;
#[path = "all/schema_extension_cancellation.rs"]
mod schema_extension_cancellation;
#[path = "all/schema_extensions.rs"]
mod schema_extensions;
#[path = "all/shared_directories.rs"]
mod shared_directories;
#[path = "all/sharing_and_tagging_guards.rs"]
mod sharing_and_tagging_guards;
#[path = "all/single_sign_on.rs"]
mod single_sign_on;
#[path = "all/single_sign_on_credentials.rs"]
mod single_sign_on_credentials;
#[path = "all/single_sign_on_disable.rs"]
mod single_sign_on_disable;
#[path = "all/snapshot_deletion.rs"]
mod snapshot_deletion;
#[path = "all/snapshot_read_scopes.rs"]
mod snapshot_read_scopes;
#[path = "all/snapshots.rs"]
mod snapshots;
#[path = "all/sso_access_url_assignment.rs"]
mod sso_access_url_assignment;
#[path = "all/sso_access_url_prerequisite.rs"]
mod sso_access_url_prerequisite;
#[path = "all/sso_disable_preconditions.rs"]
mod sso_disable_preconditions;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_limit_diagnostic.rs"]
mod tag_limit_diagnostic;
#[path = "all/trust_conflict_message.rs"]
mod trust_conflict_message;
#[path = "all/trust_failure_lifecycle.rs"]
mod trust_failure_lifecycle;
#[path = "all/trust_outcomes.rs"]
mod trust_outcomes;
#[path = "all/trust_peer_resolution.rs"]
mod trust_peer_resolution;
#[path = "all/trusts.rs"]
mod trusts;
#[path = "all/user_passwords.rs"]
mod user_passwords;
#[path = "all/validation_pattern_wording.rs"]
mod validation_pattern_wording;
#[path = "all/vpc_resolution.rs"]
mod vpc_resolution;
