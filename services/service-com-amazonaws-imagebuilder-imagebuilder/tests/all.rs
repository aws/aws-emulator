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
#[path = "all/arn_parameter_refusals.rs"]
mod arn_parameter_refusals;
#[path = "all/component_build_version_listing.rs"]
mod component_build_version_listing;
#[path = "all/component_document_sources.rs"]
mod component_document_sources;
#[path = "all/component_image_recipe.rs"]
mod component_image_recipe;
#[path = "all/component_listing.rs"]
mod component_listing;
#[path = "all/component_listing_wire_members.rs"]
mod component_listing_wire_members;
#[path = "all/component_policy.rs"]
mod component_policy;
#[path = "all/component_policy_lifecycle.rs"]
mod component_policy_lifecycle;
#[path = "all/distribution_configuration.rs"]
mod distribution_configuration;
#[path = "all/error_response_envelope.rs"]
mod error_response_envelope;
#[path = "all/image_creation.rs"]
mod image_creation;
#[path = "all/image_packages.rs"]
mod image_packages;
#[path = "all/image_pipeline_execution.rs"]
mod image_pipeline_execution;
#[path = "all/infrastructure_configuration.rs"]
mod infrastructure_configuration;
#[path = "all/infrastructure_configuration_listing.rs"]
mod infrastructure_configuration_listing;
#[path = "all/infrastructure_configuration_update.rs"]
mod infrastructure_configuration_update;
#[path = "all/parameter_value_refusals.rs"]
mod parameter_value_refusals;
#[path = "all/resource_not_found.rs"]
mod resource_not_found;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/semantic_version_bounds.rs"]
mod semantic_version_bounds;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_resource_resource_kinds.rs"]
mod tag_resource_resource_kinds;
#[path = "all/untag_resource.rs"]
mod untag_resource;
#[path = "all/versioned_resource_responses.rs"]
mod versioned_resource_responses;
#[path = "all/waiting_workflow_step_pages.rs"]
mod waiting_workflow_step_pages;
#[path = "all/waiting_workflow_steps.rs"]
mod waiting_workflow_steps;
