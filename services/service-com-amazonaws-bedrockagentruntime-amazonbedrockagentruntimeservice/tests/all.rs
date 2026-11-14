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
#[path = "all/agent_and_flow_invocations.rs"]
mod agent_and_flow_invocations;
#[path = "all/agent_memory.rs"]
mod agent_memory;
#[path = "all/collection_length_refusals.rs"]
mod collection_length_refusals;
#[path = "all/date_time_wire_form.rs"]
mod date_time_wire_form;
#[path = "all/document_content.rs"]
mod document_content;
#[path = "all/inline_agent_invocations.rs"]
mod inline_agent_invocations;
#[path = "all/invocations.rs"]
mod invocations;
#[path = "all/knowledge_base_retrieval.rs"]
mod knowledge_base_retrieval;
#[path = "all/list_sessions.rs"]
mod list_sessions;
#[path = "all/pattern_constraint_messages.rs"]
mod pattern_constraint_messages;
#[path = "all/prompt_optimization.rs"]
mod prompt_optimization;
#[path = "all/pushed_document_retrieval.rs"]
mod pushed_document_retrieval;
#[path = "all/reranking.rs"]
mod reranking;
#[path = "all/retrieve_and_generate.rs"]
mod retrieve_and_generate;
#[path = "all/session_error_messages.rs"]
mod session_error_messages;
#[path = "all/sessions.rs"]
mod sessions;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/streaming_retrieval.rs"]
mod streaming_retrieval;
#[path = "all/tagging.rs"]
mod tagging;
