// SPDX-License-Identifier: CC0-1.0

//! The snapshots the EBS DIRECT APIs hold, as this service's `Snapshot` shape.
//!
//! `ebs:StartSnapshot` "creates a new Amazon EBS snapshot" and the EBS direct APIs model no
//! read of a snapshot's state at all, so `ec2:DescribeSnapshots` is the only read that
//! reports one: the two APIs describe ONE snapshot resource, and a snapshot minted on the
//! direct-API side is a snapshot of this account for every EC2 read that resolves or reports
//! a snapshot id.
//!
//! So this module reads the EBS service's OWN store — through that crate's own
//! [`state accessor`](::service_com_amazonaws_ebs_ebs::state::service_state) — and projects
//! each snapshot it holds onto [`SnapshotView`], the shape every EC2 snapshot report answers
//! with. NOTHING is copied into this service's store: a direct-API snapshot has exactly one
//! record, held by the service whose calls create, fill and seal it, so the two sides cannot
//! disagree about its state, its size or its encryption, and no write of this service's can
//! strand a copy of it.
//!
//! The projection is a READ: the `pending` -> `error` move an unsealed snapshot's `Timeout`
//! owes is DERIVED here from the expiry instant the EBS store already holds
//! (`StartSnapshotRequest$Timeout`, "the snapshot is automatically cancelled" if it is not
//! completed in time), exactly as the EBS store's own materialisation compares it, so this
//! read observes that move without writing to another service's state.
//!
//! The ONE write is the DELETE ([`delete_direct_api_snapshot`]): `ec2:DeleteSnapshot`
//! "deletes the specified snapshot" of the account, the EBS direct APIs model no delete of
//! their own, and one resource with one record has one removal — so the delete removes that
//! record through the owning crate's own removal rather than hiding the snapshot behind a
//! tombstone of this service's.
//!
//! LOCK ORDER: an EC2 operation takes its OWN store first and the EBS store second (every
//! function here is called with the EC2 store already locked, or with none locked at all),
//! and no EBS handler reads this service's store — so the two orders cannot invert.
//!
//! BOUNDS: a projection costs one `Snapshot` per snapshot that store holds, which is bounded
//! by the EBS service's own ceiling on stored snapshots
//! (`service_com_amazonaws_ebs_ebs::state::MAX_STORED_SNAPSHOTS`) — no request can grow it —
//! and the block BYTES are never copied: a snapshot's size is counted off its block map.
//! What a reader then spends comparing these snapshots against a request's filters is charged
//! to that reader's own match budget, the same one its own store's snapshots are charged to.

use std::collections::BTreeMap;
use std::time::SystemTime;

use ::service_com_amazonaws_ebs_ebs::state::{
    self as ebs_store, Snapshot as DirectApiSnapshot, SnapshotStatus as DirectApiSnapshotStatus,
};

use super::*;

/// A snapshot the EBS direct APIs hold that has been cancelled: the `SnapshotState` enum's
/// `error` member (`smithy_query(query="shape",shape="SnapshotState")`), which is what a
/// snapshot whose `Timeout` elapsed before `CompleteSnapshot` sealed it is in — the EBS
/// direct APIs report the same condition as their own `Status` `error`.
pub const SNAPSHOT_STATE_ERROR: &str = "error";

/// Every snapshot the EBS direct APIs hold for this account as of `now`, keyed by
/// `SnapshotId`, as the `Snapshot` shape an EC2 report answers with.
///
/// Empty when this service is mounted on an engine the EBS service is not mounted on, which
/// is the same answer as an account that has started no direct-API snapshot: an EC2 read
/// reports the snapshots there are, and is never refused for want of the other service.
///
/// Keyed by id in a [`BTreeMap`], so a report built from it is in the same `SnapshotId` order
/// as one built from this service's own store and a page token can name a position in the
/// merged sequence.
pub fn direct_api_snapshots<Eng: Send + Sync + 'static>(
    engine: &Eng,
    now: SystemTime,
) -> BTreeMap<String, SnapshotView> {
    let Some(state) = ebs_store::service_state(engine) else {
        return BTreeMap::new();
    };
    let store = state.lock();
    store
        .snapshots
        .iter()
        .map(|(snapshot_id, snapshot)| (snapshot_id.clone(), snapshot_view(snapshot, now)))
        .collect()
}

