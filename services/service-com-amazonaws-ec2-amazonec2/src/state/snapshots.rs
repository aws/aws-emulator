// SPDX-License-Identifier: CC0-1.0

//! The EBS snapshots the account holds: what one snapshot is stored as, the bounds the
//! store keeps, and the clock-owed `pending` -> `completed` move every snapshot starts out
//! owing.
//!
//! A snapshot is stored as the MODELLED [`SnapshotView`] shape itself — the same `Snapshot`
//! structure `DescribeSnapshots` reports — for the same reason a volume is stored as a
//! `Volume` ([`super::VolumeRecord`]): every member a writer sets round-trips through a
//! later read without a field-by-field copy that can silently drop one. The store
//! ([`ComAmazonawsEc2Amazonec2State::snapshots`]) is keyed by `SnapshotId` in a
//! [`std::collections::BTreeMap`], so a report has one stable order and a pagination token
//! can name the id the next page starts at.
//!
//! Four members are NOT stored: the state, the progress, the completion time and the full
//! snapshot size. "Creates
//! crash-consistent snapshots" hands back a snapshot in `pending`
//! ([`SNAPSHOT_STATE_PENDING`]), and reaching `completed` is owed by the CLOCK rather than
//! by a later call — so the record keeps the instant that move falls due
//! ([`SnapshotRecord::completed_at`], monotonic and never reported, on the same
//! [`super::TRANSITION_DELAY`] span as a volume's `creating` -> `available` move) and
//! [`SnapshotRecord::view_at`] derives the first three from it. A read therefore observes the
//! move the clock already owed without writing anything, and two writers cannot store two
//! different words for one snapshot's state. The fourth is derived from the size of the
//! volume the snapshot was taken of ([`volume_full_snapshot_size_in_bytes`]), which is what
//! the member measures, so one volume is never measured two ways.
//!
//! [`ComAmazonawsEc2Amazonec2State::insert_snapshot`] is the one way a snapshot is stored,
//! and it mints the id itself: a snapshot id is the service's to choose, never a caller's.
//! `CreateSnapshot`, `CreateSnapshots` and `CopySnapshot` are the writers wired to it, and a
//! reader resolves an id out of this store rather than keeping a second copy of it.
//!
//! [`ComAmazonawsEc2Amazonec2State::remove_snapshot`] is the one way one leaves it, which is
//! what `DeleteSnapshot` does: the record is dropped, so the id stops resolving for every
//! reader at once.

use std::time::{Instant, SystemTime};

use crate::ops::types::{CreateSnapshotOutput, Tag};

use super::{
    ComAmazonawsEc2Amazonec2State, TRANSITION_DELAY, system_time_to_date_time, tag_list_bytes,
};

/// The `Snapshot` shape, under the name the scaffold generated for it.
///
/// `com.amazonaws.ec2#Snapshot` is both `CreateSnapshot`'s output shape and the member type
/// of `DescribeSnapshots`'s `Snapshots` list, so the generator emitted it once under the
/// output name. This alias lets the code read as the model does, the same way
/// [`super::VolumeView`] does for `Volume`.
pub type SnapshotView = CreateSnapshotOutput;

/// A snapshot whose data is still being written: the `SnapshotState` enum's `pending`
/// member (`smithy_query(query="shape",shape="SnapshotState")`), which is the state every
/// snapshot is created in.
pub const SNAPSHOT_STATE_PENDING: &str = "pending";

/// A snapshot that holds all of the source volume's data: the `SnapshotState` enum's
/// `completed` member, which is what the clock owes a `pending` snapshot.
pub const SNAPSHOT_STATE_COMPLETED: &str = "completed";

/// "The progress of the snapshot, as a percentage" (`Snapshot$Progress`) for a snapshot
/// that has only just been started: EMPTY. AWS answers the element with no text at all for
/// a snapshot it has not begun transferring, which is a different answer from a percentage,
/// so it is named here rather than being written as a literal where a percentage belongs.
pub const SNAPSHOT_PROGRESS_PENDING: &str = "";

/// `Snapshot$Progress` for a snapshot that has completed: all of the source volume's data
/// has been written, which as a percentage is 100.
pub const SNAPSHOT_PROGRESS_COMPLETED: &str = "100%";

