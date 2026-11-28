// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `list_infrastructure_configurations` operation.
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
    shape = "com.amazonaws.imagebuilder#ListInfrastructureConfigurationsRequest",
    model = "smithy-model/local-additions.json"
)]
#[derive(Debug, Clone, Default)]
pub struct ListInfrastructureConfigurationsInput {
    pub filters: Option<Vec<Filter>>,
    pub max_results: Option<i32>,
    pub next_token: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.imagebuilder#ListInfrastructureConfigurationsResponse")]
#[derive(Debug, Clone, Default)]
pub struct ListInfrastructureConfigurationsOutput {
    pub request_id: Option<String>,
    pub infrastructure_configuration_summary_list: Option<Vec<InfrastructureConfigurationSummary>>,
    pub next_token: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.imagebuilder#InfrastructureConfigurationSummary")]
#[derive(Debug, Clone, Default)]
pub struct InfrastructureConfigurationSummary {
    pub arn: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub date_created: Option<String>,
    pub date_updated: Option<String>,
    pub resource_tags: Option<std::collections::HashMap<String, String>>,
    pub tags: Option<std::collections::HashMap<String, String>>,
    pub instance_types: Option<Vec<String>>,
    pub instance_profile_name: Option<String>,
    pub placement: Option<Placement>,
}

