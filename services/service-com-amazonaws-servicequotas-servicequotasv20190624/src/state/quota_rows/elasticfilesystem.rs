// SPDX-License-Identifier: CC0-1.0

//! Amazon Elastic File System's published default-quota catalog.

use super::*;

/// The `AWS/Usage` display name Amazon EFS publishes its API call counts under.
const EFS_USAGE_SERVICE: &str = "EFS";

/// Amazon Elastic File System's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `elasticfilesystem` service
/// code (the "Amazon EFS quotas that you can increase" and "Quotas for Amazon EFS file
/// systems" tables of the Amazon EFS User Guide), and the recorded default-quota
/// listing of that code for which `QuotaCode` names which quota, for each code's
/// adjustability and for the per-API request rates it publishes beside the resource
/// planes. A throughput row carries the unit AWS publishes it in rather than being a
/// count, and a request-rate row carries the one-second window and the `AWS/Usage`
/// `CallCount` of the operation it throttles ([`request_rate`]). This service closes a
/// PLANE's sentence without a full stop, which is the form every plane here is worded
/// through.
///
/// Beside those, the service publishes planes of the FILE SYSTEM ITSELF and of one NFS
/// CLIENT that are neither: the read IOPS one file system with Elastic throughput may drive
/// (`L-B34D07B4`, counted per second), the users one NFS client may have files open for at
/// the same time (`L-FA0AAA42`, a count with no window, published as fixed), the throughput
/// one NFS client may drive (`L-06519EB8`, published in megabytes over a one-second window)
/// and the length of a symbolic link (`L-B8B14E21`, published in bytes). The recorded
/// listing states those codes, so they carry those quotas here rather than the mount-target,
/// update and access-point-description rates this catalog had published under them and
/// rather than the account-wide provisioned-throughput plane; those rates and that plane
/// name no other code the listing states and are left to the default-quota-set gap, the way
/// the tag plane of a file system is — this catalog can cite no code for it, and a code
/// identifies ONE quota (see [`super::descriptive_quota_code`]).
///
/// That listing also states `L-848C634D` for the FILE-SYSTEM plane of the account, so that
/// is the code it is published under here, with that code's published adjustability; the
/// `DeleteAccessPoint` rate this catalog had served there takes the code that binding
/// vacated (`L-03A6A61D`), with the adjustability of this service's rate class.
pub(in crate::state) fn elasticfilesystem_quotas() -> Vec<QuotaRow> {
    // This service publishes its per-API rates as NOT adjustable — a per-second API rate is
    // a property of the service rather than a ceiling an account can have raised — so the
    // class carries that adjustability and the one row the recorded listing reports the
    // other way round states it for itself.
    let rate = |code: &str, api: &str, per_second: f64| {
        request_rate(code, EFS_USAGE_SERVICE, api, per_second).fixed()
    };
    // The sentence this service publishes for a PLANE: its own phrase, closed WITHOUT a
    // full stop.
    //
    // SOURCE: the recorded default-quota listing of `elasticfilesystem`, whose access-point
    // plane reads `The maximum number of access points allowed per file system` and whose
    // active-user plane closes the same way — no trailing stop on either. It is the form of
    // this service's planes, so every one of them is worded through this one hook.
    let plane_sentence = |sentence: &str| sentence.trim_end_matches('.').to_string();
    vec![
        // The access-point deletion rate, on the code the file-system plane below vacated
        // and with the adjustability this service's rate class is published with.
        rate("L-03A6A61D", "DeleteAccessPoint", 5.0),
        // The throughput plane of ONE NFS client: how much a single client may drive,
        // published in megabytes over a one-second window rather than as a rate per second
        // in the unit itself, and as fixed. The recorded listing states this code for that
        // plane, so it carries it here; the account-wide provisioned-throughput plane this
        // catalog had published under it names no other code that listing states and is
        // left to the default-quota-set gap.
        quota("Throughput per NFS client", 1500.0)
            .code("L-06519EB8")
            .unit("Megabytes")
            .fixed()
            .period(1, "SECOND")
            .describe(&plane_sentence(
                "The maximum throughput allowed per NFS client.",
            )),
        quota("Mount targets per file system per Availability Zone", 1.0)
            .code("L-1E30FF38")
            .fixed()
            .describe(&plane_sentence(
                "The maximum number of mount targets that one file system can have in one \
                 Availability Zone.",
            )),
        quota("VPCs per file system", 1.0)
            .code("L-207AE8E0")
            .fixed()
            .describe(&plane_sentence(
                "The maximum number of VPCs that one file system can be mounted from.",
            )),
        quota("Security groups per mount target", 5.0)
            .code("L-29770CF9")
            .fixed()
            .describe(&plane_sentence(
                "The maximum number of security groups per mount target.",
            )),
        // The file-system operation rate, which the service publishes as a count with no
        // window of its own rather than as one of the per-API rates below, and as FIXED:
        // it is a property of the performance mode the file system runs in. Its own
        // sentence names that mode and what each kind of operation costs against the
        // limit, because a read and a write do not cost the same.
        quota("Rate of file system operations", 250_000.0)
            .code("L-336A920F")
            .fixed()
            .describe(&plane_sentence(
                "The maximum number of file system operations per second for a file system \
                 using General Purpose performance mode. Each read operation consumes one \
                 file operation, and each write operation consumes five file operations.",
            )),
        quota("Replication configurations per file system", 1.0)
            .code("L-3D348029")
            .fixed()
            .describe(&plane_sentence(
                "The maximum number of replication configurations per file system.",
            )),
        quota("Provisioned throughput per file system", 1024.0)
            .code("L-48B15094")
            .unit("Megabytes/Second")
            .fixed()
            .describe(&plane_sentence(
                "The maximum throughput, in MiB/s, that can be provisioned for one file \
                 system.",
            )),
        quota("Access points per file system", 10_000.0)
            .code("L-4CCB99B3")
            .fixed()
            .describe(&plane_sentence(
                "The maximum number of access points allowed per file system",
            )),
        rate("L-509A1582", "CreateAccessPoint", 5.0),
        rate("L-6D380DD0", "CreateFileSystem", 5.0),
        rate("L-7391004C", "CreateMountTarget", 5.0),
        rate("L-7FEABCD7", "CreateReplicationConfiguration", 5.0),
        // The file-system plane of the account, published under the code the recorded
        // listing states for it and with that code's published adjustability: the file
        // systems one account may have in a Region. The `DeleteAccessPoint` rate this
        // catalog had served here takes the code this binding vacated.
        quota("File systems per account", 1000.0)
            .code("L-848C634D")
            .describe(&plane_sentence(
                "The maximum number of file systems that a customer account can have in an \
                 AWS Region",
            )),
        rate("L-A139E5A7", "DeleteFileSystem", 5.0),
        // The read-IOPS plane of ONE file system running Elastic throughput: counted per
        // second, like the service's rates, but a plane of the file system rather than a
        // throttle on an API, so it carries no call count.
        quota(
            "Maximum read IOPS per file system with Elastic throughput",
            250_000.0,
        )
        .code("L-B34D07B4")
        .period(1, "SECOND")
        .describe(&plane_sentence(
            "The maximum read IOPS for one file system with Elastic throughput mode enabled.",
        )),
        // The symbolic-link plane of a file system: how long a link may be, published in
        // bytes and as fixed. The recorded listing states this code for that plane, so it
        // carries it here; the `DescribeAccessPoints` rate this catalog had served under it
        // names no other code that listing states and is left to the default-quota-set gap.
        quota("File system symbolic link (symlink) length", 4080.0)
            .code("L-B8B14E21")
            .unit("Bytes")
            .fixed()
            .describe(&plane_sentence(
                "The maximum length that a file system symbolic link, or symlink, can be",
            )),
        rate("L-BFB6FB19", "DescribeFileSystems", 100.0),
        rate("L-C8C0D2A0", "DescribeMountTargets", 100.0),
        rate("L-CDCE244F", "DescribeReplicationConfigurations", 100.0),
        rate("L-D6548EE7", "PutBackupPolicy", 10.0),
        rate("L-F3C5D9EF", "PutLifecycleConfiguration", 10.0),
        // The user plane of ONE NFS client: how many users it may have files open for at
        // the same time, which AWS publishes as fixed.
        quota("Active users per NFS client", 65_536.0)
            .code("L-FA0AAA42")
            .fixed()
            .describe(&plane_sentence(
                "The maximum number of active users per NFS client that can have files open \
                 at the same time",
            )),
    ]
}
