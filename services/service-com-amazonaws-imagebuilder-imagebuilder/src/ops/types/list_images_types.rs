// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `list_images` operation.
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
    shape = "com.amazonaws.imagebuilder#ListImagesRequest",
    model = "smithy-model/local-additions.json"
)]
#[derive(Debug, Clone, Default)]
pub struct ListImagesInput {
    pub owner: Option<String>,
    pub filters: Option<Vec<Filter>>,
    pub by_name: bool,
    pub max_results: Option<i32>,
    pub next_token: Option<String>,
    pub include_deprecated: Option<bool>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.imagebuilder#ListImagesResponse")]
#[derive(Debug, Clone, Default)]
pub struct ListImagesOutput {
    pub request_id: Option<String>,
    pub image_version_list: Option<Vec<ImageVersion>>,
    pub next_token: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.imagebuilder#ImageVersion")]
#[derive(Debug, Clone, Default)]
pub struct ImageVersion {
    pub arn: Option<String>,
    pub name: Option<String>,
    pub r#type: Option<String>,
    pub version: Option<String>,
    pub platform: Option<String>,
    pub os_version: Option<String>,
    pub owner: Option<String>,
    pub date_created: Option<String>,
    pub build_type: Option<String>,
    pub image_source: Option<String>,
}

