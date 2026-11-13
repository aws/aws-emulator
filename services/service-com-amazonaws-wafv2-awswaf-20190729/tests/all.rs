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
#[path = "all/api_key_deletion.rs"]
mod api_key_deletion;
#[path = "all/api_key_identity.rs"]
mod api_key_identity;
#[path = "all/api_key_integration_url.rs"]
mod api_key_integration_url;
#[path = "all/api_key_listing.rs"]
mod api_key_listing;
#[path = "all/api_key_minting.rs"]
mod api_key_minting;
#[path = "all/api_key_token_form.rs"]
mod api_key_token_form;
#[path = "all/available_managed_rule_groups.rs"]
mod available_managed_rule_groups;
#[path = "all/challenge_immunity.rs"]
mod challenge_immunity;
#[path = "all/check_capacity.rs"]
mod check_capacity;
#[path = "all/counted_rule_samples.rs"]
mod counted_rule_samples;
#[path = "all/entity_description_form.rs"]
mod entity_description_form;
#[path = "all/exception_messages.rs"]
mod exception_messages;
#[path = "all/firewall_manager_rule_groups.rs"]
mod firewall_manager_rule_groups;
#[path = "all/forwarded_ip_set_reference.rs"]
mod forwarded_ip_set_reference;
#[path = "all/get_web_acl_for_resource.rs"]
mod get_web_acl_for_resource;
#[path = "all/global_scope_web_acl_creation.rs"]
mod global_scope_web_acl_creation;
#[path = "all/invalid_permission_policy_message.rs"]
mod invalid_permission_policy_message;
#[path = "all/ip_set_address_order.rs"]
mod ip_set_address_order;
#[path = "all/ip_set_canonicalization.rs"]
mod ip_set_canonicalization;
#[path = "all/ip_set_listing.rs"]
mod ip_set_listing;
#[path = "all/ip_set_membership_order.rs"]
mod ip_set_membership_order;
#[path = "all/ip_set_mutations.rs"]
mod ip_set_mutations;
#[path = "all/list_resources_for_web_acl.rs"]
mod list_resources_for_web_acl;
#[path = "all/list_rule_groups.rs"]
mod list_rule_groups;
#[path = "all/logging_configuration.rs"]
mod logging_configuration;
#[path = "all/logging_destination_names.rs"]
mod logging_destination_names;
#[path = "all/logging_redacted_fields.rs"]
mod logging_redacted_fields;
#[path = "all/managed_products.rs"]
mod managed_products;
#[path = "all/managed_products_by_vendor.rs"]
mod managed_products_by_vendor;
#[path = "all/managed_rule_group_description.rs"]
mod managed_rule_group_description;
#[path = "all/managed_rule_group_labels.rs"]
mod managed_rule_group_labels;
#[path = "all/managed_rule_group_pinned_version.rs"]
mod managed_rule_group_pinned_version;
#[path = "all/managed_rule_group_version_expiry.rs"]
mod managed_rule_group_version_expiry;
#[path = "all/managed_rule_group_versioning.rs"]
mod managed_rule_group_versioning;
#[path = "all/managed_rule_group_versions.rs"]
mod managed_rule_group_versions;
#[path = "all/managed_rule_groups.rs"]
mod managed_rule_groups;
#[path = "all/managed_rule_sets.rs"]
mod managed_rule_sets;
#[path = "all/mobile_sdk_release_listing.rs"]
mod mobile_sdk_release_listing;
#[path = "all/mobile_sdk_releases.rs"]
mod mobile_sdk_releases;
#[path = "all/monetization_amount_form.rs"]
mod monetization_amount_form;
#[path = "all/overridden_group_rule_names.rs"]
mod overridden_group_rule_names;
#[path = "all/parameter_validation.rs"]
mod parameter_validation;
#[path = "all/permission_policy.rs"]
mod permission_policy;
#[path = "all/published_managed_rule_catalog.rs"]
mod published_managed_rule_catalog;
#[path = "all/rate_based_statement_managed_keys.rs"]
mod rate_based_statement_managed_keys;
#[path = "all/rate_based_window_recovery.rs"]
mod rate_based_window_recovery;
#[path = "all/referenced_resource_deletes.rs"]
mod referenced_resource_deletes;
#[path = "all/refusal_classes.rs"]
mod refusal_classes;
#[path = "all/regex_pattern_set_lifecycle.rs"]
mod regex_pattern_set_lifecycle;
#[path = "all/regex_pattern_set_listing.rs"]
mod regex_pattern_set_listing;
#[path = "all/regex_pattern_set_updates.rs"]
mod regex_pattern_set_updates;
#[path = "all/regex_syntax_validation.rs"]
mod regex_syntax_validation;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/resource_untagging.rs"]
mod resource_untagging;
#[path = "all/response_members.rs"]
mod response_members;
#[path = "all/revenue_statistics_summary.rs"]
mod revenue_statistics_summary;
#[path = "all/revenue_time_series.rs"]
mod revenue_time_series;
#[path = "all/rule_evaluation_order.rs"]
mod rule_evaluation_order;
#[path = "all/rule_group_capacity_refusal.rs"]
mod rule_group_capacity_refusal;
#[path = "all/rule_group_deletion.rs"]
mod rule_group_deletion;
#[path = "all/rule_group_lifecycle.rs"]
mod rule_group_lifecycle;
#[path = "all/rule_group_override_staging.rs"]
mod rule_group_override_staging;
#[path = "all/rule_group_updates.rs"]
mod rule_group_updates;
#[path = "all/sampled_requests.rs"]
mod sampled_requests;
#[path = "all/sampled_rule_provenance.rs"]
mod sampled_rule_provenance;
#[path = "all/settlement_records.rs"]
mod settlement_records;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_listing.rs"]
mod tag_listing;
#[path = "all/tag_listing_marker.rs"]
mod tag_listing_marker;
#[path = "all/tag_value_round_trip.rs"]
mod tag_value_round_trip;
#[path = "all/top_path_statistics.rs"]
mod top_path_statistics;
#[path = "all/wcu_cost_model.rs"]
mod wcu_cost_model;
#[path = "all/web_acl_association_fanout.rs"]
mod web_acl_association_fanout;
#[path = "all/web_acl_associations.rs"]
mod web_acl_associations;
#[path = "all/web_acl_edge_reports.rs"]
mod web_acl_edge_reports;
#[path = "all/web_acl_lifecycle.rs"]
mod web_acl_lifecycle;
#[path = "all/web_acl_request_metrics.rs"]
mod web_acl_request_metrics;
#[path = "all/web_acl_rule_set_preconditions.rs"]
mod web_acl_rule_set_preconditions;
