// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `delete_account_password_policy` operation.
//!
//! HAND-AUTHORED: the scaffold op-stub generator declined to emit a stub for
//! `DeleteAccountPasswordPolicy` because its input is `smithy.api#Unit`, which it does not
//! type yet (see `src/ops/SKIPPED_STUBS.md`), so these shapes are written out here in the
//! same shape as a rendered sibling — exactly as `get_account_password_policy_types.rs` was
//! written for the READ of the same account record.
//!
//! The operation's OUTPUT is a unit too, and a unit output is declared beside the operation
//! that answers it (`src/ops/delete_account_password_policy.rs`), exactly as every rendered
//! sibling of this service with a unit output declares its own.
//!
//! Each structure/union is declared exactly once across this service's `types/`
//! modules, and `types/mod.rs` re-exports them all.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-iam-awsidentitymanagementv20100508.json");

/// `DeleteAccountPasswordPolicy` takes no input: the operation's modelled input is
/// `smithy.api#Unit`. The empty shape still carries a schema, a serializer and a
/// deserializer, because the framework's centralized request walk deserializes and
/// validates every input against `INPUT_SCHEMA` before `handle` runs.
#[smithy_macros::smithy(input = "smithy.api#Unit")]
#[derive(Debug, Clone, Default)]
pub struct DeleteAccountPasswordPolicyInput;
