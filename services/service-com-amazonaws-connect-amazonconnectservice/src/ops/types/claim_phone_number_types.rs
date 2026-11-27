// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `claim_phone_number` operation.
//!
//! HAND-WRITTEN, at a generated path: the member list below is the one the model
//! snapshot declares for `ClaimPhoneNumberRequest` / `ClaimPhoneNumberResponse`, and
//! `PhoneNumber` carries `skip_validation` — an edit the op-stub generator does not
//! make, and would overwrite. `tests/all/phone_number_syntax.rs` pins both halves
//! against the snapshot, so a refreshed model that adds a member, or spells the
//! member's `@pattern` as published, fails there rather than silently narrowing what
//! this operation accepts.
//!
//! `skip_validation = ["PhoneNumber"]`: the snapshot gives
//! `com.amazonaws.connect#PhoneNumber` its `@pattern` with every backslash of the
//! published expression escaped a second time, so the trait asks for a literal
//! backslash and NO E.164 number satisfies it — at the boundary every claim a caller
//! can make would be refused. The expression as published is checked by the handler
//! instead, which answers this operation's own modelled `InvalidParameterException`
//! (see `crate::state::is_phone_number` and `src/state/phone_number_syntax.rs`).
//!
//! Each structure/union is declared exactly once across this service's `types/`
//! modules, and `types/mod.rs` re-exports them all.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-connect-amazonconnectservice.json");

#[smithy_macros::smithy(
    shape = "com.amazonaws.connect#ClaimPhoneNumberRequest",
    skip_validation = ["PhoneNumber"]
)]
#[derive(Debug, Clone, Default)]
pub struct ClaimPhoneNumberInput {
    pub target_arn: Option<String>,
    pub instance_id: Option<String>,
    pub phone_number: String,
    pub phone_number_description: Option<String>,
    pub tags: Option<std::collections::HashMap<String, String>>,
    pub client_token: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.connect#ClaimPhoneNumberResponse")]
#[derive(Debug, Clone, Default)]
pub struct ClaimPhoneNumberOutput {
    pub phone_number_id: Option<String>,
    pub phone_number_arn: Option<String>,
}
