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
#[path = "all/adapter_listing.rs"]
mod adapter_listing;
#[path = "all/adapter_mutations.rs"]
mod adapter_mutations;
#[path = "all/adapter_versions.rs"]
mod adapter_versions;
#[path = "all/adapters.rs"]
mod adapters;
#[path = "all/authorization.rs"]
mod authorization;
#[path = "all/block_geometry.rs"]
mod block_geometry;
#[path = "all/block_geometry_table_cells.rs"]
mod block_geometry_table_cells;
#[path = "all/block_outlines.rs"]
mod block_outlines;
#[path = "all/creation_time_windows.rs"]
mod creation_time_windows;
#[path = "all/document_extraction.rs"]
mod document_extraction;
#[path = "all/document_formats.rs"]
mod document_formats;
#[path = "all/expense_analysis.rs"]
mod expense_analysis;
#[path = "all/foreign_typeface.rs"]
mod foreign_typeface;
#[path = "all/proportional_and_turned_pages.rs"]
mod proportional_and_turned_pages;
#[path = "all/queries_and_input_refusals.rs"]
mod queries_and_input_refusals;
#[path = "all/raster_documents.rs"]
mod raster_documents;
#[path = "all/refusal_messages.rs"]
mod refusal_messages;
#[path = "all/refusal_wording_for_lookups_and_parameters.rs"]
mod refusal_wording_for_lookups_and_parameters;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/s3_sourced_documents.rs"]
mod s3_sourced_documents;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
