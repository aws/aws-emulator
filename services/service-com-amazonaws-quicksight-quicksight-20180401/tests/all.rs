// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! Put a new suite at `tests/all/<name>.rs` and declare it below, keeping the list
//! sorted.
//! A suite that is not declared here compiles nowhere and its tests never run. A
//! suite left at the top level of `tests/` fails the build.
// `tests/all.rs` is a test-target CRATE ROOT, so a bare `mod` resolves to a SIBLING
// (`tests/state_roundtrip.rs`) — which the layout check refuses. The `#[path]` is what
// points it into `tests/all/`.
#[path = "all/account_subscription.rs"]
mod account_subscription;
#[path = "all/data_set_permissions.rs"]
mod data_set_permissions;
#[path = "all/data_source_data_set_lifecycle.rs"]
mod data_source_data_set_lifecycle;
#[path = "all/data_source_not_found_arn.rs"]
mod data_source_not_found_arn;
#[path = "all/data_source_update.rs"]
mod data_source_update;
#[path = "all/delete_namespace.rs"]
mod delete_namespace;
#[path = "all/delete_theme.rs"]
mod delete_theme;
#[path = "all/describe_namespace.rs"]
mod describe_namespace;
#[path = "all/iam_policy_assignment_lifecycle.rs"]
mod iam_policy_assignment_lifecycle;
#[path = "all/mutation_response_status.rs"]
mod mutation_response_status;
#[path = "all/namespace_lifecycle.rs"]
mod namespace_lifecycle;
#[path = "all/refresh_schedule_lifecycle.rs"]
mod refresh_schedule_lifecycle;
#[path = "all/refresh_schedule_quota_refusal.rs"]
mod refresh_schedule_quota_refusal;
#[path = "all/refusal_messages.rs"]
mod refusal_messages;
#[path = "all/search_data_sets.rs"]
mod search_data_sets;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/theme_alias_lifecycle.rs"]
mod theme_alias_lifecycle;
#[path = "all/theme_lifecycle.rs"]
mod theme_lifecycle;
#[path = "all/topic_lifecycle.rs"]
mod topic_lifecycle;
#[path = "all/folder_lifecycle.rs"]
mod folder_lifecycle;
#[path = "all/group_lifecycle.rs"]
mod group_lifecycle;
