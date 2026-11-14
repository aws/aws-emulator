// SPDX-License-Identifier: CC0-1.0

//! The EBS volume store: the stored record, its projection into the modelled `Volume` shape,
//! the attachments it carries and the modification requests made against it.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every
//! other primitive there.

use super::*;

/// The `Volume` shape, under the name the scaffold generated for it.
///
/// `com.amazonaws.ec2#Volume` is both `CreateVolume`'s output shape and the member
/// type of `DescribeVolumes`'s `Volumes` list, so the generator emitted it once under
/// the output name. This alias lets the code read as the model does, the same way
/// [`ReservationView`] does for `Reservation`.
pub type VolumeView = CreateVolumeOutput;

/// A volume that is being created and cannot be attached yet: `VolumeState`'s
/// `creating` (`smithy_query(query="shape",shape="VolumeState")`).
pub const VOLUME_STATE_CREATING: &str = "creating";

/// A created volume that is not attached to an instance: `VolumeState`'s `available`.
pub const VOLUME_STATE_AVAILABLE: &str = "available";

/// A created volume that an instance holds an attachment of: `VolumeState`'s `in-use`.
pub const VOLUME_STATE_IN_USE: &str = "in-use";

/// An attachment an instance holds: `VolumeAttachmentState`'s `attached`
/// (`crate::ops::types::VOLUME_ATTACHMENT_STATE_VALUES` is the generated catalog).
pub const VOLUME_ATTACHMENT_STATE_ATTACHED: &str = "attached";

/// An attachment `AttachVolume` has started bringing up: `VolumeAttachmentState`'s
/// `attaching`. The move to [`VOLUME_ATTACHMENT_STATE_ATTACHED`] is owed by the clock
/// ([`TRANSITION_DELAY`], through [`VolumeRecord::attaching_at`]), so a caller that asks
/// straight after the attach still sees the transient state.
pub const VOLUME_ATTACHMENT_STATE_ATTACHING: &str = "attaching";

/// An attachment `DetachVolume` has started taking down: `VolumeAttachmentState`'s
/// `detaching`. The move to `detached` is owed by the clock ([`TRANSITION_DELAY`]), at
/// which point the attachment is gone and the volume is `available` again.
pub const VOLUME_ATTACHMENT_STATE_DETACHING: &str = "detaching";

/// The EBS card an attachment sits on when the request names none: "The default EBS card
/// index is 0" (`AttachVolumeRequest$EbsCardIndex`).
pub const DEFAULT_EBS_CARD_INDEX: i32 = 0;

/// Whether an attachment made by `AttachVolume` is deleted with the instance.
///
/// "When you attach a volume to a running instance, the `DeleteOnTermination` attribute is
/// set to `false` by default" (Amazon EBS User Guide, "Attach an Amazon EBS volume to an
/// instance") — only a volume a LAUNCH creates for its instance defaults to `true`
/// (`EbsBlockDevice$DeleteOnTermination`, where the launch path reads it).
pub const ATTACHMENT_DELETE_ON_TERMINATION: bool = false;

/// The longest `Device` an attachment may name.
///
/// CHOSEN, like [`MAX_KMS_KEY_ID_CHARS`] and for the same reason: the model declares no
/// `@length` on the member, the string is caller-chosen and it is STORED on the volume, so
/// without a bound one request could keep an arbitrarily long string per attachment. Every
/// device name AWS documents ("Device names on Linux instances": `/dev/sd[a-z]`,
/// `/dev/xvd[b-c][a-z]`, `/dev/nvme[0-26]n1`) is far inside it.
pub const MAX_DEVICE_NAME_CHARS: usize = 32;

/// How many instances one Multi-Attach enabled volume may be attached to at once:
/// "Multi-Attach enabled volumes can be attached to up to 16 instances" (Amazon EBS User
/// Guide, "Attach a volume to multiple instances with Amazon EBS Multi-Attach").
pub const MULTI_ATTACH_INSTANCE_LIMIT: usize = 16;

/// EC2's error code for an attach of a volume that is not in the Availability Zone of the
/// instance it would be attached to (EC2 API error codes, "InvalidVolume.ZoneMismatch").
pub const INVALID_VOLUME_ZONE_MISMATCH: &str = "InvalidVolume.ZoneMismatch";

/// EC2's error code for an attach that would take an instance past the number of EBS
/// volumes its instance type can hold attached: "If you exceed the volume attachment limit
/// for an instance type, the attachment request fails with the AttachmentLimitExceeded
/// error" (`AttachVolume`).
pub const ATTACHMENT_LIMIT_EXCEEDED: &str = "AttachmentLimitExceeded";

/// Refuse an attach whose volume and instance are in different Availability Zones: HTTP 400
/// `InvalidVolume.ZoneMismatch`.
///
/// "The volume and instance must be within the same Availability Zone"
/// (`AttachVolumeRequest$VolumeId`), so the refusal names the two resources the REQUEST
/// named and states that they are not in one zone. Which zone each one is in is state the
/// caller can read off the resources themselves, and is not part of the wording.
pub fn invalid_volume_zone_mismatch<E>(
    volume_id: &str,
    instance_id: &str,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_VOLUME_ZONE_MISMATCH,
        400,
        format!(
            "The volume '{volume_id}' is not in the same availability zone as instance \
             '{instance_id}'"
        ),
    )
}

/// Refuse an attach that would take a resource past the attachments it can hold: HTTP 400
/// `AttachmentLimitExceeded`.
pub fn attachment_limit_exceeded<E>(message: impl Into<String>) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(ATTACHMENT_LIMIT_EXCEEDED, 400, message)
}

