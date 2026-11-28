// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `start_workflow_execution` operation.
//!
//! Hand-written at a generated path, and so NOT re-rendered by a model refresh, for one
//! reason: the fault this operation refuses a colliding start with carries, beside its
//! message, the identity of the run that already holds the `workflowId` — a member the
//! published snapshot does not declare. The snapshot is kept exactly as shipped (see
//! `smithy-model/LOCAL-ADDITIONS.md`), so that one shape is generated from the small
//! overlay model `smithy-model/local-additions.json` and every other from the snapshot, in
//! two sibling modules.
//!
//! * [`snapshot`] — every shape as the published model declares it.
//! * [`overlay`] — the fault stated with the member AWS answers with.
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