/// "`standard` indicates that the snapshot is stored in the standard snapshot storage tier
/// and that it is ready for use" (`Snapshot$StorageTier`): the tier a snapshot is created
/// in, `archive` being where `ModifySnapshotTier` moves one.
pub const SNAPSHOT_STORAGE_TIER_STANDARD: &str = "standard";

/// "`standard` - The snapshot copy was created with a standard snapshot copy operation"
/// (`Snapshot$TransferType`): the transfer type of a copy the request asked no completion
/// duration for, which is "completed on a best-effort basis". A member of the modelled
/// `TransferType` catalog, the generated whole of which is
/// [`crate::ops::types::TRANSFER_TYPE_VALUES`].
///
/// "Only for snapshot copies": a snapshot a create made carries no transfer type at all,
/// which is why neither of these two words is a default of [`SnapshotRecord`].
pub const SNAPSHOT_TRANSFER_TYPE_STANDARD: &str = "standard";

/// "`time-based` - The snapshot copy was created with a time-based snapshot copy operation"
/// (`Snapshot$TransferType`): the transfer type of a copy the request asked a completion
/// duration for, which "complete[s] within the completion duration specified in the
/// request".
pub const SNAPSHOT_TRANSFER_TYPE_TIME_BASED: &str = "time-based";

/// The one resource type a snapshot-creating action's `TagSpecifications` may name: "Tags
/// to apply to every snapshot specified by the instance"
/// (`CreateSnapshotsRequest$TagSpecifications`) tags the snapshots it creates, and
/// `snapshot` is the `ResourceType` member for one — the same word
/// [`super::IMAGE_TAGGABLE_RESOURCES`] lists for the snapshots an image create tags.
pub const SNAPSHOT_TAGGABLE_RESOURCE: &str = "snapshot";

/// The id prefix of an EBS snapshot, as [`super::snapshot_id_is_well_formed`] reads one and
/// as a refusal names the form it expected.
pub const SNAPSHOT_ID_PREFIX: &str = "snap";

/// The name EC2 gives `DescribeSnapshotsRequest$SnapshotIds` when it refuses one of its
/// entries at the PARAMETER layer: "Value (…) for parameter snapshotId is invalid. Expected:
/// 'snap-...'." — the member's wire name is `SnapshotId.N`, and `snapshotId` is what the
/// service calls the parameter those values make up, the way it calls `DescribeVolumes`'
/// own list [`super::VOLUME_IDS_PARAMETER`].
pub const SNAPSHOT_IDS_PARAMETER: &str = "snapshotId";

/// How many EBS snapshots this emulator stores before a create is refused.
///
/// CHOSEN, like [`super::MAX_STORED_VOLUMES`] and for the same reason: nothing in this
/// emulator reclaims a stored snapshot, so the store needs a ceiling that is not the
/// process's memory. It is well above any published per-Region snapshot quota a test would
/// exercise. A count ceiling is not a memory bound on its own — see
/// [`super::MAX_STORED_BYTES`] — so the caller-chosen bytes of each snapshot are charged
/// against that budget too ([`snapshot_payload_bytes`]).
pub const MAX_STORED_SNAPSHOTS: usize = 4_096;

/// How many data volumes one request may exclude from a multi-volume snapshot set: "You
/// can specify up to 40 volume IDs per request"
/// (`InstanceSpecification$ExcludeDataVolumeIds`). The model declares no `@length` on the
/// list, so a handler is the only place the published bound can be enforced.
pub const MAX_EXCLUDED_DATA_VOLUME_IDS: usize = 40;

/// "To create local snapshots in the same Local Zone as the source instance, specify
/// `local`" (`CreateSnapshotsRequest$Location`): the `SnapshotLocationEnum` member that
/// asks for a snapshot of a Local Zone, which only an instance in one can be given.
pub const SNAPSHOT_LOCATION_LOCAL: &str = "local";

/// "To create regional snapshots in the parent Region of the Local Zone, specify
/// `regional` or omit this parameter. Default value: `regional`"
/// (`CreateSnapshotsRequest$Location`): the location every snapshot this store holds is in.
pub const SNAPSHOT_LOCATION_REGIONAL: &str = "regional";