/// How many EBS volumes an instance of `instance_type` can hold attached, or `None` for a
/// type this service derives no facts for.
///
/// Read off the ONE derivation of an instance type's facts ([`instance_type_info`], whose
/// `EbsInfo$MaximumEbsAttachments` is what `DescribeInstanceTypes` reports), so the limit
/// an attach is refused by and the limit the describe publishes cannot disagree.
pub fn ebs_attachment_limit(instance_type: &str) -> Option<i32> {
    instance_type_info(instance_type)?
        .ebs_info?
        .maximum_ebs_attachments
}

/// How many network interfaces an instance of `instance_type` can hold attached, or `None`
/// for a type this service derives no facts for.
///
/// Read off the ONE derivation of an instance type's facts ([`instance_type_info`], whose
/// `NetworkInfo$MaximumNetworkInterfaces` is what `DescribeInstanceTypes` reports) the way
/// [`ebs_attachment_limit`] beside it reads the volume limit, so the limit an attach is
/// refused by and the limit the describe publishes cannot disagree.
pub fn network_interface_limit(instance_type: &str) -> Option<i32> {
    instance_type_info(instance_type)?
        .network_info?
        .maximum_network_interfaces
}

/// The volume type `CreateVolume` uses when the request names none: "Default: gp2"
/// (`CreateVolumeRequest$VolumeType`).
pub const DEFAULT_VOLUME_TYPE: &str = "gp2";

/// The one resource type `CreateVolume`'s `TagSpecifications` can name: "The tags to
/// apply to the volume during creation" (`CreateVolumeRequest$TagSpecifications`).
pub const VOLUME_TAGGABLE_RESOURCE: &str = "volume";

/// How many EBS volumes this emulator stores before `CreateVolume` is refused.
///
/// CHOSEN, like [`MAX_STORED_INSTANCES`] and for the same reason: nothing in this
/// emulator reclaims a stored volume, so the store needs a ceiling that is not the
/// process's memory. It is well above any published per-Region volume quota a test
/// would exercise.
///
/// A count ceiling is not a memory bound on its own — see [`MAX_STORED_BYTES`] — so the
/// caller-chosen bytes of each volume are charged against that budget too
/// ([`volume_payload_bytes`]).
pub const MAX_STORED_VOLUMES: usize = 4096;

/// The longest `KmsKeyId` `CreateVolume` accepts.
///
/// AWS KMS bounds a key identifier — a key id, an alias, or either as an ARN — at 2048
/// characters (`KeyId` in the AWS KMS API Reference). The EC2 model declares no
/// `@length` on the member, so the handler is the only place the bound can exist, and
/// without one a single request could store an arbitrarily long string per volume.
pub const MAX_KMS_KEY_ID_CHARS: usize = 2048;

/// The size range, in GiB, each volume type accepts, as `(type, min, max)`.
///
/// Source: `CreateVolumeRequest$Size` — "Valid sizes: gp2: 1 - 16,384 GiB gp3: 1 -
/// 65,536 GiB io1: 4 - 16,384 GiB io2: 4 - 65,536 GiB st1 and sc1: 125 - 16,384 GiB
/// standard: 1 - 1024 GiB". Every value of the modelled `VolumeType` enum
/// ([`crate::ops::types::VOLUME_TYPE_VALUES`]) appears exactly once.
pub const VOLUME_SIZE_RANGES: [(&str, i32, i32); 7] = [
    ("gp2", 1, 16_384),
    ("gp3", 1, 65_536),
    ("io1", 4, 16_384),
    ("io2", 4, 65_536),
    ("sc1", 125, 16_384),
    ("st1", 125, 16_384),
    ("standard", 1, 1_024),
];

/// The size range `volume_type` accepts, or `None` for a type the model does not
/// publish one for.
pub fn volume_size_range(volume_type: &str) -> Option<(i32, i32)> {
    VOLUME_SIZE_RANGES
        .iter()
        .find(|(name, _, _)| *name == volume_type)
        .map(|(_, min, max)| (*min, *max))
}

/// The IOPS range each volume type that provisions IOPS accepts, as
/// `(type, min, max)`.
///
/// Source: `CreateVolumeRequest$Iops` — "Required for io1 and io2 volumes. Optional
/// for gp3 volumes. Omit for all other volume types. Valid ranges: gp3: 3,000
/// (default) - 80,000 IOPS io1: 100 - 64,000 IOPS io2: 100 - 256,000 IOPS". A type
/// absent from this table takes no `Iops` at all.
pub const VOLUME_IOPS_RANGES: [(&str, i32, i32); 3] =
    [("gp3", 3_000, 80_000), ("io1", 100, 64_000), ("io2", 100, 256_000)];

/// The IOPS range `volume_type` accepts, or `None` for a type that takes none.
pub fn volume_iops_range(volume_type: &str) -> Option<(i32, i32)> {
    VOLUME_IOPS_RANGES
        .iter()
        .find(|(name, _, _)| *name == volume_type)
        .map(|(_, min, max)| (*min, *max))
}

/// The volume types `Iops` is REQUIRED for: "Required for io1 and io2 volumes"
/// (`CreateVolumeRequest$Iops`) — and the same two types Multi-Attach is supported
/// with: "This parameter is supported with io1 and io2 volumes only"
/// (`CreateVolumeRequest$MultiAttachEnabled`).
pub const PROVISIONED_IOPS_VOLUME_TYPES: [&str; 2] = ["io1", "io2"];

/// The IOPS a `gp3` volume gets when the request names none: "gp3: 3,000 (default)"
/// (`CreateVolumeRequest$Iops`).
pub const DEFAULT_GP3_IOPS: i32 = 3_000;

/// The one volume type `Throughput` applies to: "Supported for gp3 volumes only. Omit
/// for all other volume types" (`CreateVolumeRequest$Throughput`).
pub const THROUGHPUT_VOLUME_TYPE: &str = "gp3";

