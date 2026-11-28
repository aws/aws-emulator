// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `list_waiting_workflow_steps` operation.
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
    shape = "com.amazonaws.imagebuilder#ListWaitingWorkflowStepsRequest",
    model = "smithy-model/local-additions.json"
)]
#[derive(Debug, Clone, Default)]
pub struct ListWaitingWorkflowStepsInput {
    pub max_results: Option<i32>,
    pub next_token: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.imagebuilder#ListWaitingWorkflowStepsResponse")]
#[derive(Debug, Clone, Default)]
pub struct ListWaitingWorkflowStepsOutput {
    pub steps: Option<Vec<WorkflowStepExecution>>,
    pub next_token: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.imagebuilder#WorkflowStepExecution")]
#[derive(Debug, Clone, Default)]
pub struct WorkflowStepExecution {
    pub step_execution_id: Option<String>,
    pub image_build_version_arn: Option<String>,
    pub workflow_execution_id: Option<String>,
    pub workflow_build_version_arn: Option<String>,
    pub name: Option<String>,
    pub action: Option<String>,
    pub start_time: Option<String>,
}

