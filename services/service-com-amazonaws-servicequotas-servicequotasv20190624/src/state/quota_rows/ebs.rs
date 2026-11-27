// SPDX-License-Identifier: CC0-1.0

//! Amazon Elastic Block Store's published default-quota catalog.

use super::*;

/// The `AWS/Usage` display name Amazon EBS publishes its API call counts under, which
/// is the `Service` dimension of every metered EBS quota.
const EBS_USAGE_SERVICE: &str = "EBS";

/// One of the EBS direct-API request planes: so many requests of one operation PER SECOND
/// against one scope — a single snapshot, or the account.
///
/// SOURCE: the recorded default-quota listing of `ebs`, whose block-write and
/// block-listing rows are published as `<Api> requests per <scope>` with the sentence
/// `The maximum number of <Api> requests allowed per <scope>.`, over a one-second window
/// and with no usage metric, and are NOT adjustable. One constructor fills the class so two
/// of its rows cannot drift apart.
fn direct_api_rate(code: &str, api: &str, scope: &str, per_second: f64) -> QuotaRow {
    quota(format!("{api} requests per {scope}"), per_second)
        .code(code)
        .fixed()
        .period(1, "SECOND")
        .describe(&format!(
            "The maximum number of {api} requests allowed per {scope}."
        ))
}

/// The sentence Amazon EBS publishes for one of its AGGREGATED planes: how much of a
/// resource all volumes of one type in the Region may hold together.
///
/// SOURCE: the recorded default-quota listing of `ebs`, whose gp2 storage-modification row
/// reads `The maximum aggregated amount of storage, in TiB, that can be requested in volume
/// modifications across General Purpose SSD (gp2) volumes in this Region.` and whose io1
/// IOPS-modification row reads `The maximum aggregated number of IOPS that can be requested
/// in volume modifications across Provisioned IOPS SSD (io1) volumes in this Region.` The
/// class is that one sentence around how the plane's quantity is counted — an AMOUNT of
/// storage in the TiB it is sold in, a NUMBER of IOPS — what may be DONE with it on that
/// plane, and the volume type it is counted over, so it is built once here.
///
/// The two planes of a volume type differ in that verb and in nothing else: the capacity
/// plane itself is what may be PROVISIONED across the type's volumes, while the
/// modification plane beside it is what may be REQUESTED in volume modifications across
/// them ([`provisioned_description`] and [`modification_description`] are the two call
/// forms).
fn aggregated_description(aggregated: &str, may_be: &str, volume_type: &str) -> String {
    format!(
        "The maximum aggregated {aggregated} that can be {may_be} \
         across {volume_type} volumes in this Region."
    )
}

/// The sentence of a CAPACITY plane of one volume type: what may be provisioned across the
/// type's volumes in the Region.
fn provisioned_description(aggregated: &str, volume_type: &str) -> String {
    aggregated_description(aggregated, "provisioned", volume_type)
}

/// The sentence of a MODIFICATION plane of one volume type: what may be requested in volume
/// modifications across the type's volumes in the Region.
fn modification_description(aggregated: &str, volume_type: &str) -> String {
    aggregated_description(aggregated, "requested in volume modifications", volume_type)
}

/// How this service counts a STORAGE plane: an amount, in the TiB the capacity is sold in.
const STORAGE_COUNTED: &str = "amount of storage, in TiB,";

/// How this service counts an IOPS plane: a number of operations per second.
const IOPS_COUNTED: &str = "number of IOPS";