/// The throughput range a `gp3` volume accepts, in MiB/s: "Valid Range: 125 - 2000
/// MiB/s" (`CreateVolumeRequest$Throughput`).
pub const GP3_THROUGHPUT_RANGE: (i32, i32) = (125, 2_000);

/// The throughput a `gp3` volume gets when the request names none: the bottom of
/// [`GP3_THROUGHPUT_RANGE`], which is what AWS provisions by default.
pub const DEFAULT_GP3_THROUGHPUT: i32 = GP3_THROUGHPUT_RANGE.0;

/// The `VolumeAttributeName` enum's `autoEnableIO` member.
///
/// Named here so the handlers and [`VolumeRecord`] spell one attribute one way; the
/// generated catalog of every member is
/// `crate::ops::types::VOLUME_ATTRIBUTE_NAME_VALUES`, which `DescribeVolumeAttribute`
/// validates its `Attribute` against and the crate's tests pin these two constants to.
pub const VOLUME_ATTRIBUTE_AUTO_ENABLE_IO: &str = "autoEnableIO";

/// The `VolumeAttributeName` enum's `productCodes` member (see
/// [`VOLUME_ATTRIBUTE_AUTO_ENABLE_IO`] for the generated catalog these come from).
pub const VOLUME_ATTRIBUTE_PRODUCT_CODES: &str = "productCodes";

/// The `autoEnableIO` a volume is created with.
///
/// "By default, all I/O operations for the volume are suspended when the data on the
/// volume is determined to be potentially inconsistent […] You can change the default
/// behavior to resume I/O operations" (`ModifyVolumeAttribute`) — so I/O is NOT
/// auto-enabled until a caller says so.
pub const AUTO_ENABLE_IO_AT_CREATION: bool = false;

/// One stored EBS volume: the modelled [`VolumeView`] every operation that reports
/// one answers with, plus the instant the clock owes it its `creating` ->
/// `available` move.
///
/// The view holds no `State`: the state is a fact of the volume's age, so
/// [`VolumeRecord::view_at`] derives it rather than letting two writers store two
/// different words for it.
#[derive(Debug, Clone)]
pub struct VolumeRecord {
    /// The volume as the model shapes it, with `State` left for `view_at` to fill.
    pub view: VolumeView,
    /// The instant from which this volume's `creating` -> `available` move is due.
    /// Monotonic ([`Instant`]), because it is only ever compared against the clock
    /// and never reported, so a wall-clock adjustment cannot move a volume back to
    /// `creating`.
    pub available_at: Instant,
    /// `autoEnableIO`: "whether the volume should be auto-enabled for I/O operations"
    /// (`ModifyVolumeAttributeRequest$AutoEnableIO`), which `ModifyVolumeAttribute`
    /// sets and `DescribeVolumeAttribute` reports.
    ///
    /// The modelled [`VolumeView`] carries no member for it — no describe of a volume
    /// reports it — so it lives beside the view the way [`VpcAttributes`] lives beside
    /// the [`Vpc`] shape, but ON this record rather than in a second map, so the delete
    /// that removes a volume removes its attributes with it. Starts at
    /// [`AUTO_ENABLE_IO_AT_CREATION`], so a describe never has to invent a default for a
    /// volume nobody has modified.
    pub auto_enable_io: bool,
    /// Whether this volume's I/O operations are enabled.
    ///
    /// Amazon EBS disables I/O for a volume "because the data on the volume was
    /// potentially inconsistent" (`EnableVolumeIO`), and `EnableVolumeIO` is the call that
    /// turns it back on — so that operation's precondition is a fact OF THE VOLUME, held
    /// here because the modelled `Volume` shape carries no member for it (it is reported
    /// by `DescribeVolumeStatus`'s `io-enabled` check). Nothing in this emulator finds a
    /// volume's data inconsistent, so a volume is stored with its I/O enabled and the
    /// enable is refused as out of state until something disables it.
    pub io_enabled: bool,
    /// For each attachment of this volume that is being taken down, the instant its
    /// `detaching` -> `detached` move falls due, keyed by the `(InstanceId, Device)` pair
    /// that identifies the attachment — an instance can hold the same Multi-Attach volume
    /// at more than one device, so the instance alone does not name one attachment.
    ///
    /// `DetachVolume` answers an attachment in the non-terminal `detaching` state, so the
    /// clock owes it `detached` [`TRANSITION_DELAY`] later whether or not anybody calls
    /// again — the same span, read off the same monotonic clock, as this volume's
    /// `creating` -> `available` move. A detached attachment is not a state of an
    /// attachment the volume still holds: it is the attachment being gone, so
    /// [`VolumeRecord::attachments_at`] drops it once the move is due rather than
    /// reporting a `detached` row for ever, and the next writer materialises that removal
    /// ([`VolumeRecord::materialize_due_detachments`]) so no entry outlives the attachment
    /// it was made for.
    pub detaching_at: BTreeMap<(String, String), Instant>,
    /// For each attachment of this volume that is being brought up, the instant its
    /// `attaching` -> `attached` move falls due, keyed by the same `(InstanceId, Device)`
    /// pair as [`VolumeRecord::detaching_at`].
    ///
    /// `AttachVolume` answers an attachment in the non-terminal `attaching` state, so the
    /// clock owes it `attached` [`TRANSITION_DELAY`] later whether or not anybody calls
    /// again — the same span, read off the same monotonic clock, as this volume's
    /// `creating` -> `available` move. Unlike a finished detach, a finished attach leaves
    /// the attachment in place and only moves its state, so
    /// [`VolumeRecord::attachments_at`] reports it `attached` once the move is due and the
    /// next writer materialises that
    /// ([`VolumeRecord::materialize_due_attachments`]).
    pub attaching_at: BTreeMap<(String, String), Instant>,
    /// The instant at which EC2 takes this volume back, because the KMS key it was to be
    /// encrypted under will not produce its data key
    /// ([`crate::state::volume_key_refusal`]) — and `None` for every volume whose key will,
    /// which is every volume that is to be created.
    ///
    /// A volume is created before its data key is wrapped, so a request under an unusable
    /// key is ACCEPTED and answers a `creating` volume; what the key refuses is the volume
    /// coming into being, so it never reaches `available` and is withdrawn instead. The
    /// withdrawal falls due at the same instant the `creating` -> `available` move would
    /// have ([`TRANSITION_DELAY`]), so a caller that describes the volume straight after the
    /// create still sees `creating` and a caller that waits sees the volume gone — which is
    /// owed at that instant whether or not anybody calls, so the next writer applies it
    /// ([`ComAmazonawsEc2Amazonec2State::withdraw_due_unencryptable_volumes`]) and every
    /// reader observes it through the same reading
    /// ([`ComAmazonawsEc2Amazonec2State::volume_is_withdrawn`]).
    pub withdrawn_at: Option<Instant>,
}

