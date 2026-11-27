// SPDX-License-Identifier: CC0-1.0

//! Amazon RDS's published default-quota catalog.

use super::*;

/// The `AWS/Usage` display name Amazon RDS publishes its usage counts under, which is
/// the `Service` dimension of every metered RDS quota.
const RDS_USAGE_SERVICE: &str = "RDS";

/// One Amazon RDS RESOURCE plane as RDS publishes it: a count of the resources of one
/// plane, metered by that plane's `AWS/Usage` `ResourceCount` series over the ONE-HOUR
/// window this service publishes its resource counts over.
///
/// SOURCE: the recorded default-quota listing of `rds`, whose metered rows all carry
/// that window and a `ResourceCount` metric whose `Resource` dimension is the series
/// the plane's count is published under. Every member of such a row but the plane's own
/// name, series and limit is the same across the class, so one constructor fills it and
/// two metered planes cannot drift apart.
fn rds_resource_plane(code: &str, quota_name: &str, value: f64, resource: &str) -> QuotaRow {
    quota(quota_name, value)
        .code(code)
        .period(1, "HOUR")
        .metric(usage_metric(
            RDS_USAGE_SERVICE,
            &[
                ("Class", "None"),
                ("Resource", resource),
                ("Type", "Resource"),
            ],
        ))
}

/// Amazon RDS's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `rds` service code (the
/// "Quotas and constraints for Amazon RDS" page of the Amazon RDS User Guide), and
/// the recorded default-quota listing of that code for which `QuotaCode` names which
/// quota, for the sentence published under each, for the ONE-HOUR window RDS publishes
/// its resource planes over and for the `AWS/Usage` `ResourceCount` metric it meters
/// them with. The rest of the service's published catalog names no quota this catalog
/// can cite under the remaining codes that listing reports, so those rows are left to
/// the default-quota-set gap rather than published under a guessed binding (see
/// [`super::descriptive_quota_code`]).
pub(in crate::state) fn rds_quotas() -> Vec<QuotaRow> {
    vec![
        // The read-replica plane of ONE primary DB instance, metered like the rows
        // below. The series RDS publishes the count under names the plane the quota
        // counts (`ReadReplicasPerMaster`), and the sentence is the recorded listing's,
        // which states BOTH facts of the plane: the count, and that the limit is not
        // adjustable for Aurora however adjustable the quota itself is.
        rds_resource_plane(
            "L-5BC124EF",
            "Read replicas per primary",
            15.0,
            "ReadReplicasPerMaster",
        )
        .describe(
            "The maximum number of read replicas per primary DB instance. This quota can't \
             be adjusted for Amazon Aurora.",
        ),
        // The RESERVED-instance plane of the account, metered like the planes beside it:
        // its purchased reservations are counted for the account in ONE Region, which is
        // what the sentence the recorded listing publishes for it states. The series the
        // count is published under is the plane's own name, the form this service's
        // metered rows name theirs in, rather than a cited fact.
        rds_resource_plane(
            "L-78E853F4",
            "Reserved DB instances",
            40.0,
            "ReservedDBInstances",
        )
        .describe(
            "The maximum number of reserved DB instances allowed in this account in the \
             current AWS Region",
        ),
        // A per-cluster plane RDS meters: the `Service` dimension and the `Resource`
        // series the count is published under are the ones the recorded row carries,
        // and the sentence names the Region the cluster's endpoints are counted in
        // and what a reading of the plane means.
        rds_resource_plane(
            "L-9372BAB3",
            "Custom endpoints per DB cluster",
            5.0,
            "CustomEndpointsPerDBCluster",
        )
        .describe(
            "The maximum number of custom endpoints that you can create per Aurora DB \
             cluster in this account in the current Region. This value reflects the \
             highest number of custom endpoints in a DB cluster in the account. Other DB \
             clusters in the account might have a lower number of custom endpoints.",
        ),
        // The subnet plane of ONE DB subnet group, metered like the planes beside it and
        // published as NOT adjustable: a subnet group's size is a property of the service
        // rather than an allowance an account may ask to have raised. The series the
        // count is published under names the plane the quota counts, as every metered row
        // of this service does.
        rds_resource_plane(
            "L-6F3ACC36",
            "Subnets per DB subnet group",
            20.0,
            "SubnetsPerDBSubnetGroup",
        )
        .fixed()
        .describe("The maximum number of subnets per DB subnet group"),
        rds_resource_plane("L-DE55804A", "Parameter groups", 50.0, "DBParameterGroups")
            .describe("The maximum number of parameter groups"),
        // The two RDS Data API planes, which AWS publishes as not adjustable. The first
        // MEASURES one request's body rather than counting anything, at the size the Data
        // API's published quotas state for it (the "Data API" quotas of the Amazon RDS
        // User Guide); the second counts the pairs in flight for the account in ONE
        // Region, which is what its sentence states. The first row's sentence is not
        // among the data available here, so it is written in this service's counting
        // voice rather than cited.
        quota("Data API HTTP request body size", 4.0)
            .code("L-D87A28C7")
            .unit("Megabytes")
            .fixed()
            .describe("The maximum size of the HTTP request body in a Data API request"),
        quota("Data API maximum concurrent cluster-secret pairs", 30.0)
            .code("L-E3721453")
            .fixed()
            .describe(
                "The maximum number of unique pairs of Aurora Serverless v1 DB clusters and \
                 secrets in concurrent Data API requests for this account in the current AWS \
                 Region.",
            ),
    ]
}
