// SPDX-License-Identifier: CC0-1.0

//! The EBS root volume a LAUNCH creates for the instance it stores.
//!
//! "When you launch an instance, the root device volume […] contains the image used to boot
//! the instance": launching from an EBS-backed AMI creates a volume, restored from the
//! snapshot that image's root block device mapping names (`Image$BlockDeviceMappings`,
//! `Image$RootDeviceName`, `Image$RootDeviceType`'s `ebs`). So the volume is a resource the
//! LAUNCH creates, which is why it is minted from
//! [`ComAmazonawsEc2Amazonec2State::insert_instance`] — the one place every launch path
//! stores its instance — rather than inside one operation's handler: the account's volume
//! store is then the single place "which volumes does this instance have" is answered, for
//! `DescribeVolumes` and for `CreateSnapshots` alike.
//!
//! That holds for EVERY instance this emulator stores, including one launched from an AMI
//! the account cannot see: the launch was served, so the instance exists and is EBS-backed,
//! and an instance with no root volume at all would be one no describe of volumes can
//! account for. [`ComAmazonawsEc2Amazonec2State::launched_root_device`] is the one place
//! that root device is resolved — off the image's own mapping where there is one, and off
//! the crate's default EBS-backed Linux image otherwise.

use std::time::{Instant, SystemTime};

use crate::ops::types::{AttachVolumeOutput, BlockDeviceMapping, EbsBlockDevice};

use super::{
    ComAmazonawsEc2Amazonec2State, DEFAULT_GP3_IOPS, DEFAULT_GP3_THROUGHPUT, DEFAULT_VOLUME_TYPE,
    IMAGE_ROOT_DEVICE_TYPE_EBS, ImageSpec, MAX_STORED_VOLUMES, THROUGHPUT_VOLUME_TYPE,
    TRANSITION_DELAY, VOLUME_ATTACHMENT_STATE_ATTACHED, VOLUME_ID_PREFIX, VolumeRecord, VolumeView,
    region_of_zone_name, system_time_to_date_time, unmanaged_operator, zone_id_for_zone_name,
};

impl ComAmazonawsEc2Amazonec2State {
    /// The device an instance launched from `image_id` holds its EBS ROOT volume at, and the
    /// block device facts that volume is restored from — or `None` for a launch that creates
    /// no root volume at all.
    ///
    /// The facts are the AMI's own root block device mapping whenever the account can SEE
    /// that AMI, which is the only reading of them: the device name is `Image$RootDeviceName`
    /// and the rest is the `EbsBlockDevice` the mapping at that device publishes. An AMI the
    /// account cannot see publishes no mapping to read, and the launch still stored an
    /// EBS-backed instance — "when you launch an instance, the root device volume […]
    /// contains the image used to boot the instance" holds for every instance, so one with no
    /// root volume at all would be an instance this emulator reports and no describe of
    /// volumes can account for. Such a launch is therefore given the root device of the
    /// EBS-backed Linux image this crate publishes by default ([`ImageSpec::default`], the row
    /// every published catalog image starts from), rather than a size and a type invented
    /// here; the volume restores no snapshot, because no image was read to name one.
    ///
    /// `None` is for an AMI the account CAN see that is not EBS-backed
    /// (`Image$RootDeviceType`) or publishes no mapping at its root device: an instance store
    /// root device is no volume, so there is nothing to snapshot and nothing to create.
    ///
    /// The ONE answer to "which device is this instance's root one", so the volume a launch
    /// mints ([`Self::mint_launched_root_volume`]) and the boot volume a snapshot call
    /// excludes cannot disagree.
    pub fn launched_root_device(&self, image_id: Option<&str>) -> Option<(String, EbsBlockDevice)> {
        let Some(image) = image_id.and_then(|image_id| self.images.get(image_id)) else {
            return Some(default_root_device());
        };
        // "Indicates whether the root device […] is an Amazon EBS volume or an instance
        // store volume" (`Image$RootDeviceType`): only an EBS-backed AMI restores its root
        // device into a volume.
        if image.root_device_type.as_deref() != Some(IMAGE_ROOT_DEVICE_TYPE_EBS) {
            return None;
        }
        let device_name = image.root_device_name.clone()?;
        let ebs = image
            .block_device_mappings
            .iter()
            .flatten()
            .find(|mapping| mapping.device_name.as_deref() == Some(device_name.as_str()))
            .and_then(|mapping| mapping.ebs.clone())?;
        Some((device_name, ebs))
    }

