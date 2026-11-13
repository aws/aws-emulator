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
#[path = "all/ab_test_lifecycle.rs"]
mod ab_test_lifecycle;
#[path = "all/ab_test_support.rs"]
mod ab_test_support;
#[path = "all/ab_tests.rs"]
mod ab_tests;
#[path = "all/batch_evaluations.rs"]
mod batch_evaluations;
#[path = "all/browser_sessions.rs"]
mod browser_sessions;
#[path = "all/code_interpreter_session_stop.rs"]
mod code_interpreter_session_stop;
#[path = "all/code_interpreter_sessions.rs"]
mod code_interpreter_sessions;
#[path = "all/code_interpreter_tools.rs"]
mod code_interpreter_tools;
#[path = "all/evaluation_result_context.rs"]
mod evaluation_result_context;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/memory_actors.rs"]
mod memory_actors;
#[path = "all/memory_events.rs"]
mod memory_events;
#[path = "all/memory_extraction_jobs.rs"]
mod memory_extraction_jobs;
#[path = "all/memory_ingestion.rs"]
mod memory_ingestion;
#[path = "all/memory_records.rs"]
mod memory_records;
#[path = "all/on_demand_evaluation.rs"]
mod on_demand_evaluation;
#[path = "all/parent_existence_preconditions.rs"]
mod parent_existence_preconditions;
#[path = "all/payment_sessions.rs"]
mod payment_sessions;
#[path = "all/resource_not_found_messages.rs"]
mod resource_not_found_messages;
#[path = "all/service_claim.rs"]
mod service_claim;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/validation_constraint_wording.rs"]
mod validation_constraint_wording;
#[path = "all/validation_range_wording.rs"]
mod validation_range_wording;
#[path = "all/workload_access_tokens.rs"]
mod workload_access_tokens;
