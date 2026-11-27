// SPDX-License-Identifier: CC0-1.0

//! `GetResourceSyncStatusOutput`: the sync state this service reports for one
//! `(SyncType, ResourceName)` pair.
//!
//! It is declared here rather than beside the operation's other shapes because AWS
//! answers a pair it has recorded no sync for with a success carrying NONE of the
//! sync-state members, while the published model snapshot declares `LatestSync`
//! `@required`, and that snapshot is kept as shipped (see
//! `smithy-model/LOCAL-ADDITIONS.md`). The snapshot's requirement is a requirement on a
//! resource that HAS been synced; a resource that never was is reported with the member
//! absent, which is what the wire fixtures record (a resource with no sync configuration
//! is answered 200 with an empty payload).
//!
//! So the type is generated, like every other modelled shape, from a model file — the
//! small overlay `smithy-model/local-additions.json`, which restates the three members
//! against their own targets and leaves `LatestSync` optional. Nothing about the wire is
//! hand-written.
//!
//! Only a sync the SERVICE performed records a state, and nothing in this emulator
//! fetches a remote repository's commits (see [`crate::state::StoredResourceSyncState`]),
//! so the members are rendered from the stored row and absent when nothing is stored.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/local-additions.json");

/// What `GetResourceSyncStatus` reports: the recorded sync state of the pair, and nothing
/// else.
#[smithy_macros::smithy(shape = "com.amazonaws.codeconnections#GetResourceSyncStatusOutput")]
#[derive(Debug, Clone, Default)]
pub struct GetResourceSyncStatusOutput {
    pub desired_state: Option<Revision>,
    pub latest_successful_sync: Option<ResourceSyncAttempt>,
    pub latest_sync: Option<ResourceSyncAttempt>,
}