impl VolumeRecord {
    /// A volume this store has just created: attached to nothing, with its I/O enabled,
    /// reaching `available` at `available_at`.
    pub fn created(view: VolumeView, available_at: Instant) -> Self {
        Self {
            view,
            available_at,
            // Every path that MINTS a volume starts its `autoEnableIO` here, so no
            // creator can invent a different default and `ModifyVolumeAttribute` is the
            // one operation that moves it.
            auto_enable_io: AUTO_ENABLE_IO_AT_CREATION,
            io_enabled: true,
            detaching_at: BTreeMap::new(),
            attaching_at: BTreeMap::new(),
            // Every volume this store mints is one its encryption key will serve: the one
            // path that learns otherwise is `CreateVolume`, which sets the instant itself.
            withdrawn_at: None,
        }
    }

    /// Whether this volume has already been withdrawn at `now`, because the key it was to be
    /// encrypted under refused its data key. A withdrawn volume is GONE — no read reports it
    /// and no call resolves it — so this is asked wherever a volume is resolved, through
    /// [`ComAmazonawsEc2Amazonec2State::volume_is_withdrawn`].
    pub fn is_withdrawn_at(&self, now: Instant) -> bool {
        self.withdrawn_at.is_some_and(|due_at| now >= due_at)
    }

    /// This volume as of `now`: the stored view with the `State` the clock has
    /// reached and the attachments it still holds. A read observes the transitions the
    /// clock already owed without writing anything.
    pub fn view_at(&self, now: Instant) -> VolumeView {
        let mut view = self.view.clone();
        view.attachments = Some(self.attachments_at(now));
        view.state = Some(self.state_at(now).to_string());
        view
    }

    /// The attachments this volume holds at `now`: every stored attachment except one
    /// whose `detaching` -> `detached` move the clock has already made due, each reporting
    /// the state the clock has reached — an `attaching` one whose move is due reads
    /// `attached`.
    pub fn attachments_at(&self, now: Instant) -> Vec<AttachVolumeOutput> {
        self.view
            .attachments
            .iter()
            .flatten()
            .filter(|attachment| !self.detach_is_due(attachment, now))
            .map(|attachment| {
                let mut attachment = attachment.clone();
                if self.attach_is_due(&attachment, now) {
                    attachment.state = Some(VOLUME_ATTACHMENT_STATE_ATTACHED.to_string());
                }
                attachment
            })
            .collect()
    }

    /// The attachments this volume holds TO ONE INSTANCE at `now`: the same reading
    /// [`Self::attachments_at`] gives, narrowed to the attachments whose `InstanceId` is
    /// `instance_id`.
    ///
    /// What an instance's block device mappings are read through
    /// ([`ComAmazonawsEc2Amazonec2State::instance_block_device_mappings`]), so the device,
    /// the volume and the attachment state an instance reports and the ones
    /// `DescribeVolumes` reports are the same record. The filter runs before the clone, so a
    /// volume attached elsewhere costs one comparison.
    pub fn attachments_to_instance_at(
        &self,
        instance_id: &str,
        now: Instant,
    ) -> Vec<AttachVolumeOutput> {
        self.view
            .attachments
            .iter()
            .flatten()
            .filter(|attachment| attachment.instance_id.as_deref() == Some(instance_id))
            .filter(|attachment| !self.detach_is_due(attachment, now))
            .map(|attachment| {
                let mut attachment = attachment.clone();
                if self.attach_is_due(&attachment, now) {
                    attachment.state = Some(VOLUME_ATTACHMENT_STATE_ATTACHED.to_string());
                }
                attachment
            })
            .collect()
    }

    /// Whether the detach of `attachment` was owed by `now`.
    pub(super) fn detach_is_due(&self, attachment: &AttachVolumeOutput, now: Instant) -> bool {
        self.detaching_at
            .get(&Self::attachment_key(attachment))
            .is_some_and(|due_at| now >= *due_at)
    }

    /// Whether `attachment` is one being brought up whose `attached` move was owed by
    /// `now`. An attachment stored in any other state owes no such move — a detach that has
    /// started takes the attachment the other way, and a stored `attached` one has arrived
    /// already.
    pub(super) fn attach_is_due(&self, attachment: &AttachVolumeOutput, now: Instant) -> bool {
        attachment.state.as_deref() == Some(VOLUME_ATTACHMENT_STATE_ATTACHING)
            && self
                .attaching_at
                .get(&Self::attachment_key(attachment))
                .is_none_or(|due_at| now >= *due_at)
    }

    /// The `(InstanceId, Device)` pair that identifies `attachment` among the attachments
    /// of one volume — the ONE reading of that key, so a writer and a reader cannot key
    /// the same attachment two ways.
    pub fn attachment_key(attachment: &AttachVolumeOutput) -> (String, String) {
        (
            attachment.instance_id.clone().unwrap_or_default(),
            attachment.device.clone().unwrap_or_default(),
        )
    }

