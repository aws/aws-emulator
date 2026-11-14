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
#[path = "all/account_scoped_recommendation_identifier.rs"]
mod account_scoped_recommendation_identifier;
#[path = "all/account_scoped_recommendation_list.rs"]
mod account_scoped_recommendation_list;
#[path = "all/batch_exclusion_foreign_account.rs"]
mod batch_exclusion_foreign_account;
#[path = "all/batch_exclusion_unique_arns.rs"]
mod batch_exclusion_unique_arns;
#[path = "all/cost_optimizing_aggregates.rs"]
mod cost_optimizing_aggregates;
#[path = "all/cursor_scope_encoding.rs"]
mod cursor_scope_encoding;
#[path = "all/localized_text.rs"]
mod localized_text;
#[path = "all/malformed_batch_precedes_the_gate.rs"]
mod malformed_batch_precedes_the_gate;
#[path = "all/organization_member_not_stored.rs"]
mod organization_member_not_stored;
#[path = "all/organization_member_set.rs"]
mod organization_member_set;
#[path = "all/organization_prioritized_only.rs"]
mod organization_prioritized_only;
#[path = "all/organization_recommendation_accounts.rs"]
mod organization_recommendation_accounts;
#[path = "all/organization_recommendation_lifecycle.rs"]
mod organization_recommendation_lifecycle;
#[path = "all/organization_recommendation_read.rs"]
mod organization_recommendation_read;
#[path = "all/organization_recommendation_resources.rs"]
mod organization_recommendation_resources;
#[path = "all/organization_recommendations.rs"]
mod organization_recommendations;
#[path = "all/owning_account_from_arn.rs"]
mod owning_account_from_arn;
#[path = "all/page_token_refusal_sentence.rs"]
mod page_token_refusal_sentence;
#[path = "all/priority_scoped_created_at.rs"]
mod priority_scoped_created_at;
#[path = "all/recommendation_loop.rs"]
mod recommendation_loop;
#[path = "all/recommendations_for_resource.rs"]
mod recommendations_for_resource;
#[path = "all/refusal_sentence_one_builder.rs"]
mod refusal_sentence_one_builder;
#[path = "all/rejected_value_echo.rs"]
mod rejected_value_echo;
#[path = "all/resource_arn_owning_account.rs"]
mod resource_arn_owning_account;
#[path = "all/seeded_support_plan.rs"]
mod seeded_support_plan;
#[path = "all/shared_lifecycle_write.rs"]
mod shared_lifecycle_write;
#[path = "all/shared_recommendation_filters.rs"]
mod shared_recommendation_filters;
#[path = "all/shared_resource_view.rs"]
mod shared_resource_view;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/support_level_gate.rs"]
mod support_level_gate;
#[path = "all/support_level_gate_request_independence.rs"]
mod support_level_gate_request_independence;
#[path = "all/validation_message_rendering.rs"]
mod validation_message_rendering;
