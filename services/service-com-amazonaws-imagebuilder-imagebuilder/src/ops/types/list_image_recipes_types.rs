// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `list_image_recipes` operation.
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
    shape = "com.amazonaws.imagebuilder#ListImageRecipesRequest",
    model = "smithy-model/local-additions.json"
)]
#[derive(Debug, Clone, Default)]
pub struct ListImageRecipesInput {
    pub owner: Option<String>,
    pub filters: Option<Vec<Filter>>,
    pub max_results: Option<i32>,
    pub next_token: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.imagebuilder#ListImageRecipesResponse")]
#[derive(Debug, Clone, Default)]
pub struct ListImageRecipesOutput {
    pub request_id: Option<String>,
    pub image_recipe_summary_list: Option<Vec<ImageRecipeSummary>>,
    pub next_token: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.imagebuilder#ImageRecipeSummary")]
#[derive(Debug, Clone, Default)]
pub struct ImageRecipeSummary {
    pub arn: Option<String>,
    pub name: Option<String>,
    pub platform: Option<String>,
    pub owner: Option<String>,
    pub parent_image: Option<String>,
    pub date_created: Option<String>,
    pub tags: Option<std::collections::HashMap<String, String>>,
}

