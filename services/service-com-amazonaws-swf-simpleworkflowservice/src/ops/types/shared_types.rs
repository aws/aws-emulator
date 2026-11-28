// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached by more than one operation.
//!
//! Hand-written at a generated path, and so NOT re-rendered by a model refresh, for one
//! reason: this service answers with five of these shapes carrying members the published
//! snapshot does not declare — the discriminator on a reported tag, the handle on a task
//! hand-over, whether a decision task continued the previous one's completion, whether a
//! scheduled activity task was started straight away, and the close and reopening members
//! of an execution record. The snapshot is kept exactly as shipped (see
//! `smithy-model/LOCAL-ADDITIONS.md`), so those five shapes are generated from the small
//! overlay model `smithy-model/local-additions.json` and every other from the snapshot, in
//! two sibling modules.
//!
//! * [`snapshot`] — every shape as the published model declares it.
//! * [`overlay`] — the five shapes stated with more members.
//!
//! Each structure/union is declared exactly once across this service's `types/`
//! modules, and `types/mod.rs` re-exports them all.
#![allow(unused_imports)]

use super::*;

mod overlay;
mod snapshot;

#[allow(unused_imports)]
pub use overlay::*;
#[allow(unused_imports)]
pub use snapshot::*;
