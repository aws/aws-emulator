// SPDX-License-Identifier: CC0-1.0

//! Type definitions for this service's operation stubs.
//!
//! Each structure/union is declared exactly once. A shape reached by more than
//! one operation lives in `shared_types`; a shape only one operation reaches
//! lives in `<operation>_types`. Every submodule is re-exported here, so
//! `use super::types::*` resolves a type wherever it lives.
//!
//! The `_types` suffix keeps these submodule names disjoint from the operation
//! module names: `ops/mod.rs` declares `pub mod types;` beside one
//! `pub mod <operation>;` per operation and glob-re-exports both, so a submodule
//! here named after an operation would make that operation's own module name
//! ambiguous inside `ops` (`E0659`).
//!
//! HAND-WRITTEN at a generated path, and so NOT re-rendered by a model refresh, for one
//! reason: the declaration of [`error_message_wire`] below. Every exception this service
//! declares is written to the wire with one member more than the on-disk model lists, so
//! those shapes are declared through that module's macros rather than through
//! `#[smithy_macros::smithy]`, and a `macro_rules!` reaches a sibling module only from
//! the parent that pulls it in with `#[macro_use]` AHEAD of them — which is this file. A
//! refresh that adds a shape module therefore has to be followed by its declaration and
//! its re-export here.

// The schema and the serializer every modelled exception of this service uses. Declared
// FIRST, and with `#[macro_use]`, so its macros are in textual scope in every shape module
// below; it declares no type of its own, so there is nothing to re-export from it.
#[macro_use]
mod error_message_wire;

pub mod cancel_gremlin_query_types;
pub mod cancel_index_rebuild_types;
pub mod cancel_loader_job_types;
pub mod cancel_ml_data_processing_job_types;
pub mod cancel_ml_model_training_job_types;
pub mod cancel_ml_model_transform_job_types;
pub mod cancel_open_cypher_query_types;
pub mod create_ml_endpoint_types;
pub mod delete_ml_endpoint_types;
pub mod execute_fast_reset_types;
pub mod execute_gremlin_explain_query_types;
pub mod execute_gremlin_profile_query_types;
pub mod execute_gremlin_query_types;
pub mod execute_open_cypher_explain_query_types;
pub mod execute_open_cypher_query_types;
pub mod get_gremlin_query_status_types;
pub mod get_index_rebuild_status_types;
pub mod get_loader_job_status_types;
pub mod get_ml_data_processing_job_types;
pub mod get_ml_endpoint_types;
pub mod get_ml_model_training_job_types;
pub mod get_ml_model_transform_job_types;
pub mod get_open_cypher_query_status_types;
pub mod get_propertygraph_stream_types;
pub mod get_propertygraph_summary_types;
pub mod get_rdf_graph_summary_types;
pub mod get_sparql_stream_types;
pub mod list_gremlin_queries_types;
pub mod list_loader_jobs_types;
pub mod list_ml_data_processing_jobs_types;
pub mod list_ml_endpoints_types;
pub mod list_ml_model_training_jobs_types;
pub mod list_ml_model_transform_jobs_types;
pub mod list_open_cypher_queries_types;
pub mod manage_propertygraph_statistics_types;
pub mod manage_sparql_statistics_types;
pub mod shared_types;
pub mod start_index_rebuild_types;
pub mod start_loader_job_types;
pub mod start_ml_data_processing_job_types;
pub mod start_ml_model_training_job_types;
pub mod start_ml_model_transform_job_types;

