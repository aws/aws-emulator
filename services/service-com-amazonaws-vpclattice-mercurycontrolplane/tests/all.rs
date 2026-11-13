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
#[path = "all/batch_update_rule.rs"]
mod batch_update_rule;
#[path = "all/default_rule_and_rule_update.rs"]
mod default_rule_and_rule_update;
#[path = "all/delete_rule_and_target_group.rs"]
mod delete_rule_and_target_group;
#[path = "all/error_payload_construction.rs"]
mod error_payload_construction;
#[path = "all/list_rules.rs"]
mod list_rules;
#[path = "all/list_service_network_service_associations.rs"]
mod list_service_network_service_associations;
#[path = "all/listener_conflict_deletion_and_error_shape.rs"]
mod listener_conflict_deletion_and_error_shape;
#[path = "all/listeners.rs"]
mod listeners;
#[path = "all/minted_id_alphabet.rs"]
mod minted_id_alphabet;
#[path = "all/referential_integrity_conflict_messages.rs"]
mod referential_integrity_conflict_messages;
#[path = "all/resource_not_found_messages.rs"]
mod resource_not_found_messages;
#[path = "all/resource_policy.rs"]
mod resource_policy;
#[path = "all/rules.rs"]
mod rules;
#[path = "all/service_network_update_and_timestamps.rs"]
mod service_network_update_and_timestamps;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/target_group_required_config.rs"]
mod target_group_required_config;
#[path = "all/target_groups.rs"]
mod target_groups;
#[path = "all/validation_pattern_rendering.rs"]
mod validation_pattern_rendering;
#[path = "all/vpclattice_service_slice.rs"]
mod vpclattice_service_slice;
