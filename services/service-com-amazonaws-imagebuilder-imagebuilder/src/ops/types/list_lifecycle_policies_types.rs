// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `list_lifecycle_policies` operation.
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
    shape = "com.amazonaws.imagebuilder#ListLifecyclePoliciesRequest",
    model = "smithy-model/local-additions.json"
)]
#[derive(Debug, Clone, Default)]
pub struct ListLifecyclePoliciesInput {
    pub filters: Option<Vec<Filter>>,
    pub max_results: Option<i32>,
    pub next_token: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.imagebuilder#ListLifecyclePoliciesResponse")]
#[derive(Debug, Clone, Default)]
pub struct ListLifecyclePoliciesOutput {
    pub lifecycle_policy_summary_list: Option<Vec<LifecyclePolicySummary>>,
    pub next_token: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.imagebuilder#LifecyclePolicySummary")]
#[derive(Debug, Clone, Default)]
pub struct LifecyclePolicySummary {
    pub arn: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub status: Option<String>,
    pub execution_role: Option<String>,
    pub resource_type: Option<String>,
    pub date_created: Option<::aws_smithy_types::DateTime>,
    pub date_updated: Option<::aws_smithy_types::DateTime>,
    pub date_last_run: Option<::aws_smithy_types::DateTime>,
    pub tags: Option<std::collections::HashMap<String, String>>,
}

