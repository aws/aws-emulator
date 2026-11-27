// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `batch_get_account_status` operation.
//!
//! Hand-written at a generated path, and so NOT re-rendered by a model refresh, for one
//! reason: AWS reports an account's Amazon Inspector posture as a `State` carrying the
//! `status` in force and NEITHER of `errorCode` and `errorMessage`, which explain why an
//! account failed to enable Amazon Inspector, while the published model marks all three
//! `@required` and `ErrorCode` admits no token for "nothing failed". The published
//! snapshot is kept EXACTLY as shipped (see `smithy-model/LOCAL-ADDITIONS.md`), so that one
//! shape is generated from the small overlay model `smithy-model/local-additions.json`,
//! which restates it with those two members optional against the snapshot's own member
//! targets.
//!
//! Nothing about the wire is hand-written: every shape this operation reaches is still
//! declared by `#[smithy_macros::smithy]` from a model file. The two model files are named
//! from two submodules rather than from one file because `smithy_macros::smithy_model!`
//! defines the local `smithy!` macro that `#[smithy]` expands into, and that definition
//! covers the whole file it appears in — nested modules included — so a module naming two
//! models cannot resolve the attribute.
//!
//! * [`snapshot`] — every shape as the published model declares it.
//! * [`overlay`] — `State`, the one shape stated more loosely.
//!
//! Each structure/union is declared exactly once across this service's `types/`
//! modules, and `types/mod.rs` re-exports them all.

mod overlay;
mod snapshot;

#[allow(unused_imports)]
pub use overlay::*;
#[allow(unused_imports)]
pub use snapshot::*;
