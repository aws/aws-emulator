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
#[path = "all/chipset_generation_form.rs"]
mod chipset_generation_form;
#[path = "all/gone_image_delete_refusal.rs"]
mod gone_image_delete_refusal;
#[path = "all/image_build_outcome.rs"]
mod image_build_outcome;
#[path = "all/image_build_read_path.rs"]
mod image_build_read_path;
#[path = "all/image_build_scope.rs"]
mod image_build_scope;
#[path = "all/image_configuration_update.rs"]
mod image_configuration_update;
#[path = "all/image_identity_and_version_generations.rs"]
mod image_identity_and_version_generations;
#[path = "all/image_listing.rs"]
mod image_listing;
#[path = "all/image_read_authorization.rs"]
mod image_read_authorization;
#[path = "all/image_refusal_paths.rs"]
mod image_refusal_paths;
#[path = "all/image_request_validation.rs"]
mod image_request_validation;
#[path = "all/image_version_listing.rs"]
mod image_version_listing;
#[path = "all/image_version_read_path.rs"]
mod image_version_read_path;
#[path = "all/image_version_status_publication.rs"]
mod image_version_status_publication;
#[path = "all/list_tags_read_path.rs"]
mod list_tags_read_path;
#[path = "all/managed_base_image_catalogue.rs"]
mod managed_base_image_catalogue;
#[path = "all/managed_base_image_generations.rs"]
mod managed_base_image_generations;
#[path = "all/managed_base_image_versions.rs"]
mod managed_base_image_versions;
#[path = "all/managed_catalogue_publication_metadata.rs"]
mod managed_catalogue_publication_metadata;
#[path = "all/managed_namespace_authorization.rs"]
mod managed_namespace_authorization;
#[path = "all/microvm_endpoint_data_plane.rs"]
mod microvm_endpoint_data_plane;
#[path = "all/microvm_guest_data_plane.rs"]
mod microvm_guest_data_plane;
#[path = "all/microvm_image_and_instance_paths.rs"]
mod microvm_image_and_instance_paths;
#[path = "all/microvm_lifecycle.rs"]
mod microvm_lifecycle;
#[path = "all/microvm_runtime_path.rs"]
mod microvm_runtime_path;
#[path = "all/microvm_suspend_resume.rs"]
mod microvm_suspend_resume;
#[path = "all/recorded_image_identity.rs"]
mod recorded_image_identity;
#[path = "all/region_scoped_resources.rs"]
mod region_scoped_resources;
#[path = "all/resource_tag_removal.rs"]
mod resource_tag_removal;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
