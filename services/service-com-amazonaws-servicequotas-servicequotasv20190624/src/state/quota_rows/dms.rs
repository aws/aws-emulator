// SPDX-License-Identifier: CC0-1.0

//! AWS Database Migration Service's published default-quota catalog.

use super::*;

/// AWS Database Migration Service's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `dms` service code (the "Quotas
/// for AWS Database Migration Service" page of its User Guide), and the recorded
/// default-quota listing of that code for which `QuotaCode` names which quota, for the
/// value and adjustability each publishes and for the `Description` the service publishes
/// under a code the listing states one for. The remaining codes that listing reports name
/// quotas whose published name or value no source available here states, and a code
/// identifies ONE quota, so those rows are left to the default-quota-set gap rather than
/// published under a guessed binding (see [`crate::state::descriptive_quota_code`]).
pub(in crate::state) fn dms_quotas() -> Vec<QuotaRow> {
    vec![
        // The data migrations of the account, published with the terse sentence this
        // service words that plane's text in.
        quota("Number of Data Migrations", 1000.0)
            .code("L-1AB41EE0")
            .describe("Maximum number of data migrations"),
        // The endpoint plane of one replication instance. The `Description` is this
        // catalog's own wording, in the form AWS's published quota descriptions take: the
        // service publishes one for this quota and its text is not among the data
        // available here.
        quota("Endpoints per instance", 100.0)
            .code("L-2146F1FD")
            .describe("The maximum number of endpoints allowed per replication instance."),
        // The data migrations of ONE migration project, which AWS publishes as a hard
        // limit of the project, and the migration projects of the account below. The
        // sentence AWS publishes for each is not among the data available here, so neither
        // row carries one.
        quota("Number of Data Migrations per Migration Project", 10.0)
            .code("L-62EFB27A")
            .fixed(),
        quota("Event subscriptions", 60.0)
            .code("L-D97343A2")
            .describe("The maximum number of AWS DMS events that you can subscribe to."),
        quota("Number of Migration Projects", 50.0).code("L-DE63148C"),
        quota("Endpoint count", 1000.0)
            .code("L-E17328E9")
            .describe(
                "The maximum number of endpoints allowed in this account in the current Region.",
            ),
        quota("Serverless replications", 100.0)
            .code("L-E569F59D")
            .describe(
                "The maximum number of serverless replications allowed in this account in the \
                 current Region.",
            ),
        quota("Number of running Data Migrations", 500.0)
            .code("L-FBEA20FB")
            .describe("Maximum number of running data migrations simultaneously."),
        // The certificates of the account in the Region.
        quota("Certificate count", 100.0)
            .code("L-FE918D88")
            .describe(
                "The maximum number of certificates allowed in this account in the current \
                 Region.",
            ),
    ]
}
