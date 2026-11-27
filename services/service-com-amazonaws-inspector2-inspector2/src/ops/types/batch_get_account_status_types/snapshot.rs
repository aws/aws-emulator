// SPDX-License-Identifier: CC0-1.0

//! The shapes `batch_get_account_status` reaches, as the published model declares them.
//!
//! `State` is NOT here: AWS answers with it narrower than the snapshot declares, so it is
//! generated from this crate's overlay model in [`super::overlay`] (see
//! `smithy-model/LOCAL-ADDITIONS.md`). The structures that carry one — `AccountState` and
//! `ResourceState` — are the snapshot's own: codegen takes a structure member's shape from
//! the member's own Rust type, so they name the overlay's `State` without restating it.
#![allow(unused_imports)]

use super::*;
// `FailedAccount` is reached by more than one operation, so it lives in `shared_types`;
// this module names it rather than restating it.
use crate::ops::types::FailedAccount;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-inspector2-inspector2.json");

#[smithy_macros::smithy(shape = "com.amazonaws.inspector2#BatchGetAccountStatusRequest")]
#[derive(Debug, Clone, Default)]
pub struct BatchGetAccountStatusInput {
    pub account_ids: Option<Vec<String>>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.inspector2#BatchGetAccountStatusResponse")]
#[derive(Debug, Clone, Default)]
pub struct BatchGetAccountStatusOutput {
    pub accounts: Vec<AccountState>,
    pub failed_accounts: Option<Vec<FailedAccount>>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.inspector2#AccountState")]
#[derive(Debug, Clone, Default)]
pub struct AccountState {
    pub account_id: String,
    pub state: State,
    pub resource_state: ResourceState,
}

#[smithy_macros::smithy(shape = "com.amazonaws.inspector2#ResourceState")]
#[derive(Debug, Clone, Default)]
pub struct ResourceState {
    pub ec2: State,
    pub ecr: State,
    pub lambda: Option<State>,
    pub lambda_code: Option<State>,
    pub code_repository: Option<State>,
    pub ami: Option<State>,
}