/// The single member of the `CopyTagsFromSource` enum
/// (`smithy_query(query="shape",shape="CopyTagsFromSource")`): "copies the tags from the
/// specified volume to corresponding snapshot" — the source a snapshot's tags may be copied
/// from, the generated catalog of the whole enum being
/// `crate::ops::types::COPY_TAGS_FROM_SOURCE_VALUES`.
pub const COPY_TAGS_FROM_SOURCE_VOLUME: &str = "volume";

/// EC2's error code for a request that would take the account past the number of EBS
/// snapshots it may hold — "you've reached the limit on the number of Amazon EBS snapshots
/// that you can create" (EC2 API "Error codes", `SnapshotLimitExceeded`). A ceiling on what
/// is STORED is a quota, not something the request got wrong, which is why this is not an
/// `InvalidParameterValue`.
pub const SNAPSHOT_LIMIT_EXCEEDED: &str = "SnapshotLimitExceeded";

/// Refuse a create this store has no room for: HTTP 400 `SnapshotLimitExceeded`, with
/// `message` saying which ceiling was reached — the count of records
/// ([`MAX_STORED_SNAPSHOTS`]) or the caller-chosen bytes they hold
/// ([`super::MAX_STORED_BYTES`]).
pub fn snapshot_limit_exceeded<E>(message: impl Into<String>) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(SNAPSHOT_LIMIT_EXCEEDED, 400, message)
}

/// Refuse a delete of a snapshot a registered AMI still restores a device from: HTTP 400
/// `InvalidSnapshot.InUse`, naming the snapshot and the image that has to be deregistered
/// first — the two resources the caller has to act on, as
/// [`ComAmazonawsEc2Amazonec2State::image_using_snapshot`] resolves the second.
pub fn snapshot_in_use<E>(snapshot_id: &str, image_id: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        super::INVALID_SNAPSHOT_IN_USE,
        400,
        format!("The snapshot {snapshot_id} is currently in use by {image_id}"),
    )
}

/// One stored EBS snapshot: the modelled [`SnapshotView`] every operation that reports one
/// answers with, plus the instant the clock owes it its `pending` -> `completed` move.
///
/// The view holds no `State`, `Progress`, `CompletionTime` or `FullSnapshotSizeInBytes`: all
/// four are facts of the snapshot's age, so [`SnapshotRecord::view_at`] derives them — see
/// the module docs.
#[derive(Debug, Clone)]
pub struct SnapshotRecord {
    /// The snapshot as the model shapes it, with the age-derived members left for
    /// [`SnapshotRecord::view_at`] to fill.
    pub view: SnapshotView,
    /// The instant from which this snapshot's `pending` -> `completed` move is due.
    /// Monotonic ([`Instant`]), because it is only ever compared against the clock and
    /// never reported, so a wall-clock adjustment cannot move a snapshot back to `pending`.
    pub completed_at: Instant,
    /// The wall-clock instant that move falls due at, which is what a completed snapshot
    /// reports as its `CompletionTime` ("The time stamp when the snapshot was completed",
    /// `Snapshot$CompletionTime`). Held beside the monotonic instant above because a
    /// reported timestamp has to be a wall-clock one, while the comparison that decides
    /// whether the move is due must not be.
    pub completion_time: SystemTime,
}

impl SnapshotRecord {
    /// A snapshot this store has just started: `pending`, reaching `completed` at
    /// `completed_at`.
    pub fn started(view: SnapshotView, completed_at: Instant, completion_time: SystemTime) -> Self {
        Self {
            view,
            completed_at,
            completion_time,
        }
    }

