// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `list_lifecycle_executions` operation.
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
    shape = "com.amazonaws.imagebuilder#ListLifecycleExecutionsRequest",
    model = "smithy-model/local-additions.json"
)]
#[derive(Debug, Clone, Default)]
pub struct ListLifecycleExecutionsInput {
    pub max_results: Option<i32>,
    pub next_token: Option<String>,
    pub resource_arn: String,
}

#[smithy_macros::smithy(shape = "com.amazonaws.imagebuilder#ListLifecycleExecutionsResponse")]
#[derive(Debug, Clone, Default)]
pub struct ListLifecycleExecutionsOutput {
    pub lifecycle_executions: Option<Vec<LifecycleExecution>>,
    pub next_token: Option<String>,
}

