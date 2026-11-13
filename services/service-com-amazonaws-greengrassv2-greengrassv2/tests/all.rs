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
#[path = "all/client_device_disassociation.rs"]
mod client_device_disassociation;
#[path = "all/component_publish_and_deploy.rs"]
mod component_publish_and_deploy;
#[path = "all/component_state_recipe_and_deployment_reads.rs"]
mod component_state_recipe_and_deployment_reads;
#[path = "all/component_versions_and_resolution.rs"]
mod component_versions_and_resolution;
#[path = "all/core_device_data_plane_authorization.rs"]
mod core_device_data_plane_authorization;
#[path = "all/core_device_fleet.rs"]
mod core_device_fleet;
#[path = "all/deployment_cancellation.rs"]
mod deployment_cancellation;
#[path = "all/deployment_history_and_tag_removal.rs"]
mod deployment_history_and_tag_removal;
#[path = "all/identity_authorization.rs"]
mod identity_authorization;
#[path = "all/not_found_refusal_class_per_route.rs"]
mod not_found_refusal_class_per_route;
#[path = "all/resource_tag_writes.rs"]
mod resource_tag_writes;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/service_role_association.rs"]
mod service_role_association;
#[path = "all/service_role_read_is_a_pure_report.rs"]
mod service_role_read_is_a_pure_report;
#[path = "all/shared_endpoint_routing.rs"]
mod shared_endpoint_routing;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
