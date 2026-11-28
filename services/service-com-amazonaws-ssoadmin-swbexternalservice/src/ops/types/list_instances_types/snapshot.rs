// SPDX-License-Identifier: CC0-1.0

//! The shapes `list_instances` reaches, as the upstream model declares them.
//!
//! Every shape the operation reaches except the instance view its entries carry, which
//! this service answers with the `State` member the upstream model declares nowhere and
//! which is therefore declared in [`super::overlay`].
#![allow(unused_imports)]

use super::super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-ssoadmin-swbexternalservice.json");

#[smithy_macros::smithy(shape = "com.amazonaws.ssoadmin#ListInstancesRequest")]
#[derive(Debug, Clone, Default)]
pub struct ListInstancesInput {
    pub max_results: Option<i32>,
    pub next_token: Option<String>,
    pub region_scope: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.ssoadmin#ListInstancesResponse")]
#[derive(Debug, Clone, Default)]
pub struct ListInstancesOutput {
    pub instances: Option<Vec<InstanceMetadata>>,
    pub next_token: Option<String>,
}
