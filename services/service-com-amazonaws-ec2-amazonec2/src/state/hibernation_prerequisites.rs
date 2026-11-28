// SPDX-License-Identifier: CC0-1.0

//! What a launch has to be for hibernation to be enabled on it.
//!
//! "This parameter is valid only if the instance meets the hibernation prerequisites"
//! (`RunInstancesRequest$HibernationOptions`): hibernation writes the instance's memory to
//! its own root volume and reads it back on the next start, so three facts of the launch
//! decide whether it can be performed at all — the instance type has to be one hibernation is
//! offered on, the root device has to be an ENCRYPTED Amazon EBS volume, and that volume has
//! to be large enough to hold the memory the instance runs with. A launch that fails any of
//! them is refused `UnsupportedHibernationConfiguration` before anything is created, rather
//! than creating an instance that could never hibernate.
//!
//! The ONE place those prerequisites are read, so every launch path that admits
//! `HibernationOptions` reaches the same verdict in the same words, and the type's own facts
//! are the catalog's ([`super::instance_type_record`]) — so a launch and
//! `DescribeInstanceTypes` cannot disagree about whether a type hibernates or about how much
//! memory it has.

use crate::ops::types::EbsBlockDevice;
use crate::scaffolded::Scaffolded;

use super::instance_type_record;

/// The MiB in one GiB: an instance type's memory is published in MiB
/// ([`super::InstanceTypeRecord::memory_mib`]) and a volume's size in GiB
/// (`EbsBlockDevice$VolumeSize`), so one of the two has to be read in the other's unit.
const MIB_PER_GIB: u64 = 1_024;

/// The root volume size, in GiB, the memory of an instance of `memory_mib` MiB occupies: the
/// memory it has to store, rounded UP to the whole GiB a volume is sized in.
///
/// "The root volume must be large enough to hold the instance's RAM contents" AND "accommodate
/// your expected usage" (the EC2 User Guide's hibernation prerequisites): the memory is written
/// to the SAME volume the instance boots from, so a volume of exactly this size leaves nothing
/// for what is already on it — which is why the admissible size is strictly ABOVE it
/// ([`hibernation_prerequisite_refusal`]). A volume is sized in whole GiB, so a memory size
/// that is not a whole number of GiB occupies the next whole one.
#[must_use]
pub fn hibernation_root_volume_gib(memory_mib: i64) -> i64 {
    let memory_mib = u64::try_from(memory_mib).unwrap_or(0);
    i64::try_from(memory_mib.div_ceil(MIB_PER_GIB)).unwrap_or(i64::MAX)
}

/// Why hibernation cannot be enabled on a launch of `instance_type` whose root device would
/// be `root_device` — or `None` when the launch meets every prerequisite, which is the launch
/// Amazon EC2 performs.
///
/// Read in the order the prerequisites bite: the instance TYPE first (a type hibernation is
/// not offered on can meet no other prerequisite), then the root device's encryption, then
/// its size. A launch that creates no root volume at all — an instance store backed AMI —
/// cannot store any memory, so it fails the encryption prerequisite exactly as an
/// unencrypted volume does.
///
/// An instance type this emulator publishes no facts for contributes no verdict of its own:
/// the refusals a launch is given are grounded in what the catalog HOLDS.
#[must_use]
pub fn hibernation_prerequisite_refusal<E>(
    instance_type: &str,
    root_device: Option<&EbsBlockDevice>,
) -> Option<Scaffolded<E>> {
    let record = instance_type_record(instance_type);
    // "Supported instance families" (the hibernation prerequisites): hibernation is offered
    // per instance type, which the catalog publishes as `InstanceTypeInfo$HibernationSupported`
    // — so a type it is not offered on is refused here whatever the rest of the launch says.
    if record
        .as_ref()
        .is_some_and(|record| !record.hibernation_supported)
    {
        return Some(Scaffolded::hibernation_not_supported_by_instance_type(
            instance_type,
        ));
    }
    // "The root volume must be an encrypted Amazon EBS volume": the memory is written to that
    // volume, so a launch whose root device would not be encrypted — and one that creates no
    // Amazon EBS root device at all — cannot hibernate.
    if root_device.is_none_or(|ebs| ebs.encrypted != Some(true)) {
        return Some(Scaffolded::hibernation_requires_encrypted_root_volume());
    }
    // And it has to be large enough to hold that memory BESIDE what the instance boots from:
    // the memory is written to the root volume, so a volume no larger than the memory could
    // not hold both.
    let memory_mib = record.as_ref().map(|record| record.memory_mib)?;
    let memory_gib = hibernation_root_volume_gib(memory_mib);
    let size_gib = i64::from(root_device.and_then(|ebs| ebs.volume_size).unwrap_or_default());
    if size_gib <= memory_gib {
        return Some(Scaffolded::hibernation_root_volume_too_small());
    }
    None
}
