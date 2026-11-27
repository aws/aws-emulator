// SPDX-License-Identifier: CC0-1.0

//! Amazon Redshift's published default-quota catalog.

use super::*;

/// Amazon Redshift's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `redshift` service code (the
/// "Quotas and limits in Amazon Redshift" page of the Amazon Redshift Management
/// Guide), and the recorded default-quota listing of that code for which `QuotaCode`
/// names which quota. Redshift publishes planes of the CLUSTER and of the database
/// inside it beside the account's own, which is why the rows are named for the thing
/// the quota counts rather than for a resource type. The rest of the service's
/// published catalog names no quota this catalog can cite under the remaining codes
/// that listing reports, so those rows are left to the default-quota-set gap rather
/// than published under a guessed binding (see [`super::descriptive_quota_code`]).
pub(in crate::state) fn redshift_quotas() -> Vec<QuotaRow> {
    vec![
        // The STORED-PROCEDURE plane of a database, which the recorded listing publishes
        // under this code as NOT adjustable at its published default. Its sentence is not
        // among the data available here, so it is written in the voice this service words
        // its own rows with rather than cited.
        quota("Stored procedures in a database", 10_000.0)
            .code("L-21B48DE6")
            .fixed()
            .describe("The maximum number of stored procedures for a database."),
        // The schema plane of a database, which AWS publishes as not adjustable.
        quota("Schemas in each database per cluster", 9900.0)
            .code("L-4E1FE923")
            .fixed()
            .describe(
                "The maximum number of schemas that you can create in each database, per \
                 cluster.",
            ),
        // The NODE plane of one cluster node type, which the recorded listing publishes
        // under this code as adjustable and at the node count a cluster of that type may
        // reach — the same count the node plane below publishes for a cluster whatever
        // its type, which that listing publishes as NOT adjustable. Its sentence is not
        // among the data available here, so it is written in the voice that node plane's
        // own sentence uses rather than cited.
        quota("DC2 nodes in a cluster", 128.0)
            .code("L-84537943")
            .describe(
                "The maximum number of DC2 nodes that you can allocate to a cluster.",
            ),
        // The column plane of an external table, whose published sentence states BOTH
        // readings of the limit: the one that holds with pseudocolumns enabled, and
        // the one that holds without them.
        quota(
            "Column limit for external tables with pseudocolumns when using AWS Glue Data \
             Catalog",
            1597.0,
        )
        .code("L-8AFA7F04")
        .describe(
            "The maximum number of columns for external tables when using an AWS Glue Data \
             Catalog, 1,597 when pseudocolumns are enabled and 1,600 when pseudocolumns \
             aren't enabled.",
        ),
        // The snapshot-sharing plane of ONE snapshot, which AWS publishes as fixed.
        // Its published sentence and adjustability are the recorded listing's; the
        // value is the allowance the service's published limits state for the plane.
        quota(
            "AWS accounts that you can authorize to restore a snapshot per snapshot",
            20.0,
        )
        .code("L-909949E5")
        .fixed()
        .describe(
            "The maximum number of AWS accounts that you can authorize to restore a \
             snapshot, per snapshot.",
        ),
        // The two TABLE planes of a cluster node type, and the node plane of a
        // cluster: AWS publishes all three as NOT adjustable, so no increase can be
        // asked for them.
        quota("Tables for 4xlarge cluster node type", 200_000.0)
            .code("L-99F53FC8")
            .fixed()
            .describe("The maximum number of tables for the 4xlarge cluster node type."),
        quota("Nodes in a cluster", 128.0)
            .code("L-BB966966")
            .fixed()
            .describe(
                "The maximum number of nodes that you can allocate to a cluster, based on \
                 the cluster's node type.",
            ),
        // The DATABASE plane of a cluster, which the recorded listing publishes under
        // this code as NOT adjustable: how many databases a cluster may hold is a
        // property of the cluster rather than an account allowance. Its sentence is not
        // among the data available here, so it is written in the voice this service words
        // a per-cluster plane with (the schema plane above) rather than cited.
        quota("User-defined databases in a cluster", 60.0)
            .code("L-CA4E81F8")
            .fixed()
            .describe(
                "The maximum number of user-defined databases that you can create per \
                 cluster.",
            ),
        // A plane MEASURED in kilobytes rather than counted: the recorded listing
        // publishes this code's limit in `Kilobytes` at the size the service's published
        // limits state for a string value read out of an ION or JSON file through an AWS
        // Glue Data Catalog, which is the one plane that page publishes at that size. Its
        // sentence is written in this service's measured-plane voice rather than cited.
        quota(
            "Size of a string value in an ION or JSON file when using an AWS Glue Data \
             Catalog",
            16.0,
        )
        .code("L-D236BDB3")
        .unit("Kilobytes")
        .describe(
            "The maximum size of a string value in an ION or JSON file when using an AWS \
             Glue Data Catalog.",
        ),
        quota("Tables for 16xlarge cluster node type", 200_000.0)
            .code("L-FA4166C9")
            .fixed()
            .describe("The maximum number of tables for the 16xlarge cluster node type."),
    ]
}