    /// The marker that makes `attachment` an attachment to an Amazon Web Services-managed
    /// resource rather than to an instance of this account, if it is one.
    ///
    /// "The ARN of the Amazon Web Services-managed resource to which the volume is
    /// attached" (`VolumeAttachment$AssociatedResource`) and "the service principal of the
    /// Amazon Web Services service that owns the underlying resource"
    /// (`VolumeAttachment$InstanceOwningService`) are each "returned only for volumes that
    /// are attached to Amazon Web Services-managed resources", so either one alone marks
    /// the attachment as one of those — and such an attachment reports no `InstanceId` or
    /// `Device` at all ("if the volume is attached to an Amazon Web Services-managed
    /// resource, this parameter returns null"). One reading, so the refusal and the message
    /// that names the resource cannot disagree about which marker is set.
    pub fn managed_resource_of(attachment: &AttachVolumeOutput) -> Option<&str> {
        [
            attachment.associated_resource.as_deref(),
            attachment.instance_owning_service.as_deref(),
        ]
        .into_iter()
        .flatten()
        .find(|marker| !marker.is_empty())
    }

    /// Apply the detachments the clock has already made due: the attachments they took
    /// down are no longer the volume's, and nothing is owed on them any more.
    ///
    /// A WRITER calls this, which is the only thing that may materialise it — a report
    /// observes the same removal through [`VolumeRecord::attachments_at`] without writing.
    /// It keeps the stored record converging on what readers already see, so no entry
    /// outlives the attachment it was made for and a later attachment of the same volume
    /// to the same device cannot inherit a past-due one.
    pub fn materialize_due_detachments(&mut self, now: Instant) {
        self.detaching_at.retain(|_, due_at| now < *due_at);
        let owed = &self.detaching_at;
        if let Some(attachments) = self.view.attachments.as_mut() {
            attachments.retain(|attachment| {
                // The ONE reading of which attachment this is, the same key the owed
                // instants above are stored under.
                owed.contains_key(&Self::attachment_key(attachment))
                    || attachment.state.as_deref() != Some(VOLUME_ATTACHMENT_STATE_DETACHING)
            });
        }
        // An attachment a finished detach took down owes no `attached` move either, so the
        // instants kept for the attachments that are gone go with them: both maps converge
        // on the attachments the volume actually holds.
        let held: ::std::collections::BTreeSet<(String, String)> = self
            .view
            .attachments
            .iter()
            .flatten()
            .map(Self::attachment_key)
            .collect();
        self.attaching_at.retain(|key, _| held.contains(key));
    }

    /// Apply the attachments the clock has already brought up: each one's state is the
    /// `attached` a reader is already being shown, and nothing is owed on it any more.
    ///
    /// A WRITER calls this, which is the only thing that may materialise it — a report
    /// observes the same state through [`VolumeRecord::attachments_at`] without writing. It
    /// also drops any owed instant whose attachment the volume no longer holds, so no entry
    /// outlives the attachment it was made for and a later attachment at the same device
    /// cannot inherit a past-due one.
    pub fn materialize_due_attachments(&mut self, now: Instant) {
        let due: Vec<(String, String)> = self
            .attaching_at
            .iter()
            .filter(|(_, due_at)| now >= **due_at)
            .map(|(key, _)| key.clone())
            .collect();
        if let Some(attachments) = self.view.attachments.as_mut() {
            for attachment in attachments.iter_mut() {
                if attachment.state.as_deref() == Some(VOLUME_ATTACHMENT_STATE_ATTACHING)
                    && due.contains(&Self::attachment_key(attachment))
                {
                    attachment.state = Some(VOLUME_ATTACHMENT_STATE_ATTACHED.to_string());
                }
            }
        }
        let held: ::std::collections::BTreeSet<(String, String)> = self
            .view
            .attachments
            .iter()
            .flatten()
            .map(Self::attachment_key)
            .collect();
        self.attaching_at
            .retain(|key, due_at| now < *due_at && held.contains(key));
    }

    /// The volume's `VolumeState` at `now`.
    ///
    /// A volume being created is `creating`; one an instance still holds an attachment of
    /// is `in-use` ("The volume state", `Volume$State`, over `VolumeState`'s members); and
    /// one attached to nothing is `available`.
    pub fn state_at(&self, now: Instant) -> &'static str {
        if now < self.available_at {
            VOLUME_STATE_CREATING
        } else if self.attachments_at(now).is_empty() {
            VOLUME_STATE_AVAILABLE
        } else {
            VOLUME_STATE_IN_USE
        }
    }

    /// This volume as `CreateVolume` reports it: [`VolumeRecord::view_at`] without
    /// the members the model says that operation does not return — "This parameter
    /// is not returned by CreateVolume" on `Attachments`, `FastRestored` and
    /// `SseType` — and without `Operator`.
    ///
    /// Those first three are dropped on the MODEL's word. `Operator` is dropped on
    /// RECORDED behaviour only: the model notes nothing about it, and a recorded
    /// `CreateVolume` answer carries no `operator` element where a recorded describe of the
    /// same volume does. The two justifications are named apart here so a later reader can
    /// tell which members the model decides and which one the corpus does.
    pub fn create_response(&self, now: Instant) -> VolumeView {
        VolumeView {
            attachments: None,
            fast_restored: None,
            sse_type: None,
            operator: None,
            ..self.view_at(now)
        }
    }

    /// This volume's `productCodes` attribute: the Amazon Web Services Marketplace
    /// product codes it carries.
    ///
    /// "Any Amazon Web Services Marketplace product codes from the snapshot are
    /// propagated to the volume" (`CreateVolume`) — a volume's product codes are the ones
    /// the snapshot it was restored from carries, so a volume created empty has none.
    /// Every volume this emulator stores IS created empty: it holds no snapshots (every
    /// EBS snapshot operation is still a stub, and `CreateVolume` refuses any `SnapshotId`
    /// through [`unresolvable_snapshot_id`]), so no record has a source snapshot and there
    /// is nothing to propagate. This is the one place that derivation lives: when a
    /// snapshot store lands, look the record's own `SnapshotId` up here.
    pub fn product_codes(&self) -> Vec<ProductCode> {
        Vec::new()
    }
}

