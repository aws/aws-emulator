// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `list_v2_logging_levels` operation.
//!
//! The `targetType` this request can carry is a `LogTargetType`, whose value set on the
//! wire is wider than the published snapshot declares, so this input is declared from this
//! crate's overlay model instead (see `smithy-model/LOCAL-ADDITIONS.md`).
//!
//! Each structure/union is declared exactly once across this service's `types/`
//! modules, and `types/mod.rs` re-exports them all.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-iot-awsiotservice.json");

#[smithy_macros::smithy(
    shape = "com.amazonaws.iot#ListV2LoggingLevelsRequest",
    model = "smithy-model/local-additions.json"
)]
#[derive(Debug, Clone, Default)]
pub struct ListV2LoggingLevelsInput {
    pub target_type: Option<String>,
    pub next_token: Option<String>,
    pub max_results: Option<i32>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.iot#ListV2LoggingLevelsResponse")]
#[derive(Debug, Clone, Default)]
pub struct ListV2LoggingLevelsOutput {
    pub log_target_configurations: Option<Vec<LogTargetConfiguration>>,
    pub next_token: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.iot#LogTargetConfiguration")]
#[derive(Debug, Clone, Default)]
pub struct LogTargetConfiguration {
    pub log_target: Option<LogTarget>,
    pub log_level: Option<String>,
}

