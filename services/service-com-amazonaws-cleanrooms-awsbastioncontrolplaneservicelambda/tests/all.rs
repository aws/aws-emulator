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
#[path = "all/addressed_resource_refusals.rs"]
mod addressed_resource_refusals;
#[path = "all/already_exists_conflict_messages.rs"]
mod already_exists_conflict_messages;
#[path = "all/analysis_rule_column_reference_refusals.rs"]
mod analysis_rule_column_reference_refusals;
#[path = "all/analysis_template_deletion.rs"]
mod analysis_template_deletion;
#[path = "all/authorization_failure_messages.rs"]
mod authorization_failure_messages;
#[path = "all/cleanrooms_analysis_templates.rs"]
mod cleanrooms_analysis_templates;
#[path = "all/cleanrooms_collaboration_lifecycle.rs"]
mod cleanrooms_collaboration_lifecycle;
#[path = "all/cleanrooms_configured_table.rs"]
mod cleanrooms_configured_table;
#[path = "all/cleanrooms_created_resource_defaults.rs"]
mod cleanrooms_created_resource_defaults;
#[path = "all/cleanrooms_privacy_budget_template.rs"]
mod cleanrooms_privacy_budget_template;
#[path = "all/cleanrooms_refusal_classification.rs"]
mod cleanrooms_refusal_classification;
#[path = "all/cleanrooms_schemas.rs"]
mod cleanrooms_schemas;
#[path = "all/collaboration_change_request_auto_commit.rs"]
mod collaboration_change_request_auto_commit;
#[path = "all/collaboration_change_request_decisions.rs"]
mod collaboration_change_request_decisions;
#[path = "all/collaboration_change_requests.rs"]
mod collaboration_change_requests;
#[path = "all/configured_table_analysis_rule_class_names.rs"]
mod configured_table_analysis_rule_class_names;
#[path = "all/configured_table_analysis_rule_reads.rs"]
mod configured_table_analysis_rule_reads;
#[path = "all/configured_table_analysis_rule_round_trip.rs"]
mod configured_table_analysis_rule_round_trip;
#[path = "all/configured_table_analysis_rules.rs"]
mod configured_table_analysis_rules;
#[path = "all/configured_table_association_analysis_rules.rs"]
mod configured_table_association_analysis_rules;
#[path = "all/configured_table_association_identifier_forms.rs"]
mod configured_table_association_identifier_forms;
#[path = "all/configured_table_associations.rs"]
mod configured_table_associations;
#[path = "all/configured_table_updates.rs"]
mod configured_table_updates;
#[path = "all/list_members.rs"]
mod list_members;
#[path = "all/member_removal.rs"]
mod member_removal;
#[path = "all/membership_creation_and_departure.rs"]
mod membership_creation_and_departure;
#[path = "all/membership_updates.rs"]
mod membership_updates;
#[path = "all/privacy_budget_template_listing.rs"]
mod privacy_budget_template_listing;
#[path = "all/protected_queries.rs"]
mod protected_queries;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
