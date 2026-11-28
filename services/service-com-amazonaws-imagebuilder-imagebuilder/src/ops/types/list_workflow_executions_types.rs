// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `list_workflow_executions` operation.
//!
//! This listing's `maxResults` is held to the page-size maximum Image Builder enforces,
//! which is smaller than the published snapshot's `RestrictedInteger` range, so the input
//! is declared from this crate's overlay model instead (see
//! `smithy-model/LOCAL-ADDITIONS.md`).
//!
//! Each structure/union is declared exactly once across this service's `types/`
//! modules, and `types/mod.rs` re-exports them all.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-imagebuilder-imagebuilder.json");

#[smithy_macros::smithy(
    shape = "com.amazonaws.imagebuilder#ListWorkflowExecutionsRequest",
    model = "smithy-model/local-additions.json"
)]
#[derive(Debug, Clone, Default)]
pub struct ListWorkflowExecutionsInput {
    pub max_results: Option<i32>,
    pub next_token: Option<String>,
    pub image_build_version_arn: String,
}

#[smithy_macros::smithy(shape = "com.amazonaws.imagebuilder#ListWorkflowExecutionsResponse")]
#[derive(Debug, Clone, Default)]
pub struct ListWorkflowExecutionsOutput {
    pub request_id: Option<String>,
    pub workflow_executions: Option<Vec<WorkflowExecutionMetadata>>,
    pub image_build_version_arn: Option<String>,
    pub message: Option<String>,
    pub next_token: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.imagebuilder#WorkflowExecutionMetadata")]
#[derive(Debug, Clone, Default)]
pub struct WorkflowExecutionMetadata {
    pub workflow_build_version_arn: Option<String>,
    pub workflow_execution_id: Option<String>,
    pub r#type: Option<String>,
    pub status: Option<String>,
    pub message: Option<String>,
    pub total_step_count: i32,
    pub total_steps_succeeded: i32,
    pub total_steps_failed: i32,
    pub total_steps_skipped: i32,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub parallel_group: Option<String>,
    pub retried: Option<bool>,
}