#[allow(ambiguous_glob_reexports, unused_imports)]
pub use cancel_gremlin_query_types::*;
#[allow(ambiguous_glob_reexports, unused_imports)]
pub use cancel_index_rebuild_types::*;
#[allow(ambiguous_glob_reexports, unused_imports)]
pub use cancel_loader_job_types::*;
#[allow(ambiguous_glob_reexports, unused_imports)]
pub use cancel_ml_data_processing_job_types::*;
#[allow(ambiguous_glob_reexports, unused_imports)]
pub use cancel_ml_model_training_job_types::*;
#[allow(ambiguous_glob_reexports, unused_imports)]
pub use cancel_ml_model_transform_job_types::*;
#[allow(ambiguous_glob_reexports, unused_imports)]
pub use cancel_open_cypher_query_types::*;
#[allow(ambiguous_glob_reexports, unused_imports)]
pub use create_ml_endpoint_types::*;
#[allow(ambiguous_glob_reexports, unused_imports)]
pub use delete_ml_endpoint_types::*;
#[allow(ambiguous_glob_reexports, unused_imports)]
pub use execute_fast_reset_types::*;
#[allow(ambiguous_glob_reexports, unused_imports)]
pub use execute_gremlin_explain_query_types::*;
#[allow(ambiguous_glob_reexports, unused_imports)]
pub use execute_gremlin_profile_query_types::*;
#[allow(ambiguous_glob_reexports, unused_imports)]
pub use execute_gremlin_query_types::*;
#[allow(ambiguous_glob_reexports, unused_imports)]
pub use execute_open_cypher_explain_query_types::*;
#[allow(ambiguous_glob_reexports, unused_imports)]
pub use execute_open_cypher_query_types::*;
#[allow(ambiguous_glob_reexports, unused_imports)]
pub use get_gremlin_query_status_types::*;
#[allow(ambiguous_glob_reexports, unused_imports)]
pub use get_index_rebuild_status_types::*;
#[allow(ambiguous_glob_reexports, unused_imports)]
pub use get_loader_job_status_types::*;
#[allow(ambiguous_glob_reexports, unused_imports)]
pub use get_ml_data_processing_job_types::*;
#[allow(ambiguous_glob_reexports, unused_imports)]
pub use get_ml_endpoint_types::*;
#[allow(ambiguous_glob_reexports, unused_imports)]
pub use get_ml_model_training_job_types::*;
#[allow(ambiguous_glob_reexports, unused_imports)]
pub use get_ml_model_transform_job_types::*;
#[allow(ambiguous_glob_reexports, unused_imports)]
pub use get_open_cypher_query_status_types::*;
#[allow(ambiguous_glob_reexports, unused_imports)]
pub use get_propertygraph_stream_types::*;
#[allow(ambiguous_glob_reexports, unused_imports)]
pub use get_propertygraph_summary_types::*;
#[allow(ambiguous_glob_reexports, unused_imports)]
pub use get_rdf_graph_summary_types::*;
#[allow(ambiguous_glob_reexports, unused_imports)]
pub use get_sparql_stream_types::*;
#[allow(ambiguous_glob_reexports, unused_imports)]
pub use list_gremlin_queries_types::*;
#[allow(ambiguous_glob_reexports, unused_imports)]
pub use list_loader_jobs_types::*;
#[allow(ambiguous_glob_reexports, unused_imports)]
pub use list_ml_data_processing_jobs_types::*;
#[allow(ambiguous_glob_reexports, unused_imports)]
pub use list_ml_endpoints_types::*;
#[allow(ambiguous_glob_reexports, unused_imports)]
pub use list_ml_model_training_jobs_types::*;
#[allow(ambiguous_glob_reexports, unused_imports)]
pub use list_ml_model_transform_jobs_types::*;
#[allow(ambiguous_glob_reexports, unused_imports)]
pub use list_open_cypher_queries_types::*;
#[allow(ambiguous_glob_reexports, unused_imports)]
pub use manage_propertygraph_statistics_types::*;
#[allow(ambiguous_glob_reexports, unused_imports)]
pub use manage_sparql_statistics_types::*;
#[allow(ambiguous_glob_reexports, unused_imports)]
pub use shared_types::*;
#[allow(ambiguous_glob_reexports, unused_imports)]
pub use start_index_rebuild_types::*;
#[allow(ambiguous_glob_reexports, unused_imports)]
pub use start_loader_job_types::*;
#[allow(ambiguous_glob_reexports, unused_imports)]
pub use start_ml_data_processing_job_types::*;
#[allow(ambiguous_glob_reexports, unused_imports)]
pub use start_ml_model_training_job_types::*;
#[allow(ambiguous_glob_reexports, unused_imports)]
pub use start_ml_model_transform_job_types::*;

