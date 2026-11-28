// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `list_workflow_build_versions` operation.
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
    shape = "com.amazonaws.imagebuilder#ListWorkflowBuildVersionsRequest",
    model = "smithy-model/local-additions.json"
)]
#[derive(Debug, Clone, Default)]
pub struct ListWorkflowBuildVersionsInput {
    pub workflow_version_arn: Option<String>,
    pub max_results: Option<i32>,
    pub next_token: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.imagebuilder#ListWorkflowBuildVersionsResponse")]
#[derive(Debug, Clone, Default)]
pub struct ListWorkflowBuildVersionsOutput {
    pub workflow_summary_list: Option<Vec<WorkflowSummary>>,
    pub next_token: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.imagebuilder#WorkflowSummary")]
#[derive(Debug, Clone, Default)]
pub struct WorkflowSummary {
    pub arn: Option<String>,
    pub name: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub change_description: Option<String>,
    pub r#type: Option<String>,
    pub owner: Option<String>,
    pub state: Option<WorkflowState>,
    pub date_created: Option<String>,
    pub tags: Option<std::collections::HashMap<String, String>>,
}