/// The caller-chosen bytes one stored volume holds: its tags, the KMS key the request
/// named and the `ClientToken` the token map keeps for it. Everything else on the record
/// — the id, the ARN, the account, the zone, the sizes and the states — is derived by
/// this module or bounded by the model, so it is fixed-size.
///
/// The ONE definition of that charge: `CreateVolume` asks it before the write and charges
/// exactly it, and a removal path must give back the same number.
pub fn volume_payload_bytes(
    tags: &[Tag],
    kms_key_id: Option<&str>,
    client_token: Option<&str>,
) -> usize {
    tag_list_bytes(tags)
        .saturating_add(kms_key_id.map_or(0, str::len))
        .saturating_add(client_token.map_or(0, str::len))
}

/// The id prefix of an EBS volume, as [`volume_id_is_well_formed`] reads one and as a
/// refusal names the form it expected.
pub const VOLUME_ID_PREFIX: &str = "vol";

/// The name EC2 gives `DescribeVolumesRequest$VolumeIds` when it refuses one of its entries
/// at the PARAMETER layer: "Value (…) for parameter volumes is invalid. Expected:
/// 'vol-...'." — the member's wire name is `VolumeId.N`, and `volumes` is what the service
/// calls the parameter those values make up.
pub const VOLUME_IDS_PARAMETER: &str = "volumes";

/// Whether `id` is a volume id EC2 could have ISSUED.
///
/// A volume id is `vol-` followed by 8 or 17 lowercase hexadecimal digits — the short and
/// the long id form, as the model's own `^vol-([0-9a-f]{8}|[0-9a-f]{17})$` style id patterns
/// read — and the LONG form carries 68 digits' worth of a 64-bit identifier, so an issued
/// long id always reads `vol-0…` and is never the all-zero placeholder. EC2 answers a value
/// outside that form as a bad id BEFORE it looks the id up, which is a different answer from
/// `InvalidVolume.NotFound` for an id inside it the account holds no volume for: a client
/// that cannot tell a value EC2 cannot read from a missing volume goes on polling for a
/// volume that can never appear.
///
/// WHICH bad-id code that is belongs to the OPERATION, not to this reader: see
/// [`invalid_id_parameter_value`].
///
/// The rule itself is [`issued_id_is_well_formed`] — the reading `DescribeVolumes` already
/// holds its `VolumeIds` entries to, and the one every resource kind recorded refusing a
/// long-form id outside the issued form shares — so the form is defined in exactly one
/// place; only the prefix is this function's own.
pub fn volume_id_is_well_formed(id: &str) -> bool {
    issued_id_is_well_formed(VOLUME_ID_PREFIX, id)
}

/// The sentence EC2 words for a request that asks for Multi-Attach on a volume type that
/// does not support it.
///
/// "This parameter is supported with io1 and io2 volumes only"
/// (`CreateVolumeRequest$MultiAttachEnabled`), and the refusal EC2 answers names the
/// PARAMETER as the wire spells it and the type that cannot carry it: the fixed text is
/// `The parameter multi-attach-enabled is not supported for ` and ` volumes.`, with the
/// volume type taken from the request (or, for a modify, from the volume the request would
/// leave it at). One sentence for the one refusal, so the create, the copy and the modify of
/// a volume cannot word it three ways.
pub fn multi_attach_unsupported_message(volume_type: &str) -> String {
    format!("The parameter multi-attach-enabled is not supported for {volume_type} volumes.")
}

/// Refuse a volume id the request named that this service cannot resolve: a malformed
/// one before an unknown one, the way the service orders the two.
///
/// For an operation that goes on to report or modify the volume, use [`require_volume`]
/// or [`require_volume_mut`] instead: they make the same refusal and hand the record back,
/// so the handler needs no second lookup and no unreachable branch for an id that resolved
/// and then could not be read.
pub fn require_known_volume<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    volume_id: &str,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    require_volume(store, volume_id)?;
    Ok(())
}

/// The ONE definition of how this service resolves a volume id a request named: an id that
/// is not shaped like one is answered as malformed, and a well-formed one this account
/// holds no volume for as not found — the way the service orders the two. A volume whose
/// withdrawal the clock has made due is one the account no longer holds
/// ([`ComAmazonawsEc2Amazonec2State::volume_is_withdrawn`]), so it resolves as not found.
pub fn require_volume<'store, E>(
    store: &'store ComAmazonawsEc2Amazonec2State,
    volume_id: &str,
) -> Result<&'store VolumeRecord, crate::scaffolded::Scaffolded<E>> {
    require_well_formed_volume_id(volume_id)?;
    let now = Instant::now();
    store
        .volumes
        .get(volume_id)
        .filter(|record| !record.is_withdrawn_at(now))
        .ok_or_else(|| invalid_volume_id_not_found(volume_id))
}

