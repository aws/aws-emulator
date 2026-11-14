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
#[path = "all/audit_records.rs"]
mod audit_records;
#[path = "all/compaction_sort_order.rs"]
mod compaction_sort_order;
#[path = "all/constraint_refusals.rs"]
mod constraint_refusals;
#[path = "all/fixed_error_sentences.rs"]
mod fixed_error_sentences;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/iceberg_rest_catalog.rs"]
mod iceberg_rest_catalog;
#[path = "all/invalid_input_refusals.rs"]
mod invalid_input_refusals;
#[path = "all/list_namespaces.rs"]
mod list_namespaces;
#[path = "all/list_tables_summary_members.rs"]
mod list_tables_summary_members;
#[path = "all/metrics_configuration_not_found_message.rs"]
mod metrics_configuration_not_found_message;
#[path = "all/namespace_and_table_lifecycle.rs"]
mod namespace_and_table_lifecycle;
#[path = "all/record_expiration_configuration_reads.rs"]
mod record_expiration_configuration_reads;
#[path = "all/rename_table.rs"]
mod rename_table;
#[path = "all/replication_put_outcome.rs"]
mod replication_put_outcome;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/table_bucket_encryption.rs"]
mod table_bucket_encryption;
#[path = "all/table_bucket_lifecycle.rs"]
mod table_bucket_lifecycle;
#[path = "all/table_bucket_maintenance_configuration.rs"]
mod table_bucket_maintenance_configuration;
#[path = "all/table_bucket_metrics_configuration.rs"]
mod table_bucket_metrics_configuration;
#[path = "all/table_bucket_replication.rs"]
mod table_bucket_replication;
#[path = "all/table_bucket_storage_class.rs"]
mod table_bucket_storage_class;
#[path = "all/table_maintenance_configuration.rs"]
mod table_maintenance_configuration;
#[path = "all/table_record_expiration.rs"]
mod table_record_expiration;
#[path = "all/wire_forms.rs"]
mod wire_forms;
