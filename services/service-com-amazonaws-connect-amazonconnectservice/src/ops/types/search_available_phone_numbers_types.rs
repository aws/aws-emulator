// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `search_available_phone_numbers` operation.
//!
//! HAND-WRITTEN, at a generated path, for the reason
//! `claim_phone_number_types.rs` gives: the member lists below are the ones the model
//! snapshot declares, and `PhoneNumberPrefix` carries `skip_validation`, an edit the
//! op-stub generator does not make and would overwrite.
//! `tests/all/phone_number_syntax.rs` pins both halves against the snapshot.
//!
//! `skip_validation = ["PhoneNumberPrefix"]`: the snapshot gives
//! `com.amazonaws.connect#PhoneNumberPrefix` its `@pattern` with every backslash of
//! the published expression escaped a second time, so the trait asks for a literal
//! backslash and refuses every prefix written with the leading `+` the member
//! documents. The expression as published is checked by the handler instead, which
//! answers this operation's own modelled `InvalidParameterException` (see
//! `crate::state::is_phone_number_prefix` and `src/state/phone_number_syntax.rs`).
//!
//! Each structure/union is declared exactly once across this service's `types/`
//! modules, and `types/mod.rs` re-exports them all.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-connect-amazonconnectservice.json");

#[smithy_macros::smithy(
    shape = "com.amazonaws.connect#SearchAvailablePhoneNumbersRequest",
    skip_validation = ["PhoneNumberPrefix"]
)]
#[derive(Debug, Clone, Default)]
pub struct SearchAvailablePhoneNumbersInput {
    pub target_arn: Option<String>,
    pub instance_id: Option<String>,
    pub phone_number_country_code: String,
    pub phone_number_type: String,
    pub phone_number_prefix: Option<String>,
    pub max_results: Option<i32>,
    pub next_token: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.connect#SearchAvailablePhoneNumbersResponse")]
#[derive(Debug, Clone, Default)]
pub struct SearchAvailablePhoneNumbersOutput {
    pub next_token: Option<String>,
    pub available_numbers_list: Option<Vec<AvailableNumberSummary>>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.connect#AvailableNumberSummary")]
#[derive(Debug, Clone, Default)]
pub struct AvailableNumberSummary {
    pub phone_number: Option<String>,
    pub phone_number_country_code: Option<String>,
    pub phone_number_type: Option<String>,
}
