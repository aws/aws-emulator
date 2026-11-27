// SPDX-License-Identifier: CC0-1.0

//! Amazon S3's published default-quota catalog.

use super::*;

/// Amazon S3's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `s3` service code (the "Amazon
/// S3 quotas" section of the Amazon S3 User Guide), and the recorded default-quota
/// listing of that code for which `QuotaCode` names which quota, for the units its
/// measured planes are published in and for the one-second window its transfer rate is
/// published over. The rest of the service's published catalog names no quota this
/// catalog can cite under the remaining codes that listing reports, so those rows are
/// left to the default-quota-set gap rather than published under a guessed binding
/// (see [`super::descriptive_quota_code`]).
pub(in crate::state) fn s3_quotas() -> Vec<QuotaRow> {
    vec![
        // The lifecycle-rule plane of a bucket, which AWS publishes as fixed: its
        // sentence names the configuration the rules are specified for, as the
        // replication-rule row below does.
        quota("Lifecycle rules", 1000.0)
            .code("L-146D5F0C")
            .fixed()
            .describe(
                "The maximum number of rules you can specify for an Amazon S3 lifecycle \
                 configuration",
            ),
        // The retrieval-request plane, published as a rate per second and as NOT
        // adjustable: its name, value, adjustability and sentence are the recorded
        // listing's, and that sentence counts the requests of the WHOLE ACCOUNT rather
        // than of one storage class.
        quota("S3 Glacier: Retrieval request rate per second.", 1000.0)
            .code("L-1923B91D")
            .fixed()
            .describe(
                "The maximum number of retrieval requests per second allowed per account.",
            ),
        quota("Replication Destinations", 28.0)
            .code("L-1A9B467F")
            .describe(
                "The number of destination S3 buckets you can add for each source S3 bucket \
                 across all replication rules.",
            ),
        // A quota published as a RATE of data moved rather than as a count, so it
        // carries the one-second window AWS publishes it over.
        quota("Replication transfer rate", 1.0)
            .code("L-349AD9CA")
            .unit("Gigabits")
            .period(1, "SECOND")
            .describe(
                "The maximum Replication Time Control transfer rate that you can replicate \
                 from the source region in this account.",
            ),
        // The POLICY-size plane of a bucket, measured in kilobytes and published as NOT
        // adjustable: how large a bucket policy document may be is a property of the
        // service. The sentence this service publishes under the code is not among the
        // data available here, so it is written in the voice this service words a
        // measured plane with rather than cited.
        quota("Bucket policy", 20.0)
            .code("L-748707F3")
            .unit("Kilobytes")
            .fixed()
            .describe("The maximum size of an Amazon S3 bucket policy."),
        // The DIRECTORY-bucket plane of the account, which the recorded listing
        // publishes under this code at its published default, beside the
        // general-purpose-bucket plane below whose wording its sentence follows.
        quota("Directory buckets", 100.0)
            .code("L-775A314D")
            .describe("The maximum number of directory buckets."),
        // A plane measured in the SIZE of one object rather than in a count of them,
        // and a HARD limit of the service: AWS publishes it as not adjustable, so no
        // increase can be asked for it.
        quota("Object size", 48.828125)
            .code("L-89BABEE8")
            .unit("Terabytes")
            .fixed()
            .describe("The maximum size (in TB) of an Amazon S3 object"),
        // The object-size plane of a CONSOLE upload, measured in gigabytes: the console
        // uploads one object in a single operation, so its ceiling is far below the
        // service's own object-size limit above. Its sentence is not among the data
        // available here, so it is written in this service's measured-plane voice.
        quota("Object size (Console upload)", 160.0)
            .code("L-9CD4D118")
            .unit("Gigabytes")
            .describe(
                "The maximum size of an object that you can upload with the Amazon S3 \
                 console.",
            ),
        // The replication-rule plane of a bucket, which AWS publishes as not
        // adjustable.
        quota("Replication rules", 1000.0)
            .code("L-B461D596")
            .fixed()
            .describe(
                "The maximum number of rules you can specify in an Amazon S3 Replication \
                 configuration",
            ),
        quota("General purpose buckets", 10_000.0)
            .code("L-DC2B2D3D")
            .describe("The maximum number of general purpose buckets."),
        // The ACCESS-POINT plane of the account, which the recorded listing publishes
        // under this code at its published default; its sentence is written in the
        // counting voice the bucket planes above carry rather than cited.
        quota("Access Points", 10_000.0)
            .code("L-FAABEEBA")
            .describe("The maximum number of access points per Region."),
    ]
}
