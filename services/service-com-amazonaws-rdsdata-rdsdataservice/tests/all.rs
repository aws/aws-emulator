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
#[path = "all/call_preconditions.rs"]
mod call_preconditions;
#[path = "all/ended_transaction_sentence.rs"]
mod ended_transaction_sentence;
#[path = "all/engine_dialects.rs"]
mod engine_dialects;
#[path = "all/engine_release_floor.rs"]
mod engine_release_floor;
#[path = "all/exact_numeric_results.rs"]
mod exact_numeric_results;
#[path = "all/idle_transaction_expiry.rs"]
mod idle_transaction_expiry;
#[path = "all/json_record_values.rs"]
mod json_record_values;
#[path = "all/malformed_transaction_ids.rs"]
mod malformed_transaction_ids;
#[path = "all/mysql_upsert.rs"]
mod mysql_upsert;
#[path = "all/probe.rs"]
mod probe;
#[path = "all/read_committed_transactions.rs"]
mod read_committed_transactions;
#[path = "all/refusal_classes.rs"]
mod refusal_classes;
#[path = "all/sql_data_api.rs"]
mod sql_data_api;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/statement_names.rs"]
mod statement_names;
#[path = "all/transaction_id_classes.rs"]
mod transaction_id_classes;
#[path = "all/typed_values.rs"]
mod typed_values;
