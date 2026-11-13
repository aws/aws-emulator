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
#[path = "all/account_association_billing_periods.rs"]
mod account_association_billing_periods;
#[path = "all/account_association_group_filter.rs"]
mod account_association_group_filter;
#[path = "all/account_exclusivity_at_list_bounds.rs"]
mod account_exclusivity_at_list_bounds;
#[path = "all/account_exclusivity_refusal.rs"]
mod account_exclusivity_refusal;
#[path = "all/associate_accounts_generation_history.rs"]
mod associate_accounts_generation_history;
#[path = "all/associate_pricing_rules.rs"]
mod associate_pricing_rules;
#[path = "all/association_delete_gate_spans_billing_periods.rs"]
mod association_delete_gate_spans_billing_periods;
#[path = "all/association_size_is_billing_period_scoped.rs"]
mod association_size_is_billing_period_scoped;
#[path = "all/association_writes_reach_superseded_generations.rs"]
mod association_writes_reach_superseded_generations;
#[path = "all/auto_associate_billing_group_cap.rs"]
mod auto_associate_billing_group_cap;
#[path = "all/aws_managed_pricing_plan_tagging.rs"]
mod aws_managed_pricing_plan_tagging;
#[path = "all/batch_disassociate_resources_from_custom_line_item.rs"]
mod batch_disassociate_resources_from_custom_line_item;
#[path = "all/batch_resource_association_closed_billing_period.rs"]
mod batch_resource_association_closed_billing_period;
#[path = "all/billing_family_membership.rs"]
mod billing_family_membership;
#[path = "all/billing_group_account_grouping_validation.rs"]
mod billing_group_account_grouping_validation;
#[path = "all/billing_group_billing_periods.rs"]
mod billing_group_billing_periods;
#[path = "all/billing_group_core_loop.rs"]
mod billing_group_core_loop;
#[path = "all/billing_group_cost_report.rs"]
mod billing_group_cost_report;
#[path = "all/billing_group_period_occupancy.rs"]
mod billing_group_period_occupancy;
#[path = "all/billing_group_size_one_rule.rs"]
mod billing_group_size_one_rule;
#[path = "all/billing_transfer_identity.rs"]
mod billing_transfer_identity;
#[path = "all/billing_transfer_preference.rs"]
mod billing_transfer_preference;
#[path = "all/billing_transfer_preference_unchanged_update.rs"]
mod billing_transfer_preference_unchanged_update;
#[path = "all/bounded_delete_narrows_resource_associations.rs"]
mod bounded_delete_narrows_resource_associations;
#[path = "all/cost_report_charges_of_the_generation_in_effect.rs"]
mod cost_report_charges_of_the_generation_in_effect;
#[path = "all/cost_report_mutual_percentage_associations.rs"]
mod cost_report_mutual_percentage_associations;
#[path = "all/cost_report_token_breakdown_scope.rs"]
mod cost_report_token_breakdown_scope;
#[path = "all/create_custom_line_item_billing_period_window.rs"]
mod create_custom_line_item_billing_period_window;
#[path = "all/create_pricing_plan_rule_references.rs"]
mod create_pricing_plan_rule_references;
#[path = "all/custom_line_item_and_create_name_refusals.rs"]
mod custom_line_item_and_create_name_refusals;
#[path = "all/custom_line_item_associations_accumulate.rs"]
mod custom_line_item_associations_accumulate;
#[path = "all/custom_line_item_billing_group_scope.rs"]
mod custom_line_item_billing_group_scope;
#[path = "all/custom_line_item_creation_billing_period.rs"]
mod custom_line_item_creation_billing_period;
#[path = "all/custom_line_item_tag_lifecycle.rs"]
mod custom_line_item_tag_lifecycle;
#[path = "all/custom_line_item_versions_by_billing_period.rs"]
mod custom_line_item_versions_by_billing_period;
#[path = "all/delete_auto_associate_billing_group.rs"]
mod delete_auto_associate_billing_group;
#[path = "all/delete_custom_line_item.rs"]
mod delete_custom_line_item;
#[path = "all/deleted_billing_group_holds_pricing_plan_back.rs"]
mod deleted_billing_group_holds_pricing_plan_back;
#[path = "all/deleted_billing_group_period_reads.rs"]
mod deleted_billing_group_period_reads;
#[path = "all/disassociate_accounts.rs"]
mod disassociate_accounts;
#[path = "all/disassociate_pricing_rules.rs"]
mod disassociate_pricing_rules;
#[path = "all/list_billing_group_cost_reports.rs"]
mod list_billing_group_cost_reports;
#[path = "all/list_custom_line_item_versions.rs"]
mod list_custom_line_item_versions;
#[path = "all/list_custom_line_items.rs"]
mod list_custom_line_items;
#[path = "all/membership_writes_keep_superseded_generations.rs"]
mod membership_writes_keep_superseded_generations;
#[path = "all/ordered_billing_period_range_refusal.rs"]
mod ordered_billing_period_range_refusal;
#[path = "all/organization_linked_account_roster.rs"]
mod organization_linked_account_roster;
#[path = "all/page_token_spelling.rs"]
mod page_token_spelling;
#[path = "all/pagination_token_opacity.rs"]
mod pagination_token_opacity;
#[path = "all/payer_account_authorization.rs"]
mod payer_account_authorization;
#[path = "all/payer_account_gate.rs"]
mod payer_account_gate;
#[path = "all/payer_gate_precedes_idempotency_existence_and_assembly.rs"]
mod payer_gate_precedes_idempotency_existence_and_assembly;
#[path = "all/percentage_custom_line_item_resource_associations.rs"]
mod percentage_custom_line_item_resource_associations;
#[path = "all/pricing_plan_billing_periods.rs"]
mod pricing_plan_billing_periods;
#[path = "all/pricing_plan_delete.rs"]
mod pricing_plan_delete;
#[path = "all/pricing_plan_list.rs"]
mod pricing_plan_list;
#[path = "all/pricing_plan_unchanged_update.rs"]
mod pricing_plan_unchanged_update;
#[path = "all/pricing_plan_update.rs"]
mod pricing_plan_update;
#[path = "all/pricing_plans_associated_with_pricing_rule.rs"]
mod pricing_plans_associated_with_pricing_rule;
#[path = "all/pricing_rule_association_billing_periods.rs"]
mod pricing_rule_association_billing_periods;
#[path = "all/pricing_rule_association_period_subsets.rs"]
mod pricing_rule_association_period_subsets;
#[path = "all/pricing_rule_association_periods.rs"]
mod pricing_rule_association_periods;
#[path = "all/pricing_rule_billing_periods.rs"]
mod pricing_rule_billing_periods;
#[path = "all/pricing_rule_core_loop.rs"]
mod pricing_rule_core_loop;
#[path = "all/pricing_rule_custom_tier_usage_ranges.rs"]
mod pricing_rule_custom_tier_usage_ranges;
#[path = "all/pricing_rule_delete_reader_agreement.rs"]
mod pricing_rule_delete_reader_agreement;
#[path = "all/pricing_rule_lifecycle.rs"]
mod pricing_rule_lifecycle;
#[path = "all/pricing_rule_rate_exclusivity.rs"]
mod pricing_rule_rate_exclusivity;
#[path = "all/pricing_rule_scope_members.rs"]
mod pricing_rule_scope_members;
#[path = "all/pricing_rule_unchanged_update.rs"]
mod pricing_rule_unchanged_update;
#[path = "all/pricing_rule_update.rs"]
mod pricing_rule_update;
#[path = "all/pricing_rules_associated_to_pricing_plan.rs"]
mod pricing_rules_associated_to_pricing_plan;
#[path = "all/reserved_tag_keys.rs"]
mod reserved_tag_keys;
#[path = "all/resource_association_billing_periods.rs"]
mod resource_association_billing_periods;
#[path = "all/resource_re_association_restores_billing_periods.rs"]
mod resource_re_association_restores_billing_periods;
#[path = "all/shared_ordered_billing_period_range_check.rs"]
mod shared_ordered_billing_period_range_check;
#[path = "all/shared_tag_dispatch.rs"]
mod shared_tag_dispatch;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tagging_surface.rs"]
mod tagging_surface;
#[path = "all/untag_created_tags.rs"]
mod untag_created_tags;
#[path = "all/update_billing_group.rs"]
mod update_billing_group;
#[path = "all/update_billing_group_no_op_generations.rs"]
mod update_billing_group_no_op_generations;
#[path = "all/update_custom_line_item.rs"]
mod update_custom_line_item;
#[path = "all/update_custom_line_item_billing_period_window.rs"]
mod update_custom_line_item_billing_period_window;
#[path = "all/validation_exception_field_member_names.rs"]
mod validation_exception_field_member_names;
#[path = "all/validation_exception_message.rs"]
mod validation_exception_message;
#[path = "all/validation_message_value_sensitivity.rs"]
mod validation_message_value_sensitivity;
#[path = "all/validation_messages.rs"]
mod validation_messages;
#[path = "all/versioned_resource_generations.rs"]
mod versioned_resource_generations;
#[path = "all/write_audit_records.rs"]
mod write_audit_records;
