// SPDX-License-Identifier: CC0-1.0

//! AWS Storage Gateway's published default-quota catalog.

use super::*;

/// AWS Storage Gateway's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `storagegateway` service code
/// (what `aws service-quotas list-aws-default-service-quotas --service-code
/// storagegateway` reports, which is the "AWS Storage Gateway quotas" pages of its user
/// guide), and the `QuotaCode`s that catalog is keyed on.
///
/// Storage Gateway publishes its quotas PER GATEWAY TYPE — the volumes a volume gateway
/// serves, the shares a file gateway serves and the virtual tapes a tape gateway serves —
/// which is why most rows are named for the plane of one gateway rather than for the
/// account. A row that measures a capacity carries the unit AWS publishes it in rather
/// than being a bare count, except where the service names the unit IN the quota's own
/// name and publishes `Unit` `None` for it.
///
/// THE CAPACITY CLASS. The upload-buffer and cache bounds of a gateway type are published
/// as one class: the name is `<Gateway type> <Plane> <Minimum|Maximum> in <unit>`, the unit
/// is named IN that name so `Unit` is `None`, the row is NOT adjustable, and the sentence
/// is `<Minimum|Maximum> <plane> size for <Gateway Type>`. The recorded listing states four
/// rows of the class — the cached and tape gateways' upload-buffer minimums, the tape
/// gateway's cache minimum and the file gateway's cache maximum — which is the source for
/// the three further rows of it below.
///
/// The recorded listing states the quota published under `L-311F8856` (the largest virtual
/// tape, 15, with the unit in the name), under `L-6D83F84B` (the cached volume gateway's
/// upload-buffer minimum, 150), under `L-87640A7E` (the tape gateway's upload-buffer
/// minimum, 150), under `L-F3EDDF8C` (the tape gateway's cache minimum, 150), under
/// `L-F5915598` (the file gateway's cache MAXIMUM, 64, with the unit in the name), under
/// `L-4951D254` (the total size of the tapes of one virtual tape library, 1, with the unit
/// in the name), under `L-C86341FE` (the longest path, 1024 Bytes), under `L-E0AEEBAC` (the
/// STORED volume gateway's upload-buffer minimum, 150), under `L-81A6E497` (the size of one
/// stored volume, 16, with the unit in the name), under `L-14F83003` (the cached volume
/// gateway's cache MAXIMUM, 64, with the unit in the name and published as NOT adjustable),
/// under `L-CEFD39A0` (the file shares of one Amazon S3 bucket, 1) and under `L-59D49F15`
/// (the TAPE gateway's cache maximum), beside the sentence the service words each of those
/// with. The virtual-tape count, the cache-disk count, the tag count, the snapshot-schedule
/// volume count, the cached volume storage of a gateway, the automatic tape creation policy
/// count, the NFS client count, the bandwidth-schedule interval count and the largest file
/// written through a share this catalog used to key on those codes are NOT published here:
/// no source available here states AWS's code for them, so they are left to the
/// default-quota-set gap rather than served under another quota's code. A row whose
/// published `Description` no source available here states carries none, rather than a
/// sentence this emulator words itself.
///
/// THE CODE NO SOURCE BINDS. `L-6F75AC83` is a code this catalog publishes whose own quota
/// no source available here names, so it carries the plane one of the bindings above
/// displaced — the gateways of a Region — as the module header says such a row does. That
/// is not a claim that AWS publishes that plane under that code.
pub(in crate::state) fn storagegateway_quotas() -> Vec<QuotaRow> {
    vec![
        // The cache of ONE CACHED VOLUME gateway, whose unit the service names IN the
        // quota's own name, published as NOT adjustable.
        quota("Cached volume gateway Cache Maximum in TiB", 64.0)
            .code("L-14F83003")
            .fixed()
            .describe("Maximum cache size for Cached Volume Gateway"),
        quota("Cached volumes per gateway", 32.0)
            .code("L-2232E8E3")
            .fixed()
            .describe("The maximum number of cached volumes per volume gateway."),
        quota("Stored volumes per gateway", 32.0)
            .code("L-2E88EE16")
            .fixed()
            .describe("The maximum number of stored volumes per volume gateway."),
        quota("Maximum size of a cached volume", 32.0)
            .code("L-2EC26EAB")
            .unit("TiB")
            .fixed()
            .describe("The maximum size, in TiB, of one cached volume."),
        quota("Max size of a virtual tape in TiB", 15.0)
            .code("L-311F8856")
            .fixed()
            .describe("Maximum size of a virtual tape"),
        // The tapes of ONE virtual tape library, whose unit the service names IN the
        // quota's own name and publishes `Unit` `None` for.
        quota("Total size of tapes in a virtual tape library in PiB", 1.0)
            .code("L-4951D254")
            .fixed()
            .describe("Total size of all tapes in a virtual tape library (VTL)"),
        quota("Total stored volume storage per gateway", 512.0)
            .code("L-5308FBCA")
            .unit("TiB")
            .fixed()
            .describe(
                "The maximum total size, in TiB, of the stored volumes of one volume \
                 gateway.",
            ),
        quota("File shares per file gateway", 50.0)
            .code("L-58DCC961")
            .fixed()
            .describe("The maximum number of file shares per file gateway."),
        // The cache of ONE TAPE gateway, whose unit the service names IN the quota's own
        // name, published like the file gateway's cache maximum.
        quota("Tape gateway Cache Maximum in TiB", 64.0)
            .code("L-59D49F15")
            .fixed()
            .describe("Maximum cache size for Tape Gateway"),
        quota("Cached volume gateway Upload Buffer Minimum in GiB", 150.0)
            .code("L-6D83F84B")
            .fixed()
            .describe("Minimum upload buffer size for Cached Volume Gateway"),
        // A code whose own published quota no source available here states, carrying the
        // plane the `L-14F83003` binding displaced (see the header).
        quota("Gateways per Region", 100.0)
            .code("L-6F75AC83")
            .describe(
                "The maximum number of gateways in this account in the current Region.",
            ),
        quota("Minimum size of a virtual tape", 100.0)
            .code("L-761F1680")
            .unit("Gigabytes")
            .fixed()
            .describe("The minimum size, in GB, of one virtual tape."),
        quota("Total virtual tape storage per gateway", 1024.0)
            .code("L-7B01AFFB")
            .unit("TiB")
            .fixed()
            .describe(
                "The maximum total size, in TiB, of the virtual tapes of one tape gateway.",
            ),
        quota("Virtual tapes in the tape archive", 10_000.0)
            .code("L-803470D8")
            .describe("The maximum number of virtual tapes in the tape archive."),
        // The size of ONE STORED volume, whose unit the service names IN the quota's own
        // name and publishes `Unit` `None` for.
        quota("Stored volume size in TiB", 16.0)
            .code("L-81A6E497")
            .fixed()
            .describe("Maximum size of a stored volume"),
        quota("Tape gateway Upload Buffer Minimum in GiB", 150.0)
            .code("L-87640A7E")
            .fixed()
            .describe("Minimum upload buffer size for Tape Gateway"),
        quota("Upload buffer disks per gateway", 20.0)
            .code("L-96D17DE9")
            .fixed()
            .describe(
                "The maximum number of local disks used as upload buffer by one gateway.",
            ),
        quota("Cache storage per gateway", 64.0)
            .code("L-98F44425")
            .unit("TiB")
            .fixed()
            .describe("The maximum cache storage, in TiB, of one gateway."),
        quota("Upload buffer storage per gateway", 2.0)
            .code("L-99E991AF")
            .unit("TiB")
            .fixed()
            .describe("The maximum upload buffer storage, in TiB, of one gateway."),
        quota("Custom tape pools per account", 100.0)
            .code("L-BD85E1A6")
            .describe(
                "The maximum number of custom tape pools in this account in the current \
                 Region.",
            ),
        // The longest path a gateway serves, which the service publishes in Bytes.
        quota("Path length", 1024.0)
            .code("L-C86341FE")
            .unit("Bytes")
            .fixed()
            .describe("Maximum path length"),
        quota("File shares per S3 bucket", 1.0)
            .code("L-CEFD39A0")
            .fixed(),
        // The upload buffer of ONE STORED VOLUME gateway, whose unit the service names IN
        // the quota's own name.
        quota("Stored volume gateway Upload Buffer Minimum in GiB", 150.0)
            .code("L-E0AEEBAC")
            .fixed()
            .describe("Minimum upload buffer size for Stored Volume Gateway"),
        quota("Tape gateway Cache Minimum in GiB", 150.0)
            .code("L-F3EDDF8C")
            .fixed()
            .describe("Minimum cache size for Tape Gateway"),
        // The cache of ONE file gateway, whose unit the service names IN the quota's own
        // name and publishes `Unit` `None` for.
        quota("File gateway Cache Maximum in TiB", 64.0)
            .code("L-F5915598")
            .fixed()
            .describe("Maximum cache size for File Gateway"),
        quota("Audit logs per file share", 1.0)
            .code("L-FF1BE522")
            .fixed()
            .describe("The maximum number of audit log destinations per file share."),
    ]
}
