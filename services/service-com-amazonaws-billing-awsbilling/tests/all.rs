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
#[path = "all/arn_account_boundary.rs"]
mod arn_account_boundary;
#[path = "all/billing_feature_access.rs"]
mod billing_feature_access;
#[path = "all/billing_preference_pagination.rs"]
mod billing_preference_pagination;
#[path = "all/billing_preferences.rs"]
mod billing_preferences;
#[path = "all/billing_view_active_time_range.rs"]
mod billing_view_active_time_range;
#[path = "all/billing_view_health_gate.rs"]
mod billing_view_health_gate;
#[path = "all/billing_view_wire_fidelity.rs"]
mod billing_view_wire_fidelity;
#[path = "all/billing_views.rs"]
mod billing_views;
#[path = "all/business_support_account_charges.rs"]
mod business_support_account_charges;
#[path = "all/credit_allocation_family.rs"]
mod credit_allocation_family;
#[path = "all/credit_allocation_history.rs"]
mod credit_allocation_history;
#[path = "all/credits.rs"]
mod credits;
#[path = "all/enterprise_support_charge_summary.rs"]
mod enterprise_support_charge_summary;
#[path = "all/refusal_and_projection_fidelity.rs"]
mod refusal_and_projection_fidelity;
#[path = "all/refusal_message_text.rs"]
mod refusal_message_text;
#[path = "all/rejection_fidelity.rs"]
mod rejection_fidelity;
#[path = "all/source_account_id.rs"]
mod source_account_id;
#[path = "all/source_view_disassociation.rs"]
mod source_view_disassociation;
#[path = "all/source_views.rs"]
mod source_views;
#[path = "all/source_views_tags_and_policy.rs"]
mod source_views_tags_and_policy;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_set_duplicate_keys.rs"]
mod tag_set_duplicate_keys;
#[path = "all/untag_resource.rs"]
mod untag_resource;
