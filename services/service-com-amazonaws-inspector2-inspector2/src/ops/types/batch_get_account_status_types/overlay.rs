// SPDX-License-Identifier: CC0-1.0

//! `State`, declared from this crate's overlay model.
//!
//! AWS reports an account's Amazon Inspector posture — the account's own and one per scan
//! type — as a `State` carrying the `status` in force and NEITHER of `errorCode` and
//! `errorMessage`: those two explain why an account FAILED to enable Amazon Inspector, and
//! `ErrorCode` admits no token for "nothing failed", so a posture with no failure behind it
//! carries neither. An account the caller may not read is reported per entry in
//! `failedAccounts`, where both members are genuinely required.
//!
//! The published model marks all three `@required`. The snapshot is kept exactly as shipped
//! (see `smithy-model/LOCAL-ADDITIONS.md`), so this one shape is generated from the small
//! overlay model `smithy-model/local-additions.json`, which restates it with those two
//! members optional against the snapshot's own member targets. Nothing about the wire is
//! hand-written: the shape is still declared by `#[smithy_macros::smithy]` from a model.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/local-additions.json");

/// The state of Amazon Inspector scanning this service holds, for one account or one of its
/// scan types.
///
/// `error_code` and `error_message` are `None` for every posture this service reports: no
/// account of this emulator fails to enable, and the one per-account failure it does
/// report — a caller with no access to the account — is reported as a `FailedAccount`
/// rather than as a `State`.
#[smithy_macros::smithy(shape = "com.amazonaws.inspector2#State")]
#[derive(Debug, Clone, Default)]
pub struct State {
    pub status: String,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}
