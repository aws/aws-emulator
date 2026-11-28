// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `describe_runtime_versions` operation.
//!
//! This file is HAND-WRITTEN at a generated path, and the `@generated` marker is absent on
//! purpose: AWS answers this read with one member the published model snapshot does not
//! declare, and that snapshot is kept EXACTLY as shipped (see
//! `smithy-model/LOCAL-ADDITIONS.md`):
//!
//! * `Language` — the language a canary script on that runtime version is written in, which
//!   every entry of the published catalog carries. The snapshot declares neither the member
//!   nor a shape of that name.
//!
//! So the shape that carries it is generated, like every other modelled shape, from a model
//! file — the small overlay `smithy-model/local-additions.json`, which restates the members
//! the snapshot declares against the snapshot's own targets (so every length constraint is
//! captured exactly as the snapshot spells it) and adds that one besides. The INPUT and
//! OUTPUT shapes are unchanged, so they are still generated from the snapshot: the overlay
//! is named on the one attribute that needs it. Nothing about the wire is hand-written —
//! the member names, the shape ids and the constraints all come from a model.
//!
//! The VALUE of the member is not declared here: it is read off the runtime FAMILY the
//! version name states ([`crate::state::runtime_language`]), so the language an entry
//! reports cannot disagree with the name a caller reads beside it.
//!
//! `tests/all/runtime_catalog.rs` pins the member on the wire and the overlay against the
//! snapshot it restates.
//!
//! Each structure/union is declared exactly once across this service's `types/`
//! modules, and `types/mod.rs` re-exports them all.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-synthetics-synthetics.json");

#[smithy_macros::smithy(shape = "com.amazonaws.synthetics#DescribeRuntimeVersionsRequest")]
#[derive(Debug, Clone, Default)]
pub struct DescribeRuntimeVersionsInput {
    pub next_token: Option<String>,
    pub max_results: Option<i32>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.synthetics#DescribeRuntimeVersionsResponse")]
#[derive(Debug, Clone, Default)]
pub struct DescribeRuntimeVersionsOutput {
    pub runtime_versions: Option<Vec<RuntimeVersion>>,
    pub next_token: Option<String>,
}

/// One canary runtime version of the published catalog.
///
/// Declared from the overlay model, which is where the `Language` member lives: the
/// snapshot declares the four modelled members and nothing of that one.
#[smithy_macros::smithy(
    shape = "com.amazonaws.synthetics#RuntimeVersion",
    model = "smithy-model/local-additions.json"
)]
#[derive(Debug, Clone, Default)]
pub struct RuntimeVersion {
    pub version_name: Option<String>,
    pub description: Option<String>,
    /// The member the snapshot does not declare: the language canaries on this runtime
    /// version are written in.
    pub language: Option<String>,
    pub release_date: Option<::aws_smithy_types::DateTime>,
    pub deprecation_date: Option<::aws_smithy_types::DateTime>,
}