    /// The root block device the launch of an instance from `image_id` creates its root
    /// volume from, with `mappings` — the launch's own `BlockDeviceMappings` — applied: the
    /// AMI's root device facts ([`Self::launched_root_device`]) overlaid, member by member,
    /// with the entry `mappings` carries for that same device, and encrypted whenever
    /// `region` has EBS encryption on by default.
    ///
    /// "The block device mapping, which defines the EBS volumes and instance store volumes
    /// to attach to the instance at launch" (`RunInstancesRequest$BlockDeviceMappings`), and
    /// a launch may override the AMI's own mapping settings (the EC2 User Guide's "Block
    /// device mappings"): what the launch names for the root device therefore DEFINES that
    /// volume — its size, type, IOPS, throughput, encryption, key and whether it goes with
    /// the instance — and the AMI's own mapping answers only the members the launch left out.
    /// The device NAME is the image's own (`Image$RootDeviceName`): a launch does not move an
    /// AMI's root device, so an entry for any other device names no root volume here.
    ///
    /// "When encryption by default is enabled, […] all new EBS volumes are encrypted"
    /// (`GetEbsEncryptionByDefault`), which is the same Region-scoped setting
    /// `CreateVolume` reads over its own request, so the two cannot disagree about whether a
    /// volume this account creates is encrypted.
    ///
    /// The ONE answer to "which volume does this launch create for the root device", so the
    /// prerequisite a launch is admitted against (a hibernating instance's root volume must
    /// be encrypted) and the volume it then stores
    /// ([`Self::mint_launched_root_volume`]) can never disagree. `None` for a launch that
    /// creates no root volume at all, for the reasons [`Self::launched_root_device`] gives.
    pub fn launch_root_block_device(
        &self,
        image_id: Option<&str>,
        mappings: &[BlockDeviceMapping],
        region: &str,
    ) -> Option<(String, EbsBlockDevice)> {
        let (device_name, image_device) = self.launched_root_device(image_id)?;
        let ebs = match mappings
            .iter()
            .find(|mapping| mapping.device_name.as_deref() == Some(device_name.as_str()))
            .and_then(|mapping| mapping.ebs.as_ref())
        {
            None => image_device,
            Some(named) => EbsBlockDevice {
                delete_on_termination: named
                    .delete_on_termination
                    .or(image_device.delete_on_termination),
                iops: named.iops.or(image_device.iops),
                snapshot_id: named
                    .snapshot_id
                    .clone()
                    .or_else(|| image_device.snapshot_id.clone()),
                volume_size: named.volume_size.or(image_device.volume_size),
                volume_type: named
                    .volume_type
                    .clone()
                    .or_else(|| image_device.volume_type.clone()),
                kms_key_id: named
                    .kms_key_id
                    .clone()
                    .or_else(|| image_device.kms_key_id.clone()),
                throughput: named.throughput.or(image_device.throughput),
                outpost_arn: named
                    .outpost_arn
                    .clone()
                    .or_else(|| image_device.outpost_arn.clone()),
                availability_zone: named
                    .availability_zone
                    .clone()
                    .or_else(|| image_device.availability_zone.clone()),
                encrypted: Some(
                    // "In no case can you remove encryption from an encrypted volume"
                    // (`EbsBlockDevice$Encrypted`): what the launch says can only ADD
                    // encryption to what the AMI's own device already carries.
                    named.encrypted == Some(true) || image_device.encrypted == Some(true),
                ),
                volume_initialization_rate: named
                    .volume_initialization_rate
                    .or(image_device.volume_initialization_rate),
                availability_zone_id: named
                    .availability_zone_id
                    .clone()
                    .or_else(|| image_device.availability_zone_id.clone()),
                ebs_card_index: named.ebs_card_index.or(image_device.ebs_card_index),
            },
        };
        Some((device_name, self.encrypted_by_default_in(ebs, region)))
    }

    /// `ebs` as the Region's own EBS encryption setting leaves it: encrypted whenever `region`
    /// has encryption on by default, and otherwise exactly as it came.
    ///
    /// "When encryption by default is enabled, […] all new EBS volumes are encrypted"
    /// (`GetEbsEncryptionByDefault`) — a Region-scoped fact about every volume the account
    /// creates there, so it holds whatever launch form created this one. The ONE place that
    /// setting reaches a launch's root device, asked both where a launch is ADMITTED against
    /// the device it would create ([`Self::launch_root_block_device`]) and where the device is
    /// actually stored ([`Self::mint_launched_root_volume`], which the launch paths that
    /// resolve no device of their own reach directly) — so the two cannot disagree, and the
    /// same setting `CreateVolume` reads over its own request cannot mean one thing for a
    /// volume a caller creates and another for one a launch creates.
    pub(super) fn encrypted_by_default_in(
        &self,
        mut ebs: EbsBlockDevice,
        region: &str,
    ) -> EbsBlockDevice {
        if self.ebs_encryption_by_default(region) {
            ebs.encrypted = Some(true);
        }
        ebs
    }

