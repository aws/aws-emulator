// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `get_support_permit` operation.
//!
//! Hand-authored: the scaffold declined `GetSupportPermit` (see
//! `src/ops/SKIPPED_STUBS.md`), so these follow the shape the scaffold gives the
//! rendered siblings in this directory. Each structure/union is declared exactly
//! once across this service's `types/` modules.
//!
//! This module is declared from `src/ops/types_local/mod.rs`, which re-exports it,
//! rather than from the `@generated` `types/mod.rs` that the model update rewrites:
//! see that file.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-supportauthz-supportauthz.json");

#[smithy_macros::smithy(shape = "com.amazonaws.supportauthz#GetSupportPermitInput")]
#[derive(Debug, Clone, Default)]
pub struct GetSupportPermitInput {
    pub support_permit_identifier: String,
}

#[smithy_macros::smithy(shape = "com.amazonaws.supportauthz#GetSupportPermitOutput")]
#[derive(Debug, Clone)]
pub struct GetSupportPermitOutput {
    pub name: String,
    pub arn: String,
    pub description: Option<String>,
    pub permit: Permit,
    pub status: String,
    pub signing_key_info: SigningKeyInfo,
    pub created_at: ::aws_smithy_types::DateTime,
    pub support_case_display_id: Option<String>,
    pub tags: Option<std::collections::HashMap<String, String>>,
}
