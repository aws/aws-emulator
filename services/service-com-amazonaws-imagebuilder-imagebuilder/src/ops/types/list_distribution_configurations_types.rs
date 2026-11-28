// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `list_distribution_configurations` operation.
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
    shape = "com.amazonaws.imagebuilder#ListDistributionConfigurationsRequest",
    model = "smithy-model/local-additions.json"
)]
#[derive(Debug, Clone, Default)]
pub struct ListDistributionConfigurationsInput {
    pub filters: Option<Vec<Filter>>,
    pub max_results: Option<i32>,
    pub next_token: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.imagebuilder#ListDistributionConfigurationsResponse")]
#[derive(Debug, Clone, Default)]
pub struct ListDistributionConfigurationsOutput {
    pub request_id: Option<String>,
    pub distribution_configuration_summary_list: Option<Vec<DistributionConfigurationSummary>>,
    pub next_token: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.imagebuilder#DistributionConfigurationSummary")]
#[derive(Debug, Clone, Default)]
pub struct DistributionConfigurationSummary {
    pub arn: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub date_created: Option<String>,
    pub date_updated: Option<String>,
    pub tags: Option<std::collections::HashMap<String, String>>,
    pub regions: Option<Vec<String>>,
}