    /// The snapshot's `SnapshotState` at `now`: `pending` until the clock-owed move falls
    /// due, `completed` from then on.
    pub fn state_at(&self, now: Instant) -> &'static str {
        if now < self.completed_at {
            SNAPSHOT_STATE_PENDING
        } else {
            SNAPSHOT_STATE_COMPLETED
        }
    }

    /// The snapshot's `Progress` at `now`, which is the same fact as its state: nothing has
    /// been transferred for a snapshot that has only just started, and all of it has for one
    /// that has completed.
    pub fn progress_at(&self, now: Instant) -> &'static str {
        if now < self.completed_at {
            SNAPSHOT_PROGRESS_PENDING
        } else {
            SNAPSHOT_PROGRESS_COMPLETED
        }
    }

    /// This snapshot as of `now`: the stored view with the state, the progress and the
    /// completion time the clock has reached. A read observes the transition the clock
    /// already owed without writing anything.
    pub fn view_at(&self, now: Instant) -> SnapshotView {
        let completed = now >= self.completed_at;
        SnapshotView {
            state: Some(self.state_at(now).to_string()),
            progress: Some(self.progress_at(now).to_string()),
            // "The time stamp when the snapshot was completed": a snapshot that has not
            // completed has none, rather than one in the future.
            completion_time: completed.then(|| system_time_to_date_time(self.completion_time)),
            // "The full size of the snapshot, in bytes […] represents the size of all the
            // blocks that were written to the source volume AT THE TIME THE SNAPSHOT WAS
            // CREATED" (`Snapshot$FullSnapshotSizeInBytes`): a fact of the source volume as
            // the create found it, so it is settled when the snapshot starts and does not
            // wait on the transfer the way the three members above do. Derived from that
            // volume's size through the one conversion [`volume_full_snapshot_size_in_bytes`]
            // holds,
            // rather than stored, so one volume is never measured two ways.
            full_snapshot_size_in_bytes: self
                .view
                .volume_size
                .and_then(volume_full_snapshot_size_in_bytes),
            ..self.view.clone()
        }
    }
}

/// The bytes one GiB holds, for the snapshot sizes EC2 reports in BYTES while the volume
/// they were taken of is measured in GiB (`Snapshot$VolumeSize`, "the size of the volume, in
/// GiB", against `Snapshot$FullSnapshotSizeInBytes`, "the full size of the snapshot, in
/// bytes").
pub const BYTES_PER_GIB: i64 = 1_024 * 1_024 * 1_024;

/// "The full size of the snapshot, in bytes […] represents the size of all the blocks that
/// were written to the source volume at the time the snapshot was created"
/// (`Snapshot$FullSnapshotSizeInBytes`) for a snapshot of a volume of `volume_size` GiB.
///
/// The FULL size rather than an incremental one: every block of the source volume is
/// accounted for, so the size a snapshot of a volume reports is that volume's own size in
/// bytes. The ONE definition of that conversion, so the snapshot a create, a copy or an
/// image's catalogue entry reports cannot measure the same volume differently; a size
/// outside the width of the reported member is reported as none rather than wrapped, which
/// the modelled width of `Snapshot$VolumeSize` makes unreachable. A snapshot written BLOCK BY
/// BLOCK through the EBS direct APIs is measured off its blocks instead, by that module's own
/// reading of the same member.
pub fn volume_full_snapshot_size_in_bytes(volume_size: i32) -> Option<i64> {
    i64::from(volume_size)
        .checked_mul(BYTES_PER_GIB)
        .filter(|size| *size >= 0)
}

