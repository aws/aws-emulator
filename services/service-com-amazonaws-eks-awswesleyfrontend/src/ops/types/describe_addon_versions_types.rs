// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `describe_addon_versions` operation.
//!
//! The `@generated` marker is absent on purpose, and this file is HAND-WRITTEN at a
//! generated path: the published model snapshot beside it is kept EXACTLY as shipped, and
//! the one member whose published constraint is not the one the service enforces —
//! `DescribeAddonVersionsRequest$maxResults`, whose target shape carries a `@range`
//! maximum of a hundred where the recorded service answers a larger page size with the
//! whole catalog — is declared from the small overlay model
//! `smithy-model/local-additions.json` instead (see `smithy-model/LOCAL-ADDITIONS.md`).
//! The overlay restates that structure against the snapshot's own members and query
//! bindings, so the request is read exactly as the snapshot binds it, and only the page
//! size's maximum is dropped — its minimum of one stays, because a page size below one
//! names no page. Nothing about the wire is hand-written: the member names, the shape ids,
//! the query bindings and the remaining constraints all come from a model file.
//!
//! Each structure/union is declared exactly once across this service's `types/`
//! modules, and `types/mod.rs` re-exports them all.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

/// The add-on catalog listing's input, declared from the overlay model — the one shape of
/// this service whose published page-size maximum is not one it enforces (see the module
/// docs).
#[smithy_macros::smithy(
    shape = "com.amazonaws.eks#DescribeAddonVersionsRequest",
    model = "smithy-model/local-additions.json"
)]
#[derive(Debug, Clone, Default)]
pub struct DescribeAddonVersionsInput {
    pub kubernetes_version: Option<String>,
    pub max_results: Option<i32>,
    pub next_token: Option<String>,
    pub addon_name: Option<String>,
    pub types: Option<Vec<String>>,
    pub publishers: Option<Vec<String>>,
    pub owners: Option<Vec<String>>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.eks#DescribeAddonVersionsResponse")]
#[derive(Debug, Clone, Default)]
pub struct DescribeAddonVersionsOutput {
    pub addons: Option<Vec<AddonInfo>>,
    pub next_token: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.eks#AddonInfo")]
#[derive(Debug, Clone, Default)]
pub struct AddonInfo {
    pub addon_name: Option<String>,
    pub r#type: Option<String>,
    pub addon_versions: Option<Vec<AddonVersionInfo>>,
    pub publisher: Option<String>,
    pub owner: Option<String>,
    pub marketplace_information: Option<MarketplaceInformation>,
    pub default_namespace: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.eks#AddonVersionInfo")]
#[derive(Debug, Clone, Default)]
pub struct AddonVersionInfo {
    pub addon_version: Option<String>,
    pub architecture: Option<Vec<String>>,
    pub compute_types: Option<Vec<String>>,
    pub compatibilities: Option<Vec<Compatibility>>,
    pub requires_configuration: bool,
    pub requires_iam_permissions: bool,
}

#[smithy_macros::smithy(shape = "com.amazonaws.eks#Compatibility")]
#[derive(Debug, Clone, Default)]
pub struct Compatibility {
    pub cluster_version: Option<String>,
    pub platform_versions: Option<Vec<String>>,
    pub default_version: bool,
}

