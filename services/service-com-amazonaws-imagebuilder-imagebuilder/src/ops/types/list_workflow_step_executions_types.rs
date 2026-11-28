// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `list_workflow_step_executions` operation.
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
    shape = "com.amazonaws.imagebuilder#ListWorkflowStepExecutionsRequest",
    model = "smithy-model/local-additions.json"
)]
#[derive(Debug, Clone, Default)]
pub struct ListWorkflowStepExecutionsInput {
    pub max_results: Option<i32>,
    pub next_token: Option<String>,
    pub workflow_execution_id: String,
}

#[smithy_macros::smithy(shape = "com.amazonaws.imagebuilder#ListWorkflowStepExecutionsResponse")]
#[derive(Debug, Clone, Default)]
pub struct ListWorkflowStepExecutionsOutput {
    pub request_id: Option<String>,
    pub steps: Option<Vec<WorkflowStepMetadata>>,
    pub workflow_build_version_arn: Option<String>,
    pub workflow_execution_id: Option<String>,
    pub image_build_version_arn: Option<String>,
    pub message: Option<String>,
    pub next_token: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.imagebuilder#WorkflowStepMetadata")]
#[derive(Debug, Clone, Default)]
pub struct WorkflowStepMetadata {
    pub step_execution_id: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub action: Option<String>,
    pub status: Option<String>,
    pub rollback_status: Option<String>,
    pub message: Option<String>,
    pub inputs: Option<String>,
    pub outputs: Option<String>,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub attempt_number: Option<i32>,
    pub max_attempts: Option<i32>,
}

