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
#[path = "all/attachment_persistence.rs"]
mod attachment_persistence;
#[path = "all/availability_zone_catalog.rs"]
mod availability_zone_catalog;
#[path = "all/catalog_fidelity.rs"]
mod catalog_fidelity;
#[path = "all/client_token_single_use.rs"]
mod client_token_single_use;
#[path = "all/create_tag_specifications.rs"]
mod create_tag_specifications;
#[path = "all/ec2_managed_instance_propagation.rs"]
mod ec2_managed_instance_propagation;
#[path = "all/idempotency_token_reuse.rs"]
mod idempotency_token_reuse;
#[path = "all/instance_configuration_validation.rs"]
mod instance_configuration_validation;
#[path = "all/instance_deletion_lifecycle.rs"]
mod instance_deletion_lifecycle;
#[path = "all/instance_placement_zone.rs"]
mod instance_placement_zone;
#[path = "all/instance_token_parameter_mismatch.rs"]
mod instance_token_parameter_mismatch;
#[path = "all/instance_type_catalog.rs"]
mod instance_type_catalog;
#[path = "all/instance_type_configuration_completeness.rs"]
mod instance_type_configuration_completeness;
#[path = "all/launch_image_resolution.rs"]
mod launch_image_resolution;
#[path = "all/list_pagination_resume_points.rs"]
mod list_pagination_resume_points;
#[path = "all/next_token_canonical_spelling.rs"]
mod next_token_canonical_spelling;
#[path = "all/next_token_rule_sharing.rs"]
mod next_token_rule_sharing;
#[path = "all/pagination_rule_sharing.rs"]
mod pagination_rule_sharing;
#[path = "all/region_catalog.rs"]
mod region_catalog;
#[path = "all/regional_scoping.rs"]
mod regional_scoping;
#[path = "all/reserved_tag_keys.rs"]
mod reserved_tag_keys;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_capacity_rule_sharing.rs"]
mod tag_capacity_rule_sharing;
#[path = "all/tag_list_merge_rule.rs"]
mod tag_list_merge_rule;
#[path = "all/tag_removal.rs"]
mod tag_removal;
#[path = "all/validation_messages.rs"]
mod validation_messages;
#[path = "all/volume_attachment.rs"]
mod volume_attachment;
#[path = "all/volume_configuration_validation.rs"]
mod volume_configuration_validation;
#[path = "all/volume_lifecycle.rs"]
mod volume_lifecycle;
#[path = "all/volume_modification.rs"]
mod volume_modification;
#[path = "all/volume_type_rule_sharing.rs"]
mod volume_type_rule_sharing;
#[path = "all/volume_zone_affinity.rs"]
mod volume_zone_affinity;
#[path = "all/wire_error_responses.rs"]
mod wire_error_responses;
#[path = "all/workspace_instance_lifecycle.rs"]
mod workspace_instance_lifecycle;
