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
#[path = "all/client_error_wire_shape.rs"]
mod client_error_wire_shape;
#[path = "all/cloud_map_core.rs"]
mod cloud_map_core;
#[path = "all/cloud_map_lifecycle.rs"]
mod cloud_map_lifecycle;
#[path = "all/custom_health_status.rs"]
mod custom_health_status;
#[path = "all/delete_namespace.rs"]
mod delete_namespace;
#[path = "all/delete_service.rs"]
mod delete_service;
#[path = "all/discover_instances_revision.rs"]
mod discover_instances_revision;
#[path = "all/discovery_health_views.rs"]
mod discovery_health_views;
#[path = "all/dns_config_defaults.rs"]
mod dns_config_defaults;
#[path = "all/instance_health_status.rs"]
mod instance_health_status;
#[path = "all/instance_operations_missing_service.rs"]
mod instance_operations_missing_service;
#[path = "all/instance_ownership.rs"]
mod instance_ownership;
#[path = "all/invalid_input_message_templates.rs"]
mod invalid_input_message_templates;
#[path = "all/list_instances.rs"]
mod list_instances;
#[path = "all/list_operations.rs"]
mod list_operations;
#[path = "all/namespace_listing.rs"]
mod namespace_listing;
#[path = "all/namespace_properties.rs"]
mod namespace_properties;
#[path = "all/operation_id_form.rs"]
mod operation_id_form;
#[path = "all/public_dns_namespace.rs"]
mod public_dns_namespace;
#[path = "all/resource_ownership.rs"]
mod resource_ownership;
#[path = "all/service_attributes.rs"]
mod service_attributes;
#[path = "all/service_identity_and_error_bodies.rs"]
mod service_identity_and_error_bodies;
#[path = "all/service_listing.rs"]
mod service_listing;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tagging.rs"]
mod tagging;
#[path = "all/update_dns_namespace.rs"]
mod update_dns_namespace;
#[path = "all/update_http_namespace.rs"]
mod update_http_namespace;
#[path = "all/update_service.rs"]
mod update_service;