/// Amazon Elastic Block Store's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `ebs` service code (the "Amazon
/// EBS quotas" page of the Amazon EBS User Guide) for the storage, IOPS and snapshot
/// planes it publishes, and the recorded default-quota listing of that code for which
/// `QuotaCode` names which quota and for the per-API request rates it publishes beside
/// them. EBS publishes a storage plane PER VOLUME TYPE — a volume type is what the
/// capacity is bought under — so a storage row is named for the type, and names the TiB
/// the capacity is counted in IN THAT NAME rather than in a `Unit` of its own, which is
/// what the modification plane of a type does as well.
/// A request-rate row carries the one-second window and the `AWS/Usage` `CallCount` of
/// the operation it throttles ([`request_rate`]); the EBS direct APIs are published as
/// their own class ([`direct_api_rate`]), over the same window but without a usage metric
/// and against the scope the rate is counted on.
///
/// Adjustability is published PER CODE and is not a property of the class: the recorded
/// listing reports this service's rate planes as NOT adjustable except the ones whose codes
/// it reports the other way round, so each row carries the adjustability the listing reports
/// for its own code rather than the one its constructor defaults to.
/// A quota of this service the data available here names no code for is NOT published
/// under a neighbouring code or a stand-in — a code identifies ONE quota (see
/// [`super::descriptive_quota_code`]) — so the snapshot plane of the Region and the
/// per-API rates whose codes are not among the recorded ones are left to the
/// default-quota-set gap, which is where the `CopySnapshot`, `ModifySnapshotAttribute` and
/// `UnlockSnapshot` rates go: the recorded listing states their codes for the io2
/// provisioned-storage and storage-modification planes, the io1 IOPS-modification plane and
/// the time-based snapshot-copy throughput of a destination Region, so those codes carry
/// those planes here and the Magnetic storage plane takes the code the io2 binding vacated.
/// The listing also states `L-309BACF6` for the SNAPSHOT plane of the Region and
/// `L-59C8FC87` for the storage-modification plane of the General Purpose SSD (gp3) type,
/// so those are the quotas those codes publish here; the `DeleteSnapshot` and
/// `DescribeVolumeStatus` rates this catalog had served under them name no other code that
/// listing states, so they go to the same gap.
/// `L-C125AE42` and `L-DB2FBAA1` are codes the recorded listing states in this catalog, so
/// each answers a row — the attachment plane of a Multi-Attach enabled volume and the
/// fast-snapshot-restore plane of the Region, the two planes this service publishes that no
/// other code here names.
pub(in crate::state) fn ebs_quotas() -> Vec<QuotaRow> {
    let rate =
        |code: &str, api: &str, per_second: f64| request_rate(code, EBS_USAGE_SERVICE, api, per_second);
    // A storage plane of one volume type: the capacity, in TiB, of all volumes of that
    // type in the Region. The TiB is named in the quota's own name, so the row carries no
    // `Unit` of its own.
    let storage = |code: &str, volume_type: &str, tebibytes: f64| {
        quota(format!("Storage for {volume_type} volumes, in TiB"), tebibytes)
            .code(code)
            .describe(&provisioned_description(STORAGE_COUNTED, volume_type))
    };
    // The storage all volumes of one type may be MODIFIED to in the Region, which the
    // service publishes beside the storage plane itself. The TiB it is counted in is named
    // in the quota's own name, so the row carries no `Unit` of its own either.
    let storage_modifications = |code: &str, volume_type: &str, tebibytes: f64| {
        quota(
            format!("Storage modifications for {volume_type} volumes, in TiB"),
            tebibytes,
        )
        .code(code)
        .describe(&modification_description(STORAGE_COUNTED, volume_type))
    };
    // An IOPS plane of one volume type: the provisioned IOPS of all volumes of that
    // type in the Region.
    let iops = |code: &str, volume_type: &str, operations_per_second: f64| {
        quota(
            format!("IOPS for {volume_type} volumes"),
            operations_per_second,
        )
        .code(code)
        .describe(&provisioned_description(IOPS_COUNTED, volume_type))
    };
    // The IOPS all volumes of one type may be MODIFIED to in the Region, published beside
    // the IOPS plane itself the way the storage-modification plane is published beside the
    // storage plane.
    let iops_modifications = |code: &str, volume_type: &str, operations_per_second: f64| {
        quota(
            format!("IOPS modifications for {volume_type} volumes"),
            operations_per_second,
        )
        .code(code)
        .describe(&modification_description(IOPS_COUNTED, volume_type))
    };
    vec![
        // The one volume-attachment rate AWS publishes as not adjustable.
        rate("L-028ACFB9", "AttachVolume", 100.0).fixed(),
        rate("L-07399329", "CompleteSnapshot", 100.0),
        // The provisioned-storage plane of the Provisioned IOPS SSD (io2) type, published
        // under the code the recorded listing states for it; the `CopySnapshot` rate this
        // catalog had served here names no other code that listing states, so it is left to
        // the default-quota-set gap.
        storage("L-09BD8365", "Provisioned IOPS SSD (io2)", 20.0),
        // The two EBS direct-API request planes the service publishes as FIXED: the
        // block-write rate of one snapshot and the block-listing rate of the account.
        // The block-listing plane keeps the value the recorded listing reports for its
        // code, which is the one this catalog already served there.
        direct_api_rate("L-1774F84A", "PutSnapshotBlock", "snapshot", 1000.0),
        direct_api_rate("L-18E976AB", "ListSnapshotBlocks", "account", 50.0),
        rate("L-1D4D9345", "CreateVolume", 100.0).fixed(),
        storage("L-17AF77E8", "Throughput Optimized HDD (st1)", 50.0),
        // The snapshot plane of the Region, published under the code the recorded listing
        // states for it: the snapshots the account may hold there at once. The
        // `DeleteSnapshot` rate this catalog had served here names no other code that
        // listing states, so it is left to the default-quota-set gap.
        quota("Snapshots per Region", 100_000.0)
            .code("L-309BACF6")
            .describe("The maximum number of snapshots per Region."),
        rate("L-35B31D98", "DeleteVolume", 100.0),
        // The volume-copy plane of the account: how many copies may run AT ONCE, which
        // is a concurrency and not one of the per-API rates beside it. The service words
        // this one as a maximum of its own rather than in the counted-plane voice.
        quota("Max concurrent copy volume operations per account", 5.0)
            .code("L-39BD5252")
            .describe(
                "Maximum number of concurrent copy volume operations allowed per account in \
                 this region.",
            ),
        rate("L-3A0E616D", "DescribeSnapshots", 100.0),
        // The storage-modification plane of the General Purpose SSD (gp3) type, published
        // under the code the recorded listing states for it; the `DescribeVolumeStatus`
        // rate this catalog had served here names no other code that listing states, so it
        // is left to the default-quota-set gap.
        storage_modifications("L-59C8FC87", "General Purpose SSD (gp3)", 500.0),
        rate("L-5F80CA91", "DescribeVolumes", 100.0),
        rate("L-631ECBD3", "DescribeVolumesModifications", 100.0),
        rate("L-651D1834", "DetachVolume", 100.0),
        rate("L-750405C3", "DisableFastSnapshotRestores", 10.0).fixed(),
        storage("L-7A658B76", "General Purpose SSD (gp3)", 50.0),
        rate("L-82ACEF56", "EnableFastSnapshotRestores", 10.0),
        // The EBS direct APIs: the block-level read and write rates of a snapshot,
        // which AWS publishes far above its control-plane rates.
        rate("L-835364B2", "GetSnapshotBlock", 1000.0).fixed(),
        iops("L-8656991D", "Provisioned IOPS SSD (io2)", 100_000.0).fixed(),
        // The storage-modification plane of the Throughput Optimized HDD type, which the
        // recorded listing publishes under this code; the `ListChangedBlocks` rate this
        // catalog had served here names no other code that listing states, so it is left to
        // the default-quota-set gap.
        storage_modifications("L-87C9DEA6", "Throughput Optimized HDD (st1)", 500.0),
        // The storage plane of the Magnetic type, the plane the io2 binding above
        // displaced: it takes the code that binding vacated.
        storage("L-8D977E7E", "Magnetic (standard)", 20.0),
        rate("L-915A3DBB", "ListSnapshotBlocks", 100.0).fixed(),
        rate("L-94D7AB7D", "LockSnapshot", 10.0).fixed(),
        // The IOPS-modification plane of the Provisioned IOPS SSD (io1) type, published
        // under the code the recorded listing states for it; the
        // `ModifySnapshotAttribute` rate this catalog had served here names no other code
        // that listing states, so it is left to the default-quota-set gap.
        iops_modifications("L-98A0B26D", "Provisioned IOPS SSD (io1)", 500_000.0),
        // The storage-modification plane of the Provisioned IOPS SSD (io2) type, published
        // under the code the recorded listing states for it, with the limit that code
        // carries.
        storage_modifications("L-9A0E0F82", "Provisioned IOPS SSD (io2)", 20.0),
        storage("L-9CF3C2EB", "Cold HDD (sc1)", 50.0),
        rate("L-9F6E7C4E", "ModifySnapshotTier", 10.0).fixed(),
        rate("L-A37D9CF3", "ModifyVolume", 100.0),
        rate("L-AFAE1BE8", "PutSnapshotBlock", 1000.0),
        rate("L-AFEBDF7A", "ResetSnapshotAttribute", 100.0).fixed(),
        iops("L-B3A130E6", "Provisioned IOPS SSD (io1)", 300_000.0),
        rate("L-B9F7C487", "RestoreSnapshotTier", 10.0),
        // The attachment plane of ONE Multi-Attach enabled volume: the instances such a
        // volume may be attached to at once, which AWS publishes as fixed. The recorded
        // listing states this code in this service's catalog, so it answers a row rather
        // than nothing, under the plane the "Amazon EBS quotas" page of the Amazon EBS User
        // Guide publishes for it.
        quota("Attachments per Multi-Attach enabled volume", 16.0)
            .code("L-C125AE42")
            .fixed()
            .describe(
                "The maximum number of instances that one Multi-Attach enabled volume can be \
                 attached to at the same time.",
            ),
        quota("In-progress snapshot archives per account", 5.0)
            .code("L-D0291BE3")
            .fixed()
            .describe(
                "The maximum number of snapshot archives that can be in progress at once in \
                 this account in the current Region.",
            ),
        storage("L-D18FCD1D", "General Purpose SSD (gp2)", 50.0),
        rate("L-D8F37C68", "StartSnapshot", 10.0).fixed(),
        quota("Concurrent snapshot copies per destination Region", 20.0)
            .code("L-DB70D580")
            .fixed()
            .describe(
                "The maximum number of snapshot copies that can be in progress at once to a \
                 single destination Region.",
            ),
        // The fast-snapshot-restore plane of the Region: the snapshots fast snapshot
        // restore may be enabled for at once. The recorded listing states this code in this
        // service's catalog, so it answers a row rather than nothing, under the plane the
        // Amazon EBS User Guide publishes for it.
        quota("Snapshots with fast snapshot restore enabled per Region", 5.0)
            .code("L-DB2FBAA1")
            .describe(
                "The maximum number of snapshots that can have fast snapshot restore enabled \
                 at once in this account in the current Region.",
            ),
        // The snapshot-copy THROUGHPUT plane of one destination Region, published under the
        // code the recorded listing states for it; the `UnlockSnapshot` rate this catalog
        // had served here names no other code that listing states, so it is left to the
        // default-quota-set gap.
        quota("Time-based snapshot copy throughput per destination Region", 2000.0)
            .code("L-E137849C")
            .describe(
                "The maximum account-level throughput, in MiB/sec, for time-based snapshot \
                 copy operations per destination Region.",
            ),
        quota("Archived snapshots per volume", 25.0)
            .code("L-E20676C1")
            .describe(
                "The maximum number of snapshots of one volume that can be in the archive \
                 storage tier.",
            ),
        // The storage a volume type's volumes may be MODIFIED to, published beside the
        // storage plane of the same type.
        storage_modifications("L-F06E64A8", "General Purpose SSD (gp2)", 500.0),
        storage("L-FD252861", "Provisioned IOPS SSD (io1)", 50.0),
    ]
}
