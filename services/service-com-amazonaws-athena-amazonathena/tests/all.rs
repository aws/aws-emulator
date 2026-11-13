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
#[path = "all/capacity_reservation_lifecycle.rs"]
mod capacity_reservation_lifecycle;

#[path = "all/catalog_statements.rs"]
mod catalog_statements;

#[path = "all/data_catalog_delete.rs"]
mod data_catalog_delete;

#[path = "all/data_catalog_lifecycle.rs"]
mod data_catalog_lifecycle;

#[path = "all/database_statements.rs"]
mod database_statements;

#[path = "all/engine_run_bound.rs"]
mod engine_run_bound;

#[path = "all/harness.rs"]
mod harness;

#[path = "all/hive_table_registration.rs"]
mod hive_table_registration;

#[path = "all/notebook_import_export.rs"]
mod notebook_import_export;

#[path = "all/prepared_statement_lifecycle.rs"]
mod prepared_statement_lifecycle;

#[path = "all/query_execution_identity.rs"]
mod query_execution_identity;

#[path = "all/query_execution_lifecycle.rs"]
mod query_execution_lifecycle;

#[path = "all/query_failure_classification.rs"]
mod query_failure_classification;

#[path = "all/query_result_objects.rs"]
mod query_result_objects;

#[path = "all/query_result_outputs.rs"]
mod query_result_outputs;

#[path = "all/refusal_bodies.rs"]
mod refusal_bodies;

#[path = "all/resource_tags.rs"]
mod resource_tags;

#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;

#[path = "all/table_metadata.rs"]
mod table_metadata;

#[path = "all/work_group_authorization.rs"]
mod work_group_authorization;

#[path = "all/work_group_delete.rs"]
mod work_group_delete;

#[path = "all/work_group_lifecycle.rs"]
mod work_group_lifecycle;

#[path = "all/work_group_listing.rs"]
mod work_group_listing;

#[path = "all/work_group_update.rs"]
mod work_group_update;
