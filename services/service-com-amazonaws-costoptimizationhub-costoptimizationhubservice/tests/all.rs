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
#[path = "all/account_scoped_reads.rs"]
mod account_scoped_reads;
#[path = "all/commitment_purchase_savings.rs"]
mod commitment_purchase_savings;
#[path = "all/cost_optimization_hub.rs"]
mod cost_optimization_hub;
#[path = "all/efficiency_metrics.rs"]
mod efficiency_metrics;
#[path = "all/efficiency_metrics_compute_optimizer_gate.rs"]
mod efficiency_metrics_compute_optimizer_gate;
#[path = "all/efficiency_metrics_period_refusal.rs"]
mod efficiency_metrics_period_refusal;
#[path = "all/enrollment_read_scope_gate.rs"]
mod enrollment_read_scope_gate;
#[path = "all/management_account_gating.rs"]
mod management_account_gating;
#[path = "all/management_account_only_discount_visibility.rs"]
mod management_account_only_discount_visibility;
#[path = "all/max_results_bounds.rs"]
mod max_results_bounds;
#[path = "all/one_view_for_every_recommendation_reader.rs"]
mod one_view_for_every_recommendation_reader;
#[path = "all/order_by_fidelity.rs"]
mod order_by_fidelity;
#[path = "all/organization_enrollment.rs"]
mod organization_enrollment;
#[path = "all/organization_wide_preference_owner.rs"]
mod organization_wide_preference_owner;
#[path = "all/page_token_rule.rs"]
mod page_token_rule;
#[path = "all/preference_gates.rs"]
mod preference_gates;
#[path = "all/preference_refusal_atomicity.rs"]
mod preference_refusal_atomicity;
#[path = "all/preference_state_consistency.rs"]
mod preference_state_consistency;
#[path = "all/preference_writes.rs"]
mod preference_writes;
#[path = "all/recommendation_detail_lookup.rs"]
mod recommendation_detail_lookup;
#[path = "all/recommendation_read_scope_gate.rs"]
mod recommendation_read_scope_gate;
#[path = "all/recommendation_summaries.rs"]
mod recommendation_summaries;
#[path = "all/rejection_fidelity.rs"]
mod rejection_fidelity;
#[path = "all/shared_recommendation_view.rs"]
mod shared_recommendation_view;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/summary_metrics_and_refusals.rs"]
mod summary_metrics_and_refusals;
#[path = "all/validation_exception_fields.rs"]
mod validation_exception_fields;