/// The one snapshot the EBS direct APIs hold under `snapshot_id` as of `now`, or `None` when
/// they hold none.
///
/// The single-id half of [`direct_api_snapshots`], for the operations that resolve a
/// `SnapshotId` the request named rather than reporting the whole set — so neither side
/// projects a snapshot the other would project differently.
pub fn direct_api_snapshot<Eng: Send + Sync + 'static>(
    engine: &Eng,
    snapshot_id: &str,
    now: SystemTime,
) -> Option<SnapshotView> {
    let state = ebs_store::service_state(engine)?;
    let store = state.lock();
    let snapshot = store.snapshots.get(snapshot_id)?;
    Some(snapshot_view(snapshot, now))
}

/// Whether the EBS direct APIs hold a snapshot under `snapshot_id`.
///
/// For a validator that only has to RESOLVE the id — a snapshot the account holds is one its
/// id names on either side, so a reader asks this beside its own store rather than refusing
/// an id the other service minted.
pub fn direct_api_snapshot_exists<Eng: Send + Sync + 'static>(
    engine: &Eng,
    snapshot_id: &str,
) -> bool {
    ebs_store::service_state(engine)
        .is_some_and(|state| state.lock().snapshots.contains_key(snapshot_id))
}

/// Delete the snapshot the EBS direct APIs hold under `snapshot_id`, answering whether one
/// was there to delete.
///
/// The ONE write this module makes, and the only one it can make: the EBS direct APIs model
/// no delete of their own, while `ec2:DeleteSnapshot` "deletes the specified snapshot" of the
/// account — and a snapshot minted through the direct APIs is a snapshot of that account. The
/// record it removes is the one that service keeps, through that crate's own removal
/// ([`ebs_store::ComAmazonawsEbsEbsState::remove_snapshot`]), so the delete takes the
/// snapshot's blocks and the `ClientToken` rows that named it too and leaves no copy of it
/// anywhere.
///
/// `false` for a snapshot those APIs do not hold, including on an engine the EBS service is
/// not mounted on — the caller has already resolved the id and answers the not-found refusal
/// for it.
pub fn delete_direct_api_snapshot<Eng: Send + Sync + 'static>(
    engine: &Eng,
    snapshot_id: &str,
) -> bool {
    ebs_store::service_state(engine).is_some_and(|state| {
        let mut store = state.lock();
        store.remove_snapshot(snapshot_id).is_some()
    })
}