/// The caller-chosen bytes one stored snapshot holds: its tags and the description the
/// request supplied. Everything else on the record — the ids, the account, the sizes, the
/// timestamps and the states — is derived by this module or bounded by the model, so it is
/// fixed-size.
///
/// The ONE definition of that charge: a create asks it before the write and
/// [`ComAmazonawsEc2Amazonec2State::insert_snapshot`] charges exactly it, so a removal path
/// must give back the same number.
pub fn snapshot_payload_bytes(tags: &[Tag], description: Option<&str>) -> usize {
    tag_list_bytes(tags).saturating_add(description.map_or(0, str::len))
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another `count` snapshots fit inside the store's ceiling
    /// ([`MAX_STORED_SNAPSHOTS`]).
    ///
    /// Asked by a create BEFORE it mints anything, so a request that cannot be stored whole
    /// is refused rather than half-written: `CreateSnapshots` makes one snapshot per volume
    /// of an instance, so the question is about the whole set and not about one record.
    pub fn has_snapshot_capacity(&self, count: usize) -> bool {
        self.snapshots.len().saturating_add(count) <= MAX_STORED_SNAPSHOTS
    }

    /// Remove the snapshot `snapshot_id` names, and everything stored only with it.
    ///
    /// The mirror of [`Self::insert_snapshot`]: the caller-chosen bytes the record was
    /// charged for are given back to the store's budget, and the fast snapshot restores of
    /// the snapshot go with it — a restore is a property of the (snapshot, zone) pair, so a
    /// snapshot the account no longer holds can have none. That last rule lives in ONE place
    /// ([`Self::forget_fast_snapshot_restores`]), because a snapshot the EBS direct APIs hold
    /// has its pairs in THIS store and its record in theirs, so the same delete has to apply
    /// it from the handler too.
    pub fn remove_snapshot(&mut self, snapshot_id: &str) -> Option<SnapshotRecord> {
        let record = self.snapshots.remove(snapshot_id)?;
        self.release_stored_bytes(snapshot_payload_bytes(
            record.view.tags.as_deref().unwrap_or_default(),
            record.view.description.as_deref(),
        ));
        self.forget_fast_snapshot_restores(snapshot_id);
        Some(record)
    }

    /// Store `view` as a snapshot that was started at `started_at` and reaches `completed`
    /// [`TRANSITION_DELAY`] later, and answer the `SnapshotId` it was given.
    ///
    /// The id is MINTED here, out of the store's own counter, and written onto the view:
    /// "The ID of the snapshot. Each snapshot receives a unique identifier when it is
    /// created" (`Snapshot$SnapshotId`) — it is the service's to choose, so no caller can
    /// supply one. The start time is the WRITER's, because a call that creates several
    /// snapshots reports one instant for all of them ("This is the same for all snapshots
    /// initiated by the same request", `SnapshotInfo$StartTime`), and the completion the
    /// clock owes is measured from it.
    ///
    /// The caller-chosen bytes the record retains are charged against the store's budget
    /// here, so no path can store them uncharged.
    pub fn insert_snapshot(&mut self, view: SnapshotView, started_at: SystemTime) -> String {
        self.insert_snapshot_completing_at(view, started_at, Instant::now() + TRANSITION_DELAY)
    }

    /// [`Self::insert_snapshot`] with the instant the `pending` -> `completed` move falls
    /// due named by the writer.
    ///
    /// For a writer whose snapshot was DUE at an earlier instant than the one this write
    /// runs at — the snapshot a VM import's conversion window owed, minted when the first
    /// caller since that window observed it — so when the snapshot completes is fixed by the
    /// call that started it rather than by when anybody looked. The reported completion
    /// timestamp is the wall-clock instant that same move is still away by, so a snapshot
    /// reported `completed` never carries one in the future.
    pub fn insert_snapshot_completing_at(
        &mut self,
        mut view: SnapshotView,
        started_at: SystemTime,
        completed_at: Instant,
    ) -> String {
        let snapshot_id = self.next_id(SNAPSHOT_ID_PREFIX);
        view.snapshot_id = Some(snapshot_id.clone());
        view.start_time = Some(system_time_to_date_time(started_at));
        // "Indicates whether the snapshot […] was created with a standard or time-based
        // snapshot copy operation […] `standard` […] completed on a best-effort basis"
        // (`Snapshot$TransferType`): EC2 reports a transfer type for EVERY snapshot it
        // reports, and `time-based` belongs to the one kind of write that asks for a
        // completion duration — a copy — so a writer that named none made a `standard` one.
        // Written here, where a snapshot is stored, so every reader of the record reports the
        // same word for it.
        view.transfer_type = view
            .transfer_type
            .or_else(|| Some(SNAPSHOT_TRANSFER_TYPE_STANDARD.to_string()));
        self.add_stored_bytes(snapshot_payload_bytes(
            view.tags.as_deref().unwrap_or_default(),
            view.description.as_deref(),
        ));
        let still_to_run = completed_at.saturating_duration_since(Instant::now());
        let completion_time = started_at.checked_add(still_to_run).unwrap_or(started_at);
        self.snapshots.insert(
            snapshot_id.clone(),
            SnapshotRecord::started(view, completed_at, completion_time),
        );
        snapshot_id
    }
}