    /// Give the instance `instance_id` the EBS root volume its launch creates, restored from
    /// the root block device its launch resolved for it — see the module docs for why it is
    /// minted here.
    ///
    /// Everything about the volume comes off that block device — the size, the type, the
    /// encryption state and key, the snapshot it is restored from and whether it is deleted
    /// with the instance — which is [`super::InstanceRecord::root_block_device`] for a launch
    /// path that resolved the device itself ([`Self::launch_root_block_device`], where the
    /// AMI's mapping and the launch's own entry for that device are read into one) and the
    /// AMI's own mapping for a launch path that resolved none. Either way the Region's EBS
    /// encryption default is applied here too ([`Self::encrypted_by_default_in`]), so no
    /// launch form can store an unencrypted root volume in a Region whose every new volume is
    /// encrypted. Its zone is the instance's own.
    ///
    /// Nothing is minted when the launch named no AMI at all, when the AMI the account can
    /// see is not EBS-backed or publishes no root device mapping (so there is no volume to
    /// restore), or when the volume store is already full ([`MAX_STORED_VOLUMES`]) — the
    /// instance is still stored in each case, exactly as it was before.
    pub(super) fn mint_launched_root_volume(&mut self, instance_id: &str) {
        let Some(instance) = self.instances.get(instance_id) else {
            return;
        };
        let zone_name = instance.placement_az.clone();
        // The root device this launch resolved, for a launch path that resolves one: the facts
        // the volume is created from, already read against the AMI's own mapping by the one
        // resolution of them.
        let requested_root = instance.root_block_device.clone();
        let Some(image_id) = instance.image_id.clone() else {
            return;
        };
        let Some((device_name, image_device)) = self.launched_root_device(Some(&image_id)) else {
            return;
        };
        if self.volumes.len() >= MAX_STORED_VOLUMES {
            return;
        }

        let volume_id = self.next_id(VOLUME_ID_PREFIX);
        // The Region the volume's ARN names is the one its Availability Zone belongs to —
        // read off the zone name through the ONE reading of that ([`region_of_zone_name`]),
        // so the volume and the instance that holds it can never be placed in two Regions.
        let region = region_of_zone_name(&zone_name).to_string();
        // The device the volume is created from, as this Region's EBS encryption default
        // leaves it: the launch path that resolved one has already been through the same
        // reading, so applying it again changes nothing there.
        let ebs = self.encrypted_by_default_in(requested_root.unwrap_or(image_device), &region);
        let volume_type = ebs
            .volume_type
            .clone()
            .unwrap_or_else(|| DEFAULT_VOLUME_TYPE.to_string());
        let now = SystemTime::now();
        let view = VolumeView {
            volume_id: Some(volume_id.clone()),
            volume_arn: Some(::engine::emulator_arn(
                "ec2",
                &region,
                &format!("volume/{volume_id}"),
            )),
            owner_id: Some(::engine::emulator_account_id().to_string()),
            availability_zone: Some(zone_name.clone()),
            availability_zone_id: zone_id_for_zone_name(&zone_name),
            create_time: Some(system_time_to_date_time(now)),
            size: ebs.volume_size,
            // "The number of I/O operations per second (IOPS)" and "the throughput that the
            // volume supports": the mapping's own, or — for the ONE type AWS documents a
            // default for, "gp3: 3,000 (default)" and the bottom of the gp3 throughput
            // range — that default where the mapping names none. A mapping of a type that
            // REQUIRES provisioned IOPS and names none is left reporting none rather than
            // given the gp3 figure, which is a different type's documented default.
            iops: ebs.iops.or(if volume_type == THROUGHPUT_VOLUME_TYPE {
                Some(DEFAULT_GP3_IOPS)
            } else {
                None
            }),
            throughput: ebs.throughput.or(if volume_type == THROUGHPUT_VOLUME_TYPE {
                Some(DEFAULT_GP3_THROUGHPUT)
            } else {
                None
            }),
            volume_type: Some(volume_type),
            // Multi-Attach is asked for at creation and a launch asks for none.
            multi_attach_enabled: Some(false),
            encrypted: Some(ebs.encrypted.unwrap_or(false)),
            kms_key_id: ebs.kms_key_id.clone(),
            // A launch tags the volumes its own `TagSpecifications` name; nothing tags this
            // record here.
            tags: Some(Vec::new()),
            // "The snapshot from which the volume was created, if applicable"
            // (`Volume$SnapshotId`): the root device snapshot of the AMI the instance was
            // launched from, which is exactly what this volume restores.
            snapshot_id: ebs.snapshot_id.clone(),
            source_volume_id: None,
            // "Indicates whether the volume was created using fast snapshot restore"
            // (`Volume$FastRestored`): nothing enabled fast snapshot restore for the
            // image's snapshot, so it was not.
            fast_restored: Some(false),
            sse_type: None,
            operator: Some(unmanaged_operator()),
            outpost_arn: None,
            volume_initialization_rate: None,
            // Derived from `available_at` on every read.
            state: None,
            // "The device name" the volume is attached at is the image's root device, and
            // the instance holds it from the moment it is launched.
            attachments: Some(vec![AttachVolumeOutput {
                volume_id: Some(volume_id.clone()),
                instance_id: Some(instance_id.to_string()),
                device: Some(device_name),
                state: Some(VOLUME_ATTACHMENT_STATE_ATTACHED.to_string()),
                attach_time: Some(system_time_to_date_time(now)),
                // "Indicates whether the volume is deleted on instance termination"
                // (`EbsBlockDevice$DeleteOnTermination`), whose own documented default for
                // a root volume is `true`.
                delete_on_termination: Some(ebs.delete_on_termination.unwrap_or(true)),
                // An instance's own attachment, not one to an Amazon Web
                // Services-managed resource.
                associated_resource: None,
                instance_owning_service: None,
                ebs_card_index: Some(0),
            }]),
        };
        self.volumes.insert(
            volume_id,
            // The same clock-owed `creating` -> `available` span every volume this store
            // mints has, so a launch's volume and a `CreateVolume` volume are reported the
            // same way.
            VolumeRecord::created(view, Instant::now() + TRANSITION_DELAY),
        );
    }
}

