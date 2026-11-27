// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `create_stream_group` operation.
//!
//! Hand-written at a generated path, and so NOT re-rendered by a model refresh, for one
//! reason: AWS answers every stream-group read with a group-level capacity summary —
//! `DesiredCapacity`, `ActiveCapacity` and `IdleCapacity` — that the published model does
//! not declare on the stream-group outputs. The published snapshot is kept EXACTLY as
//! shipped (see `smithy-model/LOCAL-ADDITIONS.md`), so that one output shape is generated
//! from the small overlay model `smithy-model/local-additions.json`, which restates it
//! with those three members against the snapshot's own member targets.
//!
//! Nothing about the wire is hand-written: every shape this operation reaches is still
//! declared by `#[smithy_macros::smithy]` from a model file. The two model files are named
//! from two submodules rather than from one file because `smithy_macros::smithy_model!`
//! defines the local `smithy!` macro that `#[smithy]` expands into, and that definition
//! covers the whole file it appears in — nested modules included — so a module naming two
//! models cannot resolve the attribute.
//!
//! * [`snapshot`] — every shape as the published model declares it.
//! * [`overlay`] — `CreateStreamGroupOutput`, the one shape stated with more members.
//!
//! Each structure/union is declared exactly once across this service's `types/`
//! modules, and `types/mod.rs` re-exports them all.

mod overlay;
mod snapshot;

#[allow(unused_imports)]
pub use overlay::*;
#[allow(unused_imports)]
pub use snapshot::*;