/// One snapshot of the EBS direct-API store as the `Snapshot` shape, as of `now`.
///
/// Every member is read off that record or derived from it; a member a snapshot of this kind
/// carries nothing for is reported as carrying nothing, the way this crate's own snapshot
/// writers report theirs.
fn snapshot_view(snapshot: &DirectApiSnapshot, now: SystemTime) -> SnapshotView {
    let state = direct_api_snapshot_state(snapshot, now);
    let completed = state == SNAPSHOT_STATE_COMPLETED;
    SnapshotView {
        snapshot_id: Some(snapshot.snapshot_id.clone()),
        owner_id: Some(snapshot.owner_id.clone()),
        description: snapshot.description.clone(),
        // "The size of the volume, in GiB" (`Snapshot$VolumeSize`) is the size the direct
        // API's caller declared the snapshot's volume to be
        // (`StartSnapshotRequest$VolumeSize`, "the size of the volume, in GiB"). A value
        // outside the modelled width of the EC2 member is reported as none rather than
        // wrapped, which the EBS handler's own bound makes unreachable.
        volume_size: i32::try_from(snapshot.volume_size).ok(),
        // "The ID of the volume that was used to create the snapshot" (`Snapshot$VolumeId`):
        // a direct-API snapshot is written block by block by its caller rather than taken
        // from a volume, so there is no volume of the account to name — the same answer an
        // imported snapshot gives, whose source is a disk image outside this emulator.
        volume_id: None,
        state: Some(state.to_string()),
        progress: Some(direct_api_snapshot_progress(state).to_string()),
        // "The time stamp when the snapshot was initiated" (`Snapshot$StartTime`): when
        // `StartSnapshot` created it.
        start_time: Some(system_time_to_date_time(snapshot.start_time)),
        // "The time stamp when the snapshot was completed" (`Snapshot$CompletionTime`): when
        // `CompleteSnapshot` sealed it, which only a sealed snapshot has.
        completion_time: completed
            .then(|| snapshot.completed_at.map(system_time_to_date_time))
            .flatten(),
        encrypted: Some(snapshot.encrypted),
        // "The Amazon Resource Name (ARN) of the KMS key that was used to protect the volume
        // encryption key" (`Snapshot$KmsKeyId`): the key the direct-API caller named, which
        // that API takes as an ARN too (`StartSnapshotRequest$KmsKeyArn`).
        kms_key_id: snapshot.kms_key_arn.clone(),
        // Both models spell the same server-side encryption types (`SSEType`: `sse-ebs`,
        // `sse-kms`, `none`), so the type the snapshot was created with is carried through as
        // it stands; a snapshot with no server-side encryption carries none.
        sse_type: snapshot
            .sse_type
            .map(|sse_type| sse_type.as_wire().to_string()),
        tags: Some(
            snapshot
                .tags
                .iter()
                .map(|tag| Tag {
                    key: tag.key.clone(),
                    value: tag.value.clone(),
                })
                .collect(),
        ),
        // "`standard` indicates that the snapshot is stored in the standard snapshot storage
        // tier and that it is ready for use" (`Snapshot$StorageTier`): the tier every
        // snapshot is created in, `archive` being where `ModifySnapshotTier` moves one.
        storage_tier: Some(SNAPSHOT_STORAGE_TIER_STANDARD.to_string()),
        // "The full snapshot size […] represents the size of all the blocks that were
        // written" (`Snapshot$FullSnapshotSizeInBytes`): for a snapshot written through the
        // direct APIs those blocks are exactly the ones its store holds, each of the
        // snapshot's own block size. Only a SEALED snapshot has a full size — one still
        // taking writes has no final set of blocks to measure.
        full_snapshot_size_in_bytes: completed.then(|| full_snapshot_size_in_bytes(snapshot)),
        // "Only for snapshot copies […] `standard` - The snapshot copy was created with a
        // standard snapshot copy operation" (`Snapshot$TransferType`): EC2 reports a transfer
        // type for every snapshot it reports, and `time-based` belongs to the one kind of
        // write that asks for a completion duration — which the direct APIs' own write does
        // not take, so a snapshot written through them is a standard one.
        transfer_type: Some(SNAPSHOT_TRANSFER_TYPE_STANDARD.to_string()),
        // The members a direct-API snapshot carries nothing for, for the reasons this crate's
        // own snapshot writers record against the same shape: the owner alias names an
        // Amazon-published snapshot, the Outpost ARN one stored on an Outpost, the
        // Availability Zone one in a Local Zone, the restore expiry a temporarily restored
        // archived snapshot, the completion duration a time-based copy, the
        // state message a failed asynchronous copy, and the data encryption key id a member
        // "only returned by DescribeSnapshots" for a snapshot of an encrypted volume this
        // emulator holds no key material for.
        owner_alias: None,
        outpost_arn: None,
        availability_zone: None,
        restore_expiry_time: None,
        completion_duration_minutes: None,
        state_message: None,
        data_encryption_key_id: None,
    }
}

/// The `SnapshotState` the EBS direct-API `Status` of `snapshot` is, as of `now`.
///
/// `pending` and `completed` are the same word on both sides. `error` is the third: the EBS
/// store moves a snapshot there when its `Timeout` elapses unsealed, and that move is owed by
/// the CLOCK at an instant that store already holds — so it is derived here from the same
/// comparison rather than read as `pending` until some EBS call next materialises it.
fn direct_api_snapshot_state(snapshot: &DirectApiSnapshot, now: SystemTime) -> &'static str {
    match snapshot.status {
        DirectApiSnapshotStatus::Completed => SNAPSHOT_STATE_COMPLETED,
        DirectApiSnapshotStatus::Error => SNAPSHOT_STATE_ERROR,
        DirectApiSnapshotStatus::Pending if now >= snapshot.expiry_time => SNAPSHOT_STATE_ERROR,
        DirectApiSnapshotStatus::Pending => SNAPSHOT_STATE_PENDING,
    }
}

/// "The progress of the snapshot, as a percentage" (`Snapshot$Progress`) for a snapshot in
/// `state`: the same fact as the state, as it is for every snapshot this crate reports — all
/// of the data has been written for a `completed` snapshot, and none of it is accounted for
/// in one that is still being written or was cancelled before it was sealed.
fn direct_api_snapshot_progress(state: &str) -> &'static str {
    if state == SNAPSHOT_STATE_COMPLETED {
        SNAPSHOT_PROGRESS_COMPLETED
    } else {
        SNAPSHOT_PROGRESS_PENDING
    }
}

/// The bytes the blocks of `snapshot` hold: one block's size per block its store holds.
fn full_snapshot_size_in_bytes(snapshot: &DirectApiSnapshot) -> i64 {
    i64::try_from(snapshot.blocks.len())
        .unwrap_or(i64::MAX)
        .saturating_mul(i64::from(snapshot.block_size))
}
