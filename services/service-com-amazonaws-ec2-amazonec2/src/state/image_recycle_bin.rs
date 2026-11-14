// SPDX-License-Identifier: CC0-1.0

//! AMIs in the Recycle Bin: the store an AMI waits in between its deregistration and its
//! permanent deletion, and the rules that move one in and out of it.
//!
//! "If a deregistered EBS-backed AMI matches a Recycle Bin retention rule, it moves to the
//! Recycle Bin for the specified retention period. It can be restored before its retention
//! period expires, after which it is permanently deleted" (`DeregisterImage`). A recycled
//! AMI is therefore NOT one of the images the account can see — `DescribeImages` must not
//! report it, and nothing can launch from it — while still being held, so
//! [`super::ComAmazonawsEc2Amazonec2State::images_in_recycle_bin`] keeps it in a store of
//! its own, keyed by the same `ImageId`, as a [`RecycledImageRecord`]: the whole [`Image`]
//! the account last saw, so restoring it puts the very same image back rather than a
//! rebuilt copy, beside the two instants `ImageRecycleBinInfo` reports — when it entered
//! the bin and when it is to be permanently deleted, which is the enter instant plus the
//! retention period of the rule that matched the deregistration.
//!
//! That exit instant is a clock-owed PERMANENT DELETION, recorded rather than slept for the
//! way a `pending` VPC's `available` move is, and
//! [`super::ComAmazonawsEc2Amazonec2State::materialize_due_transitions`] applies it: an
//! entry whose retention period has run out is gone from the bin whether or not anybody
//! asked, so `RestoreImageFromRecycleBin` can only restore an entry that is still inside
//! its retention period. A recycled image keeps counting against
//! [`super::ComAmazonawsEc2Amazonec2State::has_image_capacity`] while it is held, so
//! restoring it can never be refused for want of room.
//!
//! `RestoreImageFromRecycleBin` is the one operation wired to this store so far.
//! [`ComAmazonawsEc2Amazonec2State::recycle_image`] is what puts an AMI in the bin and is
//! called by `DeregisterImage`, and [`RecycledImageRecord::view`] is the projection
//! `ListImagesInRecycleBin` reports; both operations are still scaffolds with their own
//! stub issues, so they are what wires these two up.

use std::time::{Duration, SystemTime};

use crate::ops::types::{Image, ImageRecycleBinInfo};
use crate::scaffolded::Scaffolded;

use super::{
    ComAmazonawsEc2Amazonec2State, system_time_to_date_time, tag_list_bytes,
    unresolvable_image_id,
};

impl ComAmazonawsEc2Amazonec2State {
    /// Move the AMI `image_id` into the Recycle Bin for `retention_period`, and answer
    /// whether there was such an image to move.
    ///
    /// "If a deregistered EBS-backed AMI matches a Recycle Bin retention rule, it moves to
    /// the Recycle Bin for the specified retention period" (`DeregisterImage`): the image
    /// leaves [`Self::images`] — nothing can see or launch from it any more — and is held
    /// in [`Self::images_in_recycle_bin`] under the same id until the retention period the
    /// matching rule specified runs out. The period is the rule's, never this store's, so
    /// it is passed in rather than chosen here.
    ///
    /// `false` means NOTHING moved: no image is stored under that id, or the retention
    /// period has no representable exit instant.
    pub fn recycle_image(&mut self, image_id: &str, retention_period: Duration) -> bool {
        let entered_at = SystemTime::now();
        // A retention period a Recycle Bin rule can carry (1 to 365 days) always has a
        // representable exit instant. One that does not is no period this store can keep,
        // and the answer is to leave the AMI where it is rather than move it into a bin
        // that would permanently delete it at once — the same answer
        // `move_volume_to_recycle_bin` gives an unrepresentable retention period, and the
        // reason the arithmetic is checked BEFORE the image leaves `images`.
        let Some(exit_at) = entered_at.checked_add(retention_period) else {
            return false;
        };
        let Some(image) = self.images.remove(image_id) else {
            return false;
        };
        self.images_in_recycle_bin.insert(
            image_id.to_string(),
            RecycledImageRecord {
                image,
                recycle_bin_enter_time: entered_at,
                recycle_bin_exit_time: exit_at,
            },
        );
        true
    }

    /// Take the AMI `image_id` back out of the Recycle Bin, and answer whether it moved.
    ///
    /// "Restores an AMI from the Recycle Bin" (`RestoreImageFromRecycleBin`): the very
    /// [`Image`] that entered the bin goes back among the images the account can see, under
    /// the same id and with every member it had, and the bin holds it no more. `false` when
    /// the bin holds no entry under that id, which is the only way this can fail — so a
    /// caller answers the request's own refusal rather than reporting a restore that did
    /// not happen. The OWNERSHIP of that entry is not checked here: a Recycle Bin operation
    /// resolves the id it was given through [`require_recycled_image`] first, which is
    /// where the per-account rule lives.
    pub fn restore_recycled_image(&mut self, image_id: &str) -> bool {
        let Some(record) = self.images_in_recycle_bin.remove(image_id) else {
            return false;
        };
        self.images.insert(image_id.to_string(), record.image);
        true
    }