/// [`require_volume`] for an operation that WRITES the volume it resolved.
pub fn require_volume_mut<'store, E>(
    store: &'store mut ComAmazonawsEc2Amazonec2State,
    volume_id: &str,
) -> Result<&'store mut VolumeRecord, crate::scaffolded::Scaffolded<E>> {
    // The malformed half of the rule first, shared with `require_volume` rather than
    // repeated, because the mutable borrow of the store cannot be taken before it.
    require_well_formed_volume_id(volume_id)?;
    let now = Instant::now();
    store
        .volumes
        .get_mut(volume_id)
        .filter(|record| !record.is_withdrawn_at(now))
        .ok_or_else(|| invalid_volume_id_not_found(volume_id))
}

/// The first half of that rule on its own: a volume id that is not shaped like one is
/// refused BEFORE it is looked up, so both halves of [`require_volume`] exist once each.
pub(super) fn require_well_formed_volume_id<E>(
    volume_id: &str,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    if volume_id_is_well_formed(volume_id) {
        Ok(())
    } else {
        Err(invalid_volume_id_malformed(volume_id))
    }
}

/// Refuse a request that named something that is not a volume id at all: HTTP 400
/// `InvalidVolumeID.Malformed`, which EC2 answers BEFORE it looks the id up.
///
/// The ONE wording of that refusal: [`require_known_volume`] and [`require_volume`] answer
/// it for an operation that resolves the volume straight away, and an operation that
/// answers its `DryRun` probe between the two halves of the rule calls this and
/// [`invalid_volume_id_not_found`] itself.
pub fn invalid_volume_id_malformed<E>(volume_id: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_VOLUME_ID_MALFORMED,
        400,
        malformed_volume_id_message(volume_id),
    )
}

/// The sentence EC2 refuses an unreadable VOLUME id with: the fixed text `The volume ID '`
/// and `' is malformed`, with the id as the request sent it (cut to the bound every echoed
/// id of this crate is held to, [`echoed_resource_id`]).
///
/// The volume is one of the id kinds AWS words with a sentence of its OWN rather than with
/// the quoted template the other kinds of this crate take ([`invalid_id_message`], whose
/// recorded refusals are of instance, AMI, security group, interface attachment and Spot
/// Fleet request ids): both recorded refusals of a volume id read this way — the one
/// `RestoreVolumeFromRecycleBin` answers (the traffic behind
/// `issues/things-that-now-work/20261022T235108Z-ec2-restorevolumefromrecyclebin-stub.md`)
/// and the one `ModifyVolumeAttribute` answers, whose recorded refusal of a fabricated
/// documentation-shaped id is this code with the message `The volume ID '<the id sent>' is
/// malformed` and NOT the quoted template (the id-form issue this reading comes from asserts
/// that message on the replayed call). One definition for the kind, so no operation that
/// resolves a volume id can word it a second way.
pub fn malformed_volume_id_message(volume_id: &str) -> String {
    format!(
        "The volume ID '{}' is malformed",
        echoed_resource_id(volume_id)
    )
}

/// Refuse a `VolumeId` the Recycle Bin holds no entry for: HTTP 400
/// `InvalidVolumeID.Malformed`, the one code the service answers for every id
/// `RestoreVolumeFromRecycleBin` cannot resolve in the bin — an id that is not shaped like
/// one, a well-formed id the bin never held, one whose retention period has run out, and one
/// the account still has a live volume for.
///
/// The code and the sentence are the kind's own ([`malformed_volume_id_message`]); what is
/// this operation's is WHICH ids it answers them for, the bin holding entries the account's
/// live volume store does not.
pub fn volume_id_not_in_recycle_bin<E>(volume_id: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_VOLUME_ID_MALFORMED,
        400,
        malformed_volume_id_message(volume_id),
    )
}

/// Refuse a request that named a volume id no volume of the account has, the way EC2
/// does: HTTP 400 `InvalidVolume.NotFound`.
pub fn invalid_volume_id_not_found<E>(volume_id: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_VOLUME_ID_NOT_FOUND,
        400,
        format!("The volume '{volume_id}' does not exist."),
    )
}

/// EC2's error code for a request that needs the volume it names to hold no attachment
/// while an instance still holds one (EC2 API error codes, "VolumeInUse").
pub const VOLUME_IN_USE: &str = "VolumeInUse";

/// Refuse an ATTACH of a volume something already holds, where the request cannot be served
/// because of that: HTTP 400 `VolumeInUse`.
///
/// The attach side's own sentence: it names the volume the request named and reports that an
/// instance already holds it, without naming which one. The DELETE side, which refuses
/// because the volume still holds attachments at all, names the holders as the set they are
/// ([`volume_in_use`]) — two wordings of one code, as EC2's per-operation validators have.
pub fn volume_already_attached<E>(volume_id: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        VOLUME_IN_USE,
        400,
        format!("{volume_id} is already attached to an instance"),
    )
}

/// Refuse a request that needs the volume it named to hold NO attachment while something
/// still holds one: HTTP 400 `VolumeInUse`, naming every holder as the set it is.
///
/// "The volume must be in the available state (not attached to an instance)"
/// (`DeleteVolume`) — so the refusal names the volume the request named and what has to let
/// go of it first. A volume can be held by more than one resource at once (a Multi-Attach
/// enabled one is attached to up to [`MULTI_ATTACH_INSTANCE_LIMIT`] instances), and the
/// service renders that collection as a SET: brace-wrapped, its members separated by `, `,
/// which is the form the recorded refusal of this code carries for the one holder it had.
/// The holders come from the attachments the volume holds at the instant of the call, in the
/// order the store holds them, so a volume two instances hold names both.
///
/// The DELETE side's wording. The attach side words the same code its own way
/// ([`volume_already_attached`]), which is what the recordings of the two operations carry.
pub fn volume_in_use<E>(volume_id: &str, holders: &[String]) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        VOLUME_IN_USE,
        400,
        format!(
            "Volume {volume_id} is currently attached to {{{}}}",
            holders.join(", ")
        ),
    )
}

