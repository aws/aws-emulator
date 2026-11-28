// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `list_image_pipeline_images` operation.
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
    shape = "com.amazonaws.imagebuilder#ListImagePipelineImagesRequest",
    model = "smithy-model/local-additions.json"
)]
#[derive(Debug, Clone, Default)]
pub struct ListImagePipelineImagesInput {
    pub image_pipeline_arn: String,
    pub filters: Option<Vec<Filter>>,
    pub max_results: Option<i32>,
    pub next_token: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.imagebuilder#ListImagePipelineImagesResponse")]
#[derive(Debug, Clone, Default)]
pub struct ListImagePipelineImagesOutput {
    pub request_id: Option<String>,
    pub image_summary_list: Option<Vec<ImageSummary>>,
    pub next_token: Option<String>,
}

