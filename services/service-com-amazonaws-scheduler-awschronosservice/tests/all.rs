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
#[path = "all/dead_letter_delivery.rs"]
mod dead_letter_delivery;
#[path = "all/flexible_time_window_messages.rs"]
mod flexible_time_window_messages;
#[path = "all/list_tags_for_resource_bounds.rs"]
mod list_tags_for_resource_bounds;
#[path = "all/occurrence_timing.rs"]
mod occurrence_timing;
#[path = "all/pass_role_authorization.rs"]
mod pass_role_authorization;
#[path = "all/per_schedule_target_input.rs"]
mod per_schedule_target_input;
#[path = "all/reserved_schedule_group.rs"]
mod reserved_schedule_group;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/schedule_definition_rules.rs"]
mod schedule_definition_rules;
#[path = "all/schedule_group_listing.rs"]
mod schedule_group_listing;
#[path = "all/schedule_group_read.rs"]
mod schedule_group_read;
#[path = "all/schedule_invocation.rs"]
mod schedule_invocation;
#[path = "all/schedule_lifecycle.rs"]
mod schedule_lifecycle;
#[path = "all/server_paced_dispatch.rs"]
mod server_paced_dispatch;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tagging_refusal_messages.rs"]
mod tagging_refusal_messages;
#[path = "all/universal_target_invocation.rs"]
mod universal_target_invocation;
#[path = "all/wire_harness.rs"]
mod wire_harness;
