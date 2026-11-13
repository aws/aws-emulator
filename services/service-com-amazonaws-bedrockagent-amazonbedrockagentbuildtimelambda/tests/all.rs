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
#[path = "all/agent_lifecycle.rs"]
mod agent_lifecycle;
#[path = "all/agents_classic_maintenance_mode.rs"]
mod agents_classic_maintenance_mode;
#[path = "all/client_error_fidelity.rs"]
mod client_error_fidelity;
#[path = "all/data_source_lifecycle.rs"]
mod data_source_lifecycle;
#[path = "all/delete_removes_the_resource.rs"]
mod delete_removes_the_resource;
#[path = "all/enum_value_set_order.rs"]
mod enum_value_set_order;
#[path = "all/flow_alias_deployment.rs"]
mod flow_alias_deployment;
#[path = "all/flow_definition_validation.rs"]
mod flow_definition_validation;
#[path = "all/flow_deletion_cascade.rs"]
mod flow_deletion_cascade;
#[path = "all/flow_draft_updates.rs"]
mod flow_draft_updates;
#[path = "all/flow_lifecycle.rs"]
mod flow_lifecycle;
#[path = "all/flow_preparation.rs"]
mod flow_preparation;
#[path = "all/flow_version_listing.rs"]
mod flow_version_listing;
#[path = "all/flow_version_minting.rs"]
mod flow_version_minting;
#[path = "all/flow_version_prepare_ordering.rs"]
mod flow_version_prepare_ordering;
#[path = "all/flow_version_reads.rs"]
mod flow_version_reads;
#[path = "all/flow_version_snapshots.rs"]
mod flow_version_snapshots;
#[path = "all/immutable_configuration_members.rs"]
mod immutable_configuration_members;
#[path = "all/ingestion_index.rs"]
mod ingestion_index;
#[path = "all/ingestion_crawl.rs"]
mod ingestion_crawl;
#[path = "all/ingestion_job_lifecycle.rs"]
mod ingestion_job_lifecycle;
#[path = "all/knowledge_base_document_kinds_and_deletion.rs"]
mod knowledge_base_document_kinds_and_deletion;
#[path = "all/knowledge_base_document_listing.rs"]
mod knowledge_base_document_listing;
#[path = "all/knowledge_base_documents.rs"]
mod knowledge_base_documents;
#[path = "all/knowledge_base_lifecycle.rs"]
mod knowledge_base_lifecycle;
#[path = "all/knowledge_base_listing.rs"]
mod knowledge_base_listing;
#[path = "all/managed_connector_defaults.rs"]
mod managed_connector_defaults;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/update_reports_the_transitional_status.rs"]
mod update_reports_the_transitional_status;
#[path = "all/update_round_trip.rs"]
mod update_round_trip;
#[path = "all/vector_store_binding.rs"]
mod vector_store_binding;
#[path = "all/vector_store_fixture.rs"]
mod vector_store_fixture;
