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
#[path = "all/absent_and_default_response_values.rs"]
mod absent_and_default_response_values;
#[path = "all/account_association_lifecycle.rs"]
mod account_association_lifecycle;
#[path = "all/bare_not_found_bodies.rs"]
mod bare_not_found_bodies;
#[path = "all/cloud_connector_absence_wording.rs"]
mod cloud_connector_absence_wording;
#[path = "all/cloud_connector_delete.rs"]
mod cloud_connector_delete;
#[path = "all/cloud_connector_lifecycle.rs"]
mod cloud_connector_lifecycle;
#[path = "all/cloud_connector_update.rs"]
mod cloud_connector_update;
#[path = "all/configuration_write_operations_apply_to_state.rs"]
mod configuration_write_operations_apply_to_state;
#[path = "all/connector_destination_lifecycle.rs"]
mod connector_destination_lifecycle;
#[path = "all/credential_locker_lifecycle.rs"]
mod credential_locker_lifecycle;
#[path = "all/custom_endpoint_registration.rs"]
mod custom_endpoint_registration;
#[path = "all/default_encryption_configuration.rs"]
mod default_encryption_configuration;
#[path = "all/destination_lifecycle.rs"]
mod destination_lifecycle;
#[path = "all/device_discovery_lifecycle.rs"]
mod device_discovery_lifecycle;
#[path = "all/device_discovery_running_phase.rs"]
mod device_discovery_running_phase;
#[path = "all/enum_value_set_listing.rs"]
mod enum_value_set_listing;
#[path = "all/enum_value_set_rendering.rs"]
mod enum_value_set_rendering;
#[path = "all/error_message_wording.rs"]
mod error_message_wording;
#[path = "all/event_log_configuration_lifecycle.rs"]
mod event_log_configuration_lifecycle;
#[path = "all/event_log_configuration_listing.rs"]
mod event_log_configuration_listing;
#[path = "all/event_log_configuration_update_isolation.rs"]
mod event_log_configuration_update_isolation;
#[path = "all/hub_configuration.rs"]
mod hub_configuration;
#[path = "all/managed_thing_capabilities.rs"]
mod managed_thing_capabilities;
#[path = "all/managed_thing_lifecycle.rs"]
mod managed_thing_lifecycle;
#[path = "all/managed_thing_read_side_attributes.rs"]
mod managed_thing_read_side_attributes;
#[path = "all/minted_identifier_opacity.rs"]
mod minted_identifier_opacity;
#[path = "all/not_found_messages.rs"]
mod not_found_messages;
#[path = "all/notification_configuration_lifecycle.rs"]
mod notification_configuration_lifecycle;
#[path = "all/ota_task_configuration_delete_isolation.rs"]
mod ota_task_configuration_delete_isolation;
#[path = "all/ota_task_configuration_lifecycle.rs"]
mod ota_task_configuration_lifecycle;
#[path = "all/ota_task_configuration_not_found_messages.rs"]
mod ota_task_configuration_not_found_messages;
#[path = "all/referenced_resource_validation.rs"]
mod referenced_resource_validation;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/runtime_log_configuration_cycle.rs"]
mod runtime_log_configuration_cycle;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/validation_error_messages.rs"]
mod validation_error_messages;
