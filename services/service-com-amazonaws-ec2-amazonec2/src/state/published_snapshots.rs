// SPDX-License-Identifier: CC0-1.0

//! The snapshots the AMI CATALOGUE restores its devices from, as this service's `Snapshot`
//! shape.
//!
//! "The snapshots available to you include public snapshots, private snapshots that you own,
//! and private snapshots owned by other Amazon Web Services accounts for which you have
//! explicit create volume permissions" (`DescribeSnapshots`), and "all Amazon Web Services
//! accounts have create volume permissions" for a public one — so the account-visible
//! snapshot inventory is wider than the snapshots this account CREATED. Every AMI of the
//! catalogue restores each of its EBS devices "from a snapshot"
//! (`EbsBlockDevice$SnapshotId`), and the snapshots the public images Amazon publishes
//! restore from are public ones: they are part of that inventory, owned by the account that
//! published the image and carrying its owner alias.
//!
//! So this module DERIVES them from the image records
//! ([`ComAmazonawsEc2Amazonec2State::images`]) rather than keeping a second copy of them:
//! [`ComAmazonawsEc2Amazonec2State::insert_image`] mints the snapshot id of a derived root
//! block device mapping and writes it into that mapping, so the ids are the ones the AMI
//! itself reports and one snapshot has one record. A snapshot id a mapping names that this
//! service's own store DOES hold — the caller's own snapshot, named by a `RegisterImage`
//! mapping — is left to that record, so no snapshot is projected twice and no image can
//! resurrect one a delete removed.
//!
//! It is a READ: nothing here writes, and the members a catalogue snapshot reports are read
//! off the image that restores from it — its owner, its owner alias, its creation date and
//! the size, encryption and key of the device the mapping restores.
//!
//! BOUNDS: one projection per EBS device mapping of each stored image, which is bounded by
//! the store's own ceiling on images ([`MAX_IMAGES`]) and the mappings a register may supply;
//! what a reader then spends comparing these snapshots against a request's filters is charged
//! to that reader's own match budget, the same one its own store's snapshots are charged to.
//!
//! SCOPE: this is the projection the READ of the inventory answers with. The operations that
//! act on a snapshot OF THE ACCOUNT — a delete, a tier change, a fast snapshot restore, a
//! volume created from one — read the account's own store and are unchanged by it, so a
//! snapshot of another account's catalogue is reported here and acted on nowhere.

use std::collections::BTreeMap;

use super::*;

/// "The ID of the volume that was used to create the snapshot" (`Snapshot$VolumeId`) as EC2
/// reports it for a snapshot whose source volume is no volume of the account reading it: the
/// all-ones placeholder, which is the value every item of the recorded Amazon-published
/// snapshot catalogue carries.
///
/// A snapshot an AMI of the catalogue restores from was taken of a volume in the PUBLISHER's
/// account, which no reader can resolve, so EC2 answers the member with a volume id that names
/// nothing rather than leaking the publisher's own or leaving the member out.
pub const SNAPSHOT_SOURCE_VOLUME_PLACEHOLDER: &str = "vol-ffffffff";

impl ComAmazonawsEc2Amazonec2State {
    /// Every snapshot an AMI of this account's catalogue restores one of its devices from
    /// that this service's own store holds no record of, keyed by `SnapshotId`.
    ///
    /// Keyed by id in a [`BTreeMap`], so a report built from it is in the same `SnapshotId`
    /// order as one built from the snapshot store and a pagination token can name a position
    /// in the merged sequence. An image the Recycle Bin holds is DEREGISTERED, so it restores
    /// nothing and is not read here.
    pub fn image_backing_snapshots(&self) -> BTreeMap<String, SnapshotView> {
        let mut snapshots = BTreeMap::new();
        for image in self.images.values() {
            for mapping in image.block_device_mappings.iter().flatten() {
                let Some(ebs) = mapping.ebs.as_ref() else {
                    continue;
                };
                let Some(snapshot_id) = ebs.snapshot_id.as_deref() else {
                    continue;
                };
                if self.snapshots.contains_key(snapshot_id) {
                    continue;
                }
                snapshots.insert(
                    snapshot_id.to_string(),
                    image_backing_snapshot(image, ebs, snapshot_id),
                );
            }
        }
        snapshots
    }
}

