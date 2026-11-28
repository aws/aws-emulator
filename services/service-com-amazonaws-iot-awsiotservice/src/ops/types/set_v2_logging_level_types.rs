// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `set_v2_logging_level` operation.
//!
//! The `logTarget` this request carries names a `LogTargetType`, whose value set on the
//! wire is wider than the published snapshot declares, so this input is declared from this
//! crate's overlay model instead (see `smithy-model/LOCAL-ADDITIONS.md`).
//!
//! Each structure/union is declared exactly once across this service's `types/`
//! modules, and `types/mod.rs` re-exports them all.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-iot-awsiotservice.json");

#[smithy_macros::smithy(
    shape = "com.amazonaws.iot#SetV2LoggingLevelRequest",
    model = "smithy-model/local-additions.json"
)]
#[derive(Debug, Clone, Default)]
pub struct SetV2LoggingLevelInput {
    pub log_target: LogTarget,
    pub log_level: String,
}

