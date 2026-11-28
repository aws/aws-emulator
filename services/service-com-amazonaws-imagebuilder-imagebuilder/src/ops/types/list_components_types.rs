// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `list_components` operation.
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
    shape = "com.amazonaws.imagebuilder#ListComponentsRequest",
    model = "smithy-model/local-additions.json"
)]
#[derive(Debug, Clone, Default)]
pub struct ListComponentsInput {
    pub owner: Option<String>,
    pub filters: Option<Vec<Filter>>,
    pub by_name: bool,
    pub max_results: Option<i32>,
    pub next_token: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.imagebuilder#ListComponentsResponse")]
#[derive(Debug, Clone, Default)]
pub struct ListComponentsOutput {
    pub request_id: Option<String>,
    pub component_version_list: Option<Vec<ComponentVersion>>,
    pub next_token: Option<String>,
}

/// The defining characteristics of one component version a listing reports: the members
/// `ComponentVersion` declares, in the model's own member order, and `billable` appended
/// last.
///
/// Member set and order from
/// `smithy_query(query="shape", shape="ComponentVersion")`, plus `billable` — a member the
/// real service reports on every listing entry and the on-disk model omits, which is why the
/// schema and the codec come from `types/unmodelled_member_wire.rs` rather than from the
/// derive (see that module's note). Whether a version is billable is read off the stored
/// component (`crate::state::ComponentRecord::billable`), never fixed here.
#[derive(Debug, Clone, Default)]
pub struct ComponentVersion {
    pub arn: Option<String>,
    pub name: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub platform: Option<String>,
    pub supported_os_versions: Option<Vec<String>>,
    pub r#type: Option<String>,
    pub owner: Option<String>,
    pub date_created: Option<String>,
    pub status: Option<String>,
    pub product_codes: Option<Vec<ProductCodeListItem>>,
    pub billable: Option<bool>,
}

ib_schema_and_codec!(
    ComponentVersion,
    COMPONENT_VERSION_SCHEMA,
    "com.amazonaws.imagebuilder#ComponentVersion",
    "com.amazonaws.imagebuilder",
    "ComponentVersion",
    (M_ARN, arn, "arn", 0, opt, string),
    (M_NAME, name, "name", 1, opt, string),
    (M_VERSION, version, "version", 2, opt, string),
    (M_DESCRIPTION, description, "description", 3, opt, string),
    (M_PLATFORM, platform, "platform", 4, opt, string),
    (
        M_SUPPORTED_OS_VERSIONS,
        supported_os_versions,
        "supportedOsVersions",
        5,
        opt,
        list_of_string
    ),
    (M_TYPE, r#type, "type", 6, opt, string),
    (M_OWNER, owner, "owner", 7, opt, string),
    (M_DATE_CREATED, date_created, "dateCreated", 8, opt, string),
    (M_STATUS, status, "status", 9, opt, string),
    (
        M_PRODUCT_CODES,
        product_codes,
        "productCodes",
        10,
        opt,
        list_of_struct(ProductCodeListItem)
    ),
    (
        M_BILLABLE,
        billable,
        "billable",
        11,
        opt,
        recorded("com.amazonaws.imagebuilder.recorded", boolean)
    ),
);