/// EC2's error code for an ATTACHMENT a request names that the volume does not hold (EC2 API
/// error codes, "InvalidAttachment.NotFound").
pub const INVALID_ATTACHMENT_NOT_FOUND: &str = "InvalidAttachment.NotFound";

/// Refuse a request that named an attachment the volume does not hold: HTTP 400
/// `InvalidAttachment.NotFound`.
///
/// The ATTACHMENT, not the volume's state, is what is missing: the volume exists and is in a
/// state that admits the request, and what it does not have is the attachment the request
/// asked about — so this is the attachment-not-found class rather than `IncorrectState`,
/// which is the class for the state of the resource the request named.
///
/// The message is EC2's: the volume, and whichever of the instance and the device the request
/// named — "The volume '<id>' is not attached to instance '<id>' as device '<name>'." Both
/// members are optional on the request, so the clauses the sentence carries are the ones the
/// caller gave, and every variable part comes from the request. Only the BOTH-clause form has a
/// recorded wording behind it; the narrower forms follow the same template with the clause the
/// caller omitted left out, there being no recorded sentence for them.
pub fn invalid_attachment_not_found<E>(
    volume_id: &str,
    instance_id: Option<&str>,
    device: Option<&str>,
) -> crate::scaffolded::Scaffolded<E> {
    let named = match (instance_id, device) {
        (Some(instance_id), Some(device)) => {
            format!(" to instance '{instance_id}' as device '{device}'")
        }
        (Some(instance_id), None) => format!(" to instance '{instance_id}'"),
        (None, Some(device)) => format!(" as device '{device}'"),
        // Defensive: a request that names neither member selects every attachment the
        // resource holds, so a caller reaching this refusal has named at least one of them —
        // a resource holding NO attachment is refused for its state instead
        // ([`incorrect_state_of`]).
        (None, None) => String::new(),
    };
    crate::scaffolded::Scaffolded::service_error(
        INVALID_ATTACHMENT_NOT_FOUND,
        400,
        format!("The volume '{volume_id}' is not attached{named}."),
    )
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether the volume `volume_id` has already been withdrawn at `now`: the account holds
    /// no record for it, or the record it holds is one whose encryption key refused its data
    /// key and whose withdrawal the clock has made due
    /// ([`VolumeRecord::withdrawn_at`]).
    ///
    /// The ONE reading of "is this volume gone", so a reporting operation and a writer agree:
    /// a withdrawn volume is resolved as not found ([`require_volume`]) and reported by no
    /// describe, whether or not a writer has swept it out yet.
    pub fn volume_is_withdrawn(&self, volume_id: &str, now: Instant) -> bool {
        self.volumes
            .get(volume_id)
            .is_none_or(|record| record.is_withdrawn_at(now))
    }

    /// Apply the withdrawals the clock has already made due: every volume whose encryption
    /// key refused its data key is removed once its instant has passed, which was owed at
    /// that instant whether or not anybody called.
    ///
    /// A WRITER calls this, which is the only thing that may materialise it — a report
    /// observes the same removal through [`Self::volume_is_withdrawn`] without writing.
    /// Removal goes through [`Self::remove_volume`], so the record's bytes, its client token
    /// and its modification go back exactly as they would for a delete.
    pub fn withdraw_due_unencryptable_volumes(&mut self, now: Instant) {
        let withdrawn: Vec<String> = self
            .volumes
            .iter()
            .filter(|(_, record)| record.is_withdrawn_at(now))
            .map(|(volume_id, _)| volume_id.clone())
            .collect();
        for volume_id in withdrawn {
            self.remove_volume(&volume_id);
        }
    }

    /// The EBS attachments the instance `instance_id` holds at `now`, as
    /// `(volume id, attachment)` in volume-id order.
    ///
    /// The volume store is where an attachment lives, so "which volumes does this instance
    /// have" is answered by reading it rather than by a second index on the instance: the
    /// attachments are read through [`VolumeRecord::attachments_at`], so one whose detach
    /// the clock has already finished is not counted and one still `attaching` is.
    pub fn instance_attachments_at(
        &self,
        instance_id: &str,
        now: Instant,
    ) -> Vec<(String, AttachVolumeOutput)> {
        self.volumes
            .iter()
            .flat_map(|(volume_id, record)| {
                record
                    .attachments_at(now)
                    .into_iter()
                    .map(move |attachment| (volume_id.clone(), attachment))
            })
            .filter(|(_, attachment)| attachment.instance_id.as_deref() == Some(instance_id))
            .collect()
    }

    /// Remove the EBS volume `volume_id` and answer the record as it was stored, or `None`
    /// when the account holds no such volume.
    ///
    /// Everything that is reached only THROUGH the volume goes with it: the `ClientToken`
    /// that minted it (a token must never answer with a volume that is no longer stored)
    /// and the modification `DescribeVolumesModifications` reports for it, which is a
    /// modification OF this volume and so is nothing once the volume is gone. The bytes the
    /// record held go back to the store's budget on exactly the terms
    /// [`volume_payload_bytes`] took them on — the same accounting
    /// [`Self::move_volume_to_recycle_bin`] makes when the volume leaves the store the
    /// other way.
    pub fn remove_volume(&mut self, volume_id: &str) -> Option<VolumeRecord> {
        let record = self.volumes.remove(volume_id)?;
        let client_token = self
            .volume_client_tokens
            .iter()
            .find(|(_, minted)| minted.as_str() == volume_id)
            .map(|(token, _)| token.clone());
        if let Some(token) = &client_token {
            self.volume_client_tokens.remove(token);
        }
        self.volume_modifications.remove(volume_id);
        let view = &record.view;
        self.release_stored_bytes(volume_payload_bytes(
            view.tags.as_deref().unwrap_or_default(),
            view.kms_key_id.as_deref(),
            client_token.as_deref(),
        ));
        Some(record)
    }
}
