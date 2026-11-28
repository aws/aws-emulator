// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `delete_view` operation.
//!
//! The `name` this request carries is a `ViewName`, whose class on the wire is wider than
//! the published snapshot declares, so this input is declared from this crate's overlay
//! model instead (see `smithy-model/LOCAL-ADDITIONS.md`).
//!
//! Each structure/union is declared exactly once across this service's `types/`
//! modules, and `types/mod.rs` re-exports them all.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-cloudwatchomni-cloudwatchomnifrontend.json");

#[smithy_macros::smithy(
    shape = "com.amazonaws.cloudwatchomni#DeleteViewRequest",
    model = "smithy-model/local-additions.json"
)]
#[derive(Debug, Clone, Default)]
pub struct DeleteViewInput {
    pub name: String,
}

#[smithy_macros::smithy(shape = "com.amazonaws.cloudwatchomni#DeleteViewResponse")]
#[derive(Debug, Clone, Default)]
pub struct DeleteViewOutput {
}

