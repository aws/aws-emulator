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
#[path = "all/shield_attack_statistics.rs"]
mod shield_attack_statistics;
#[path = "all/shield_describe_attack.rs"]
mod shield_describe_attack;
#[path = "all/shield_drt_access.rs"]
mod shield_drt_access;
#[path = "all/shield_list_attacks.rs"]
mod shield_list_attacks;
#[path = "all/shield_protection_groups_tags_and_account.rs"]
mod shield_protection_groups_tags_and_account;
#[path = "all/shield_subscription_and_protections.rs"]
mod shield_subscription_and_protections;
#[path = "all/shield_unsubscribed_account.rs"]
mod shield_unsubscribed_account;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
