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
#[path = "all/arn_segment_spelling.rs"]
mod arn_segment_spelling;
#[path = "all/associated_parent_summary.rs"]
mod associated_parent_summary;
#[path = "all/authorization.rs"]
mod authorization;
#[path = "all/catalog_resource_identity.rs"]
mod catalog_resource_identity;
#[path = "all/control_arn_resolution.rs"]
mod control_arn_resolution;
#[path = "all/control_catalog.rs"]
mod control_catalog;
#[path = "all/control_mapping_filter_identity.rs"]
mod control_mapping_filter_identity;
#[path = "all/control_mappings.rs"]
mod control_mappings;
#[path = "all/control_parameters.rs"]
mod control_parameters;
#[path = "all/filter_arn_reading.rs"]
mod filter_arn_reading;
#[path = "all/framework_cross_walk.rs"]
mod framework_cross_walk;
#[path = "all/global_catalog_arn_reading.rs"]
mod global_catalog_arn_reading;
#[path = "all/mapping_type_agreement.rs"]
mod mapping_type_agreement;
#[path = "all/ontology_listing.rs"]
mod ontology_listing;
#[path = "all/pagination_token_spelling.rs"]
mod pagination_token_spelling;
#[path = "all/published_catalog_identity.rs"]
mod published_catalog_identity;
#[path = "all/published_control_identity.rs"]
mod published_control_identity;
#[path = "all/published_control_release_record.rs"]
mod published_control_release_record;
#[path = "all/published_controls.rs"]
mod published_controls;
#[path = "all/published_ontology.rs"]
mod published_ontology;
#[path = "all/refusal_wording.rs"]
mod refusal_wording;
#[path = "all/related_control_relations.rs"]
mod related_control_relations;
#[path = "all/single_resource_filter.rs"]
mod single_resource_filter;
#[path = "all/single_resource_filter_paging.rs"]
mod single_resource_filter_paging;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/validation_messages.rs"]
mod validation_messages;