/// The root block device of a launch whose AMI the account cannot see: the device name, size
/// and volume type the crate's default EBS-backed Linux image publishes
/// ([`ImageSpec::default`], which every published catalog row starts from), kept with the
/// image's defaults rather than written out here so the volume a launch from an unseen AMI
/// gets and the volume a launch from a published one gets are the same kind of volume.
///
/// The device restores NO snapshot ("the snapshot from which the volume was created, if
/// applicable", `Volume$SnapshotId`): no image was read, so there is no root device snapshot
/// to name. It is unencrypted and deleted on termination, both the documented defaults for a
/// root device the request said nothing about.
fn default_root_device() -> (String, EbsBlockDevice) {
    let defaults = ImageSpec::default();
    (
        defaults.root_device_name,
        EbsBlockDevice {
            delete_on_termination: Some(true),
            snapshot_id: None,
            volume_size: Some(defaults.root_volume_size),
            volume_type: Some(defaults.root_volume_type),
            encrypted: Some(false),
            iops: None,
            kms_key_id: None,
            throughput: None,
            outpost_arn: None,
            availability_zone: None,
            volume_initialization_rate: None,
            availability_zone_id: None,
            ebs_card_index: None,
        },
    )
}

/// The bytes one launch's resolved root block device holds: the strings on it.
///
/// Every other member of an [`EbsBlockDevice`] is a number or a flag, so it is fixed-size.
/// Some of those strings come from the request (the volume type or the key a launch named)
/// and some from the AMI's own mapping, and the record keeps whichever it resolved, so the
/// charge is over the device as stored rather than over the request alone. The ONE definition
/// of it: `RunInstances` asks it before the write, for every instance the launch would mint,
/// and [`super::instance_payload_bytes`] charges exactly it — so the bytes a launch retains
/// on its instances are the bytes its ceiling saw.
pub fn root_block_device_bytes(root_block_device: Option<&EbsBlockDevice>) -> usize {
    let Some(ebs) = root_block_device else {
        return 0;
    };
    ebs.volume_type
        .as_ref()
        .map_or(0, String::len)
        .saturating_add(ebs.kms_key_id.as_ref().map_or(0, String::len))
        .saturating_add(ebs.snapshot_id.as_ref().map_or(0, String::len))
        .saturating_add(ebs.outpost_arn.as_ref().map_or(0, String::len))
        .saturating_add(ebs.availability_zone.as_ref().map_or(0, String::len))
        .saturating_add(ebs.availability_zone_id.as_ref().map_or(0, String::len))
}