    /// Permanently delete every recycled AMI whose retention period has run out as of
    /// `now`.
    ///
    /// "It can be restored before its retention period expires, after which it is
    /// permanently deleted" (`DeregisterImage`) — a deletion the CLOCK owes the entry, due
    /// at the exit instant whether or not anybody calls, so
    /// [`Self::materialize_due_transitions`] applies it.
    ///
    /// The TAG bytes the deleted image held are given back, and only those. The rest of
    /// what `image_payload_bytes` charged [`Self::insert_image`] for stays counted against
    /// `MAX_STORED_BYTES`, because this store keeps the modelled [`Image`] and not the
    /// `ImageSpec` that charge was computed from: releasing a figure recomputed from the
    /// stored image would be a DIFFERENT number, and over-releasing would let the store
    /// hold more bytes than the budget allows. Keeping it counted is the conservative
    /// direction — it can only refuse a later write earlier, never admit one the budget
    /// forbids.
    pub fn purge_expired_recycled_images(&mut self, now: SystemTime) {
        let mut released = 0usize;
        let mut purged: Vec<String> = Vec::new();
        self.images_in_recycle_bin.retain(|image_id, record| {
            if !record.has_expired(now) {
                return true;
            }
            released = released.saturating_add(tag_list_bytes(
                record.image.tags.as_deref().unwrap_or_default(),
            ));
            purged.push(image_id.clone());
            false
        });
        // The UEFI variable store of a permanently deleted AMI goes with it: it is a fact of
        // that image, held beside it under the same id, and nothing can restore the image
        // any more. Its bytes stay counted for the reason given above.
        for image_id in purged {
            self.image_uefi_data.remove(&image_id);
        }
        self.release_stored_bytes(released);
    }
}

/// One AMI the account deregistered that is currently in the Recycle Bin — see the module
/// docs.
#[derive(Debug, Clone)]
pub struct RecycledImageRecord {
    /// The whole [`Image`] the account last saw, so a restore puts back the same image
    /// rather than a rebuilt copy.
    pub image: Image,
    /// "The date and time when the AMI entered the Recycle Bin"
    /// (`ImageRecycleBinInfo$RecycleBinEnterTime`).
    pub recycle_bin_enter_time: SystemTime,
    /// "The date and time when the AMI is to be permanently deleted from the Recycle Bin"
    /// (`ImageRecycleBinInfo$RecycleBinExitTime`): the enter time plus the retention period
    /// of the Recycle Bin rule that matched the deregistration.
    pub recycle_bin_exit_time: SystemTime,
}

impl RecycledImageRecord {
    /// This entry as `ListImagesInRecycleBin` reports it: the modelled
    /// [`ImageRecycleBinInfo`], read off the stored image rather than rebuilt, so the name
    /// and description reported are the ones the AMI itself carries.
    pub fn view(&self) -> ImageRecycleBinInfo {
        ImageRecycleBinInfo {
            image_id: self.image.image_id.clone(),
            name: self.image.name.clone(),
            description: self.image.description.clone(),
            recycle_bin_enter_time: Some(system_time_to_date_time(self.recycle_bin_enter_time)),
            recycle_bin_exit_time: Some(system_time_to_date_time(self.recycle_bin_exit_time)),
        }
    }

    /// Has this entry's retention period run out as of `now` — is its permanent deletion
    /// due?
    pub fn has_expired(&self, now: SystemTime) -> bool {
        now >= self.recycle_bin_exit_time
    }
}

/// The entry the Recycle Bin holds for the AMI `image_id`, for the calling account.
///
/// The bin is per-ACCOUNT, not per-visibility: an AMI gets there by its OWNER deregistering
/// it ("if a deregistered EBS-backed AMI matches a Recycle Bin retention rule, it moves to
/// the Recycle Bin", `DeregisterImage`), and only that owner can see or restore it. So the
/// filter here is ownership — not the broader `image_is_visible` rule a `DescribeImages`
/// lookup uses, under which another account's PUBLIC image would resolve — and an entry
/// another account owns is reported as no entry at all.
pub fn recycled_image<'store>(
    store: &'store ComAmazonawsEc2Amazonec2State,
    image_id: &str,
) -> Option<&'store RecycledImageRecord> {
    let account = ::engine::emulator_account_id();
    store
        .images_in_recycle_bin
        .get(image_id)
        .filter(|record| record.image.owner_id.as_deref() == Some(account))
}

/// Refuse an AMI id a request named that the Recycle Bin holds no entry of this account's
/// for: HTTP 400 `InvalidAMIID.Malformed`.
///
/// A Recycle Bin operation resolves an `ImageId` only among the entries the bin holds, and
/// answers every id it cannot resolve there with that ONE code — EC2 refuses even a
/// well-formed AMI id with no entry behind it as malformed here, rather than with the
/// `InvalidAMIID.NotFound` an image-store lookup (`require_visible_image`) answers for a
/// well-formed id the account can see no image for. So all four are the same refusal: an id
/// that is not an AMI id at all, a well-formed id nothing was ever stored under, a live AMI
/// nobody deregistered, and an entry whose retention period has already run out and has
/// therefore been permanently deleted. An entry another account owns is the same refusal
/// too ([`recycled_image`]).
///
/// The refusal is the one the crate's STRICT image-id validators answer
/// ([`super::unresolvable_image_id`]) — `The image ID '<id>' is malformed`, which names the
/// resource KIND the id could not be read as — built from the request's own id. A Recycle Bin
/// operation resolves its `ImageId` the strict way, so it words the refusal the strict way
/// too, rather than through the resource-agnostic `Invalid id: "<id>"` template an operation
/// whose id member is checked as a request parameter answers.
pub fn require_recycled_image<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    image_id: &str,
) -> Result<(), Scaffolded<E>> {
    if recycled_image(store, image_id).is_none() {
        return Err(unresolvable_image_id(image_id));
    }
    Ok(())
}
