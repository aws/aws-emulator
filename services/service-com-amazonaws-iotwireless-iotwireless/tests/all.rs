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
#[path = "all/destination_and_wireless_device_lifecycle.rs"]
mod destination_and_wireless_device_lifecycle;
#[path = "all/destination_update_and_not_found.rs"]
mod destination_update_and_not_found;
#[path = "all/device_profile_defaults_and_deletion.rs"]
mod device_profile_defaults_and_deletion;
#[path = "all/device_profile_lifecycle.rs"]
mod device_profile_lifecycle;
#[path = "all/fuota_task_device_association.rs"]
mod fuota_task_device_association;
#[path = "all/fuota_task_device_disassociation.rs"]
mod fuota_task_device_disassociation;
#[path = "all/fuota_task_lifecycle.rs"]
mod fuota_task_lifecycle;
#[path = "all/fuota_task_update_and_delete.rs"]
mod fuota_task_update_and_delete;
#[path = "all/multicast_group_device_association.rs"]
mod multicast_group_device_association;
#[path = "all/multicast_group_device_disassociation.rs"]
mod multicast_group_device_disassociation;
#[path = "all/multicast_group_lifecycle.rs"]
mod multicast_group_lifecycle;
#[path = "all/multicast_group_update_and_delete.rs"]
mod multicast_group_update_and_delete;
#[path = "all/network_analyzer_configuration_lifecycle.rs"]
mod network_analyzer_configuration_lifecycle;
#[path = "all/resource_log_level_overrides.rs"]
mod resource_log_level_overrides;
#[path = "all/resource_event_configuration.rs"]
mod resource_event_configuration;
#[path = "all/service_profile_lifecycle.rs"]
mod service_profile_lifecycle;
#[path = "all/service_profile_listing.rs"]
mod service_profile_listing;
#[path = "all/service_profile_network_server_settings.rs"]
mod service_profile_network_server_settings;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/validation_refusal_envelopes.rs"]
mod validation_refusal_envelopes;
#[path = "all/wireless_device_provisioning_refusals.rs"]
mod wireless_device_provisioning_refusals;
#[path = "all/wireless_device_reported_members.rs"]
mod wireless_device_reported_members;
#[path = "all/wireless_gateway_lifecycle.rs"]
mod wireless_gateway_lifecycle;
#[path = "all/wireless_gateway_task_definition_lifecycle.rs"]
mod wireless_gateway_task_definition_lifecycle;
#[path = "all/wireless_gateway_task_lifecycle.rs"]
mod wireless_gateway_task_lifecycle;
