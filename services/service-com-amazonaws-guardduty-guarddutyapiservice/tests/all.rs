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
#[path = "all/account_id_well_formedness.rs"]
mod account_id_well_formedness;
#[path = "all/administrator_disassociation.rs"]
mod administrator_disassociation;
#[path = "all/batch_id_set_normalisation.rs"]
mod batch_id_set_normalisation;
#[path = "all/client_rejections.rs"]
mod client_rejections;
#[path = "all/client_token_store.rs"]
mod client_token_store;
#[path = "all/coverage_addon_details.rs"]
mod coverage_addon_details;
#[path = "all/coverage_listing.rs"]
mod coverage_listing;
#[path = "all/coverage_statistics.rs"]
mod coverage_statistics;
#[path = "all/criterion_value_domain.rs"]
mod criterion_value_domain;
#[path = "all/custom_detection_rule_association_arn_and_expiry.rs"]
mod custom_detection_rule_association_arn_and_expiry;
#[path = "all/custom_detection_rule_association_delete.rs"]
mod custom_detection_rule_association_delete;
#[path = "all/custom_detection_rule_association_delete_idempotence.rs"]
mod custom_detection_rule_association_delete_idempotence;
#[path = "all/custom_detection_rule_association_identity.rs"]
mod custom_detection_rule_association_identity;
#[path = "all/custom_detection_rule_association_listed_handle.rs"]
mod custom_detection_rule_association_listed_handle;
#[path = "all/custom_detection_rule_association_listing.rs"]
mod custom_detection_rule_association_listing;
#[path = "all/custom_detection_rule_association_update.rs"]
mod custom_detection_rule_association_update;
#[path = "all/custom_detection_rule_org_configuration_delete.rs"]
mod custom_detection_rule_org_configuration_delete;
#[path = "all/custom_detection_rule_org_configuration_listing.rs"]
mod custom_detection_rule_org_configuration_listing;
#[path = "all/custom_detection_rule_org_configuration_update.rs"]
mod custom_detection_rule_org_configuration_update;
#[path = "all/custom_detection_rules.rs"]
mod custom_detection_rules;
#[path = "all/deleted_resource_tag_writes.rs"]
mod deleted_resource_tag_writes;
#[path = "all/detector_configuration_timestamp.rs"]
mod detector_configuration_timestamp;
#[path = "all/detector_data_sources_projection.rs"]
mod detector_data_sources_projection;
#[path = "all/detector_lifecycle.rs"]
mod detector_lifecycle;
#[path = "all/detector_sub_resource_creates.rs"]
mod detector_sub_resource_creates;
#[path = "all/detector_sub_resource_deletes.rs"]
mod detector_sub_resource_deletes;
#[path = "all/detector_update_without_configuration.rs"]
mod detector_update_without_configuration;
#[path = "all/entity_set_delete_retention.rs"]
mod entity_set_delete_retention;
#[path = "all/entity_set_id_refusal_classes.rs"]
mod entity_set_id_refusal_classes;
#[path = "all/expected_bucket_owner_form.rs"]
mod expected_bucket_owner_form;
#[path = "all/filter_action_suppression.rs"]
mod filter_action_suppression;
#[path = "all/filter_condition_spellings.rs"]
mod filter_condition_spellings;
#[path = "all/filter_delete.rs"]
mod filter_delete;
#[path = "all/filter_detector_timestamp.rs"]
mod filter_detector_timestamp;
#[path = "all/filter_lifecycle.rs"]
mod filter_lifecycle;
#[path = "all/filter_listing.rs"]
mod filter_listing;
#[path = "all/filter_only_conditions.rs"]
mod filter_only_conditions;
#[path = "all/filter_rank_positions.rs"]
mod filter_rank_positions;
#[path = "all/filter_update_isolation.rs"]
mod filter_update_isolation;
#[path = "all/finding_field_catalog.rs"]
mod finding_field_catalog;
#[path = "all/finding_record_update_clock.rs"]
mod finding_record_update_clock;
#[path = "all/findings_archive_shared_rule.rs"]
mod findings_archive_shared_rule;
#[path = "all/findings_archive_state.rs"]
mod findings_archive_state;
#[path = "all/findings_feedback.rs"]
mod findings_feedback;
#[path = "all/findings_metadata_batch_clock.rs"]
mod findings_metadata_batch_clock;
#[path = "all/findings_statistics.rs"]
mod findings_statistics;
#[path = "all/free_trial_days.rs"]
mod free_trial_days;
#[path = "all/identity_authorization.rs"]
mod identity_authorization;
#[path = "all/investigation_clock_coupling.rs"]
mod investigation_clock_coupling;
#[path = "all/investigation_cloud_environment.rs"]
mod investigation_cloud_environment;
#[path = "all/investigation_detector_timestamp.rs"]
mod investigation_detector_timestamp;
#[path = "all/investigation_lifecycle.rs"]
mod investigation_lifecycle;
#[path = "all/investigation_listing.rs"]
mod investigation_listing;
#[path = "all/invitation_acceptance_refusal_text.rs"]
mod invitation_acceptance_refusal_text;
#[path = "all/invitation_count.rs"]
mod invitation_count;
#[path = "all/invitation_decline.rs"]
mod invitation_decline;
#[path = "all/invitation_delete.rs"]
mod invitation_delete;
#[path = "all/invitation_delivery.rs"]
mod invitation_delivery;
#[path = "all/invitation_listing.rs"]
mod invitation_listing;
#[path = "all/invitation_pending_agreement.rs"]
mod invitation_pending_agreement;
#[path = "all/invitation_unprocessed_results.rs"]
mod invitation_unprocessed_results;
#[path = "all/invite_members_no_send_clock.rs"]
mod invite_members_no_send_clock;
#[path = "all/ip_set_activation_allowance.rs"]
mod ip_set_activation_allowance;
#[path = "all/ip_set_lifecycle.rs"]
mod ip_set_lifecycle;
#[path = "all/ip_set_listing.rs"]
mod ip_set_listing;
#[path = "all/ip_set_location_syntax.rs"]
mod ip_set_location_syntax;
#[path = "all/ip_set_name_alphabet.rs"]
mod ip_set_name_alphabet;
#[path = "all/ip_set_update_and_delete.rs"]
mod ip_set_update_and_delete;
#[path = "all/legacy_condition_round_trip.rs"]
mod legacy_condition_round_trip;
#[path = "all/malware_protection_plan.rs"]
mod malware_protection_plan;
#[path = "all/malware_protection_plan_delete.rs"]
mod malware_protection_plan_delete;
#[path = "all/malware_protection_plan_listing.rs"]
mod malware_protection_plan_listing;
#[path = "all/malware_protection_plan_update.rs"]
mod malware_protection_plan_update;
#[path = "all/malware_protection_plan_validation.rs"]
mod malware_protection_plan_validation;
#[path = "all/malware_scan.rs"]
mod malware_scan;
#[path = "all/malware_scan_continuous_window.rs"]
mod malware_scan_continuous_window;
#[path = "all/malware_scan_description.rs"]
mod malware_scan_description;
#[path = "all/malware_scan_lifecycle.rs"]
mod malware_scan_lifecycle;
#[path = "all/malware_scan_page_window.rs"]
mod malware_scan_page_window;
#[path = "all/malware_scan_recovery_point_vault.rs"]
mod malware_scan_recovery_point_vault;
#[path = "all/malware_scan_role_arn.rs"]
mod malware_scan_role_arn;
#[path = "all/malware_scan_settings.rs"]
mod malware_scan_settings;
#[path = "all/master_account_disassociation.rs"]
mod master_account_disassociation;
#[path = "all/member_accounts.rs"]
mod member_accounts;
#[path = "all/member_agnostic_refusal_texts.rs"]
mod member_agnostic_refusal_texts;
#[path = "all/member_association.rs"]
mod member_association;
#[path = "all/member_batch_no_op_clock.rs"]
mod member_batch_no_op_clock;
#[path = "all/member_creation.rs"]
mod member_creation;
#[path = "all/member_deletion.rs"]
mod member_deletion;
#[path = "all/member_detector_configuration.rs"]
mod member_detector_configuration;
#[path = "all/member_detector_empty_report.rs"]
mod member_detector_empty_report;
#[path = "all/member_detector_organization_managed_switches.rs"]
mod member_detector_organization_managed_switches;
#[path = "all/member_detector_timestamp.rs"]
mod member_detector_timestamp;
#[path = "all/member_invitation.rs"]
mod member_invitation;
#[path = "all/member_invitation_email_verification.rs"]
mod member_invitation_email_verification;
#[path = "all/member_monitoring.rs"]
mod member_monitoring;
#[path = "all/member_monitoring_outstanding_invitation.rs"]
mod member_monitoring_outstanding_invitation;
#[path = "all/member_monitoring_restart.rs"]
mod member_monitoring_restart;
#[path = "all/member_monitoring_unanswered_invitation_addresses.rs"]
mod member_monitoring_unanswered_invitation_addresses;
#[path = "all/member_organization_auto_enable.rs"]
mod member_organization_auto_enable;
#[path = "all/member_relationship_scope.rs"]
mod member_relationship_scope;
#[path = "all/modeled_constraint_refusals.rs"]
mod modeled_constraint_refusals;
#[path = "all/negative_class_rejections.rs"]
mod negative_class_rejections;
#[path = "all/no_op_batch_clock.rs"]
mod no_op_batch_clock;
#[path = "all/no_op_mutation_clock.rs"]
mod no_op_mutation_clock;
#[path = "all/object_malware_scan.rs"]
mod object_malware_scan;
#[path = "all/org_configuration_authorization.rs"]
mod org_configuration_authorization;
#[path = "all/organization_admin_accounts.rs"]
mod organization_admin_accounts;
#[path = "all/organization_admin_no_op_clock.rs"]
mod organization_admin_no_op_clock;
#[path = "all/organization_admin_redesignation.rs"]
mod organization_admin_redesignation;
#[path = "all/organization_admin_removal.rs"]
mod organization_admin_removal;
#[path = "all/organization_configuration.rs"]
mod organization_configuration;
#[path = "all/organization_configuration_member_propagation.rs"]
mod organization_configuration_member_propagation;
#[path = "all/organization_managed_administrator_disassociation.rs"]
mod organization_managed_administrator_disassociation;
#[path = "all/organization_managed_members.rs"]
mod organization_managed_members;
#[path = "all/organization_statistics.rs"]
mod organization_statistics;
#[path = "all/page_token_listing_identity.rs"]
mod page_token_listing_identity;
#[path = "all/page_token_spelling.rs"]
mod page_token_spelling;
#[path = "all/published_detection_rule_catalog.rs"]
mod published_detection_rule_catalog;
#[path = "all/publishing_destination_delete.rs"]
mod publishing_destination_delete;
#[path = "all/publishing_destination_lifecycle.rs"]
mod publishing_destination_lifecycle;
#[path = "all/publishing_destination_listing.rs"]
mod publishing_destination_listing;
#[path = "all/publishing_destination_update.rs"]
mod publishing_destination_update;
#[path = "all/read_response_member_sets.rs"]
mod read_response_member_sets;
#[path = "all/refusal_message_texts.rs"]
mod refusal_message_texts;
#[path = "all/refused_create_clock.rs"]
mod refused_create_clock;
#[path = "all/refused_destination_and_scan_settings_clock.rs"]
mod refused_destination_and_scan_settings_clock;
#[path = "all/refused_mutation_clock.rs"]
mod refused_mutation_clock;
#[path = "all/request_region_partition_bounds.rs"]
mod request_region_partition_bounds;
#[path = "all/resource_not_found_messages.rs"]
mod resource_not_found_messages;
#[path = "all/resource_tag_addition.rs"]
mod resource_tag_addition;
#[path = "all/resource_tag_listing.rs"]
mod resource_tag_listing;
#[path = "all/resource_tag_removal.rs"]
mod resource_tag_removal;
#[path = "all/runtime_monitoring_exclusion.rs"]
mod runtime_monitoring_exclusion;
#[path = "all/sample_finding_catalog.rs"]
mod sample_finding_catalog;
#[path = "all/sample_finding_detector_timestamp.rs"]
mod sample_finding_detector_timestamp;
#[path = "all/set_delete_terminality.rs"]
mod set_delete_terminality;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_arn_resolution_seam.rs"]
mod tag_arn_resolution_seam;
#[path = "all/tag_map_projection.rs"]
mod tag_map_projection;
#[path = "all/tags_per_resource_ceiling.rs"]
mod tags_per_resource_ceiling;
#[path = "all/threat_entity_set_delete.rs"]
mod threat_entity_set_delete;
#[path = "all/threat_entity_set_lifecycle.rs"]
mod threat_entity_set_lifecycle;
#[path = "all/threat_entity_set_listing.rs"]
mod threat_entity_set_listing;
#[path = "all/threat_entity_set_update.rs"]
mod threat_entity_set_update;
#[path = "all/threat_intel_set_delete.rs"]
mod threat_intel_set_delete;
#[path = "all/threat_intel_set_listing.rs"]
mod threat_intel_set_listing;
#[path = "all/threat_intel_set_update.rs"]
mod threat_intel_set_update;
#[path = "all/threat_intel_sets.rs"]
mod threat_intel_sets;
#[path = "all/trusted_entity_set_delete.rs"]
mod trusted_entity_set_delete;
#[path = "all/trusted_entity_set_lifecycle.rs"]
mod trusted_entity_set_lifecycle;
#[path = "all/trusted_entity_set_listing.rs"]
mod trusted_entity_set_listing;
#[path = "all/trusted_entity_set_update.rs"]
mod trusted_entity_set_update;
#[path = "all/uploaded_list_location_endpoint.rs"]
mod uploaded_list_location_endpoint;
#[path = "all/usage_statistics.rs"]
mod usage_statistics;
