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
#[path = "all/error_messages.rs"]
mod error_messages;
#[path = "all/firewall_delete_protection_updates.rs"]
mod firewall_delete_protection_updates;
#[path = "all/firewall_lifecycle.rs"]
mod firewall_lifecycle;
#[path = "all/firewall_policy_associations.rs"]
mod firewall_policy_associations;
#[path = "all/firewall_protection_settings.rs"]
mod firewall_protection_settings;
#[path = "all/flow_operations.rs"]
mod flow_operations;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/logging_configuration.rs"]
mod logging_configuration;
#[path = "all/request_validation_errors.rs"]
mod request_validation_errors;
#[path = "all/read_tag_projection.rs"]
mod read_tag_projection;
#[path = "all/reference_validation.rs"]
mod reference_validation;
#[path = "all/refusal_message_shapes.rs"]
mod refusal_message_shapes;
#[path = "all/resource_policies.rs"]
mod resource_policies;
#[path = "all/resource_policy_action_subsets.rs"]
mod resource_policy_action_subsets;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/response_projections.rs"]
mod response_projections;
#[path = "all/rule_group_capacity_and_resource_policy.rs"]
mod rule_group_capacity_and_resource_policy;
#[path = "all/rule_group_lifecycle.rs"]
mod rule_group_lifecycle;
#[path = "all/rule_group_listing.rs"]
mod rule_group_listing;
#[path = "all/rule_group_mutations.rs"]
mod rule_group_mutations;
#[path = "all/rule_group_policy_associations.rs"]
mod rule_group_policy_associations;
#[path = "all/rule_group_response_members.rs"]
mod rule_group_response_members;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/stateless_default_actions.rs"]
mod stateless_default_actions;
#[path = "all/subnet_associations.rs"]
mod subnet_associations;
#[path = "all/subnet_disassociations.rs"]
mod subnet_disassociations;
#[path = "all/subnet_mapping_minimum.rs"]
mod subnet_mapping_minimum;
#[path = "all/validation_refusal_wording.rs"]
mod validation_refusal_wording;
