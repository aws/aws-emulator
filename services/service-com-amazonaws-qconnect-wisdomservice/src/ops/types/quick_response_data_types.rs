// SPDX-License-Identifier: CC0-1.0

//! `QuickResponseData`: the quick response `CreateQuickResponse`, `GetQuickResponse` and
//! `UpdateQuickResponse` answer with.
//!
//! This is the ONE `QuickResponseData` type of this crate, so what it declares is what
//! every quick-response write and read reports.
//!
//! It is declared here rather than beside the other shared shapes because AWS answers
//! with one member the published model snapshot does not declare, and that snapshot is
//! kept as shipped (see `smithy-model/LOCAL-ADDITIONS.md`):
//!
//! * `attributeList` — the contact attributes the quick response's content names, served
//!   as the EMPTY list when it names none. The snapshot declares neither the member nor
//!   anything of that name on `QuickResponseData`.
//!
//! So the type is generated, like every other modelled shape, from a model file — the
//! small overlay `smithy-model/local-additions.json`, which restates the nineteen members
//! the snapshot declares, with their own targets so every constraint and the
//! `@timestampFormat` of both timestamps are captured exactly as the snapshot spells
//! them, and adds the twentieth. Nothing about the wire is hand-written: the member
//! names, the shape id and the traits all come from that file.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/local-additions.json");

#[smithy_macros::smithy(shape = "com.amazonaws.qconnect#QuickResponseData")]
#[derive(Debug, Clone)]
pub struct QuickResponseData {
    pub quick_response_arn: String,
    pub quick_response_id: String,
    pub knowledge_base_arn: String,
    pub knowledge_base_id: String,
    pub name: String,
    pub content_type: String,
    pub status: String,
    pub created_time: ::aws_smithy_types::DateTime,
    pub last_modified_time: ::aws_smithy_types::DateTime,
    pub last_modified_region: Option<String>,
    pub contents: Option<QuickResponseContents>,
    pub description: Option<String>,
    pub grouping_configuration: Option<GroupingConfiguration>,
    pub shortcut_key: Option<String>,
    pub last_modified_by: Option<String>,
    pub is_active: Option<bool>,
    pub channels: Option<Vec<String>>,
    pub language: Option<String>,
    pub tags: Option<std::collections::HashMap<String, String>>,
    /// The member the snapshot does not declare: the contact-attribute keys the content
    /// names (`crate::state::QuickResponse::attribute_list`), reported as an empty list
    /// when it names none.
    pub attribute_list: Option<Vec<String>>,
}
