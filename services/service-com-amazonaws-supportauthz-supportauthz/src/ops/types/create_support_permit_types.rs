// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `create_support_permit` operation.
//!
//! Hand-authored: the scaffold declined `CreateSupportPermit` (see
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

#[smithy_macros::smithy(shape = "com.amazonaws.supportauthz#CreateSupportPermitInput")]
#[derive(Debug, Clone)]
pub struct CreateSupportPermitInput {
    pub name: String,
    pub description: Option<String>,
    pub permit: Permit,
    pub signing_key_info: SigningKeyInfo,
    pub support_case_display_id: Option<String>,
    pub client_token: Option<String>,
    pub tags: Option<std::collections::HashMap<String, String>>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.supportauthz#CreateSupportPermitOutput")]
#[derive(Debug, Clone)]
pub struct CreateSupportPermitOutput {
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

#[smithy_macros::smithy(shape = "com.amazonaws.supportauthz#ServiceQuotaExceededException")]
#[derive(Debug, Clone, Default)]
pub struct ServiceQuotaExceededException {
    pub message: String,
    pub resource_id: String,
    pub resource_type: String,
    pub service_code: String,
    pub quota_code: String,
}
