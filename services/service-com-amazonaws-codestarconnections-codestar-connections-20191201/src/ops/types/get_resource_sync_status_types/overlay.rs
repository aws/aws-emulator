// SPDX-License-Identifier: CC0-1.0

//! `GetResourceSyncStatusOutput`, declared from this crate's overlay model.
//!
//! AWS answers `GetResourceSyncStatus` for a `(SyncType, ResourceName)` pair it has
//! recorded no sync state for with HTTP 200 and NONE of `DesiredState`, `LatestSync` and
//! `LatestSuccessfulSync` present, while the published model marks `LatestSync`
//! `@required`. The published snapshot is kept exactly as shipped (see
//! `smithy-model/LOCAL-ADDITIONS.md`), so this one shape is generated from the small
//! overlay model `smithy-model/local-additions.json`, which restates it with `LatestSync`
//! optional against the snapshot's own member targets. Nothing about the wire is
//! hand-written: the shape is still declared by `#[smithy_macros::smithy]` from a model.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/local-additions.json");

/// The sync state this service has recorded for one `(SyncType, ResourceName)` pair.
///
/// `latest_sync` is `None` for a pair that has never synced — the answer AWS gives with
/// none of the three members present, which is how a client polling before the resource
/// has ever synced tells "never synced" from "bad request".
#[smithy_macros::smithy(shape = "com.amazonaws.codestarconnections#GetResourceSyncStatusOutput")]
#[derive(Debug, Clone, Default)]
pub struct GetResourceSyncStatusOutput {
    pub desired_state: Option<Revision>,
    pub latest_successful_sync: Option<ResourceSyncAttempt>,
    pub latest_sync: Option<ResourceSyncAttempt>,
}
