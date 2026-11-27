// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `decapsulate` operation.
//!
//! Each structure/union is declared exactly once across this service's `types/`
//! modules, and `types/mod.rs` re-exports them all.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-kms-trentservice.json");

#[smithy_macros::smithy(shape = "com.amazonaws.kms#DecapsulateRequest")]
#[derive(Debug, Clone, Default)]
pub struct DecapsulateInput {
    pub key_id: String,
    pub shared_secret_ciphertext_blob: Vec<u8>,
    pub encapsulation_algorithm: String,
    pub grant_tokens: Option<Vec<String>>,
    pub dry_run: Option<bool>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.kms#DecapsulateResponse")]
#[derive(Debug, Clone, Default)]
pub struct DecapsulateOutput {
    pub key_id: Option<String>,
    pub shared_secret: Option<Vec<u8>>,
    pub encapsulation_algorithm: Option<String>,
}
