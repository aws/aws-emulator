// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `list_lifecycle_execution_resources` operation.
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
    shape = "com.amazonaws.imagebuilder#ListLifecycleExecutionResourcesRequest",
    model = "smithy-model/local-additions.json"
)]
#[derive(Debug, Clone, Default)]
pub struct ListLifecycleExecutionResourcesInput {
    pub lifecycle_execution_id: String,
    pub parent_resource_id: Option<String>,
    pub max_results: Option<i32>,
    pub next_token: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.imagebuilder#ListLifecycleExecutionResourcesResponse")]
#[derive(Debug, Clone, Default)]
pub struct ListLifecycleExecutionResourcesOutput {
    pub lifecycle_execution_id: Option<String>,
    pub lifecycle_execution_state: Option<LifecycleExecutionState>,
    pub resources: Option<Vec<LifecycleExecutionResource>>,
    pub next_token: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.imagebuilder#LifecycleExecutionResource")]
#[derive(Debug, Clone, Default)]
pub struct LifecycleExecutionResource {
    pub account_id: Option<String>,
    pub resource_id: Option<String>,
    pub state: Option<LifecycleExecutionResourceState>,
    pub action: Option<LifecycleExecutionResourceAction>,
    pub region: Option<String>,
    pub snapshots: Option<Vec<LifecycleExecutionSnapshotResource>>,
    pub image_uris: Option<Vec<String>>,
    pub start_time: Option<::aws_smithy_types::DateTime>,
    pub end_time: Option<::aws_smithy_types::DateTime>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.imagebuilder#LifecycleExecutionResourceState")]
#[derive(Debug, Clone, Default)]
pub struct LifecycleExecutionResourceState {
    pub status: Option<String>,
    pub reason: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.imagebuilder#LifecycleExecutionResourceAction")]
#[derive(Debug, Clone, Default)]
pub struct LifecycleExecutionResourceAction {
    pub name: Option<String>,
    pub reason: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.imagebuilder#LifecycleExecutionSnapshotResource")]
#[derive(Debug, Clone, Default)]
pub struct LifecycleExecutionSnapshotResource {
    pub snapshot_id: Option<String>,
    pub state: Option<LifecycleExecutionResourceState>,
}

