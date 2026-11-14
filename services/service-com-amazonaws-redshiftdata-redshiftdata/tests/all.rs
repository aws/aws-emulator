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
#[path = "all/batch_execute_statement.rs"]
mod batch_execute_statement;

#[path = "all/caller_patience.rs"]
mod caller_patience;

#[path = "all/connection_scope.rs"]
mod connection_scope;

#[path = "all/contended_transactions.rs"]
mod contended_transactions;

#[path = "all/data_api_surface.rs"]
mod data_api_surface;

#[path = "all/emulator.rs"]
mod emulator;

#[path = "all/error_surface.rs"]
mod error_surface;

#[path = "all/list_databases.rs"]
mod list_databases;

#[path = "all/numeric_bounds.rs"]
mod numeric_bounds;

#[path = "all/probe.rs"]
mod probe;

#[path = "all/redshift_dialect.rs"]
mod redshift_dialect;

#[path = "all/refusal_wording.rs"]
mod refusal_wording;

#[path = "all/result_pages.rs"]
mod result_pages;

#[path = "all/session_parameters.rs"]
mod session_parameters;

#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;

#[path = "all/statement_lifecycle.rs"]
mod statement_lifecycle;

#[path = "all/statement_run_states.rs"]
mod statement_run_states;

#[path = "all/statement_status_change_events.rs"]
mod statement_status_change_events;

#[path = "all/statement_submission_surface.rs"]
mod statement_submission_surface;

#[path = "all/statement_wait.rs"]
mod statement_wait;

#[path = "all/unwaited_submit_answer.rs"]
mod unwaited_submit_answer;

#[path = "all/warehouse_catalogue.rs"]
mod warehouse_catalogue;

#[path = "all/warehouse_data_plane.rs"]
mod warehouse_data_plane;
