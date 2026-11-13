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
#[path = "all/amount_magnitude_refusals.rs"]
mod amount_magnitude_refusals;
#[path = "all/commitment_bounds.rs"]
mod commitment_bounds;
#[path = "all/foreign_resource_arn_refusal.rs"]
mod foreign_resource_arn_refusal;
#[path = "all/idempotent_request_identity.rs"]
mod idempotent_request_identity;
#[path = "all/negative_amount_sign_rule.rs"]
mod negative_amount_sign_rule;
#[path = "all/offering_catalog.rs"]
mod offering_catalog;
#[path = "all/offering_product_type_coverage.rs"]
mod offering_product_type_coverage;
#[path = "all/offering_rate_catalog.rs"]
mod offering_rate_catalog;
#[path = "all/offering_service_code.rs"]
mod offering_service_code;
#[path = "all/page_cursor_canonical_spelling.rs"]
mod page_cursor_canonical_spelling;
#[path = "all/plan_id_not_found_message.rs"]
mod plan_id_not_found_message;
#[path = "all/plan_rate_card.rs"]
mod plan_rate_card;
#[path = "all/property_filter_rule.rs"]
mod property_filter_rule;
#[path = "all/queued_plan_wire_rules.rs"]
mod queued_plan_wire_rules;
#[path = "all/queued_purchase_deletion.rs"]
mod queued_purchase_deletion;
#[path = "all/return_monthly_allowance.rs"]
mod return_monthly_allowance;
#[path = "all/return_window_bound.rs"]
mod return_window_bound;
#[path = "all/savings_plans_lifecycle.rs"]
mod savings_plans_lifecycle;
#[path = "all/shared_catalog_paging.rs"]
mod shared_catalog_paging;
#[path = "all/shared_page_cursor_rule.rs"]
mod shared_page_cursor_rule;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_key_removal_round_trip.rs"]
mod tag_key_removal_round_trip;
#[path = "all/tag_key_round_trip.rs"]
mod tag_key_round_trip;
#[path = "all/tag_quota_errors.rs"]
mod tag_quota_errors;
#[path = "all/tagging_error_paths.rs"]
mod tagging_error_paths;
#[path = "all/validation_echoes_submitted_values.rs"]
mod validation_echoes_submitted_values;