/// One device mapping's snapshot as the `Snapshot` shape, read off the `image` that restores
/// from it.
///
/// A snapshot an AMI restores from is COMPLETE: the image is `available`, so every block of
/// the volume it restores has been written — which is also what its progress ("the progress
/// of the snapshot, as a percentage") and its completion time report. That instant is the
/// image's own creation date, because the snapshot is what the image was published WITH, so
/// neither can be reported as having happened without the other.
fn image_backing_snapshot(
    image: &Image,
    ebs: &EbsBlockDevice,
    snapshot_id: &str,
) -> SnapshotView {
    let published_at = image_creation_time(image.creation_date.as_deref());
    // "The Amazon Web Services owner alias, from an Amazon-maintained list (`amazon`)"
    // (`Snapshot$OwnerAlias`): the alias of the account that published the image, which is
    // the account that owns the snapshot it restores from.
    let owner_alias = image.image_owner_alias.clone();
    let volume_size = ebs.volume_size;
    SnapshotView {
        snapshot_id: Some(snapshot_id.to_string()),
        owner_id: image.owner_id.clone(),
        owner_alias,
        // "A description of the snapshot" (`Snapshot$Description`): what the image the
        // snapshot was published with describes itself as.
        description: image.description.clone(),
        // "The ID of the volume that was used to create the snapshot" (`Snapshot$VolumeId`):
        // the publisher's, which this account cannot resolve
        // ([`SNAPSHOT_SOURCE_VOLUME_PLACEHOLDER`]).
        volume_id: Some(SNAPSHOT_SOURCE_VOLUME_PLACEHOLDER.to_string()),
        // "The size of the volume, in GiB" and "indicates whether the snapshot is encrypted"
        // (`Snapshot$VolumeSize`, `$Encrypted`): the device the mapping restores, as the AMI
        // reports it, so an image and the snapshot behind it cannot disagree about either.
        volume_size,
        encrypted: Some(ebs.encrypted.unwrap_or(false)),
        kms_key_id: ebs.kms_key_id.clone(),
        state: Some(SNAPSHOT_STATE_COMPLETED.to_string()),
        progress: Some(SNAPSHOT_PROGRESS_COMPLETED.to_string()),
        start_time: published_at,
        completion_time: published_at,
        // "`standard` indicates that the snapshot is stored in the standard snapshot storage
        // tier and that it is ready for use" (`Snapshot$StorageTier`): the tier a snapshot an
        // AMI restores from has to be in, an archived one being unusable until it is restored.
        storage_tier: Some(SNAPSHOT_STORAGE_TIER_STANDARD.to_string()),
        // "Only for snapshot copies […] `standard` - The snapshot copy was created with a
        // standard snapshot copy operation" (`Snapshot$TransferType`): EC2 reports a transfer
        // type for every snapshot it reports, and `time-based` belongs to the one kind of
        // write that asks for a completion duration — so a snapshot of the catalogue reports
        // the standard one, like every snapshot nothing asked a duration for.
        transfer_type: Some(SNAPSHOT_TRANSFER_TYPE_STANDARD.to_string()),
        // "The full size of the snapshot, in bytes" (`Snapshot$FullSnapshotSizeInBytes`),
        // through the one conversion [`volume_full_snapshot_size_in_bytes`] holds: every block
        // of the restored device is written in a snapshot an image restores from.
        full_snapshot_size_in_bytes: volume_size.and_then(volume_full_snapshot_size_in_bytes),
        // "Any tags assigned to the snapshot" (`Snapshot$Tags`): a catalogue snapshot is
        // tagged by nothing, there being no call that tags one.
        tags: Some(Vec::new()),
        // The members a snapshot of this kind carries nothing for, for the reasons this
        // crate's own snapshot writers record against the same shape: the Outpost ARN names
        // one stored on an Outpost, the Availability Zone one in a Local Zone, the restore
        // expiry a temporarily restored archived snapshot, the completion duration a
        // time-based copy, the state message a failed asynchronous copy, and the data
        // encryption key id a member "only returned by DescribeSnapshots" for a snapshot
        // this emulator holds no key material for. `SseType` is "reserved for future use".
        outpost_arn: None,
        availability_zone: None,
        restore_expiry_time: None,
        completion_duration_minutes: None,
        state_message: None,
        data_encryption_key_id: None,
        sse_type: None,
    }
}
