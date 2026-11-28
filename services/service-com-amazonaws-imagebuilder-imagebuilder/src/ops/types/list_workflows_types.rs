// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `list_workflows` operation.
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
    shape = "com.amazonaws.imagebuilder#ListWorkflowsRequest",
    model = "smithy-model/local-additions.json"
)]
#[derive(Debug, Clone, Default)]
pub struct ListWorkflowsInput {
    pub owner: Option<String>,
    pub filters: Option<Vec<Filter>>,
    pub by_name: bool,
    pub max_results: Option<i32>,
    pub next_token: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.imagebuilder#ListWorkflowsResponse")]
#[derive(Debug, Clone, Default)]
pub struct ListWorkflowsOutput {
    pub workflow_version_list: Option<Vec<WorkflowVersion>>,
    pub next_token: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.imagebuilder#WorkflowVersion")]
#[derive(Debug, Clone, Default)]
pub struct WorkflowVersion {
    pub arn: Option<String>,
    pub name: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub r#type: Option<String>,
    pub owner: Option<String>,
    pub date_created: Option<String>,
}

