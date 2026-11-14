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
#[path = "all/classic_data_plane.rs"]
mod classic_data_plane;
#[path = "all/described_load_balancer_policies.rs"]
mod described_load_balancer_policies;
#[path = "all/documented_rule_refusals.rs"]
mod documented_rule_refusals;
#[path = "all/duplicate_tag_key_refusal.rs"]
mod duplicate_tag_key_refusal;
#[path = "all/elbv2_listener_chain_on_the_shared_endpoint.rs"]
mod elbv2_listener_chain_on_the_shared_endpoint;
#[path = "all/health_check_relation_message_text.rs"]
mod health_check_relation_message_text;
#[path = "all/instance_health_follows_the_backend.rs"]
mod instance_health_follows_the_backend;
#[path = "all/instance_health_probe.rs"]
mod instance_health_probe;
#[path = "all/instance_health_timeline.rs"]
mod instance_health_timeline;
#[path = "all/listeners_health_check_and_attributes.rs"]
mod listeners_health_check_and_attributes;
#[path = "all/load_balancer_lifecycle.rs"]
mod load_balancer_lifecycle;
#[path = "all/load_balancer_policies.rs"]
mod load_balancer_policies;
#[path = "all/load_balancer_tags.rs"]
mod load_balancer_tags;
#[path = "all/modeled_refusal_envelopes.rs"]
mod modeled_refusal_envelopes;
#[path = "all/relayed_network_fault_messages.rs"]
mod relayed_network_fault_messages;
#[path = "all/sample_policy_catalog.rs"]
mod sample_policy_catalog;
#[path = "all/shared_endpoint_api_version_dispatch.rs"]
mod shared_endpoint_api_version_dispatch;
#[path = "all/shared_endpoint_api_versions.rs"]
mod shared_endpoint_api_versions;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/stickiness_policies.rs"]
mod stickiness_policies;
#[path = "all/wire.rs"]
mod wire;
#[path = "all/zone_membership_by_name.rs"]
mod zone_membership_by_name;
