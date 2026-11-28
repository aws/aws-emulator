// SPDX-License-Identifier: CC0-1.0

//! Amazon DocumentDB's published default-quota catalog.

use super::*;

/// Amazon DocumentDB's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `docdb` service code (the "Quotas
/// and limits in Amazon DocumentDB" page of its Developer Guide), and the recorded
/// default-quota listing of that code for which `QuotaCode` names which quota, for the
/// value and adjustability each publishes and for the `Description` the service publishes
/// under a code the listing states one for — a terse sentence naming the plane the quota
/// counts, with no trailing stop. Every `QuotaCode` that listing reports for this service
/// code is published; a code whose quota no source available here states would be left to
/// the default-quota-set gap rather than published under a guessed binding (see
/// [`crate::state::descriptive_quota_code`]), since a code identifies ONE quota.
pub(in crate::state) fn docdb_quotas() -> Vec<QuotaRow> {
    vec![
        quota("Subnet groups", 50.0)
            .code("L-02DEA053")
            .describe("The maximum number of DB subnet groups"),
        quota("Clusters", 40.0).code("L-13F31459").describe(
            "The maximum number of clusters allowed in this account in the current Region",
        ),
        // The cluster parameter groups of the Region and the subnets ONE subnet group may
        // hold, both of which AWS publishes as hard limits, each with the terse sentence
        // the listing states for it.
        quota("Cluster parameter groups", 50.0)
            .code("L-2A542E16")
            .fixed()
            .describe("The maximum number of DB cluster parameter groups"),
        quota("Subnets per subnet group", 20.0)
            .code("L-36C7F3F8")
            .fixed()
            .describe("The maximum number of subnets per DB subnet group"),
        quota("Read replicas per cluster", 15.0)
            .code("L-5BA57179")
            // The `Description` on this row is this catalog's own wording, in the terse
            // form this service's published descriptions take: it publishes one for the
            // quota and its text is not among the data available here.
            .describe("The maximum number of read replicas per cluster"),
        // The instances of the account in the Region, published with the sentence this
        // service states for that plane.
        quota("Instances", 40.0)
            .code("L-739A3A85")
            .describe(
                "The maximum number of DB instances allowed in this account in the current Region",
            ),
        quota("Manual cluster snapshots", 100.0)
            .code("L-B2551F83")
            .describe("The maximum number of manual snapshots"),
        // The security-group plane of a VPC, which AWS publishes as fixed.
        quota("VPC security groups per instance", 5.0)
            .code("L-D02D85EA")
            .fixed()
            .describe("The maximum number of DB security groups per Amazon VPC"),
        quota("Event subscriptions", 20.0)
            .code("L-F7FABF71")
            .describe("The maximum number of event subscriptions"),
        // The recording states this code's name but not its value. SOURCE for the 50:
        // the "Quotas and limits in Amazon DocumentDB" page, whose tag row publishes 50
        // tags per resource. AWS publishes it as a hard limit rather than as adjustable,
        // and describes it as a quota of the Amazon RDS resource the cluster is one of.
        quota("Tags per resource", 50.0)
            .code("L-3B0E1499")
            .fixed()
            .describe("The maximum number of tags per Amazon RDS resource"),
    ]
}
