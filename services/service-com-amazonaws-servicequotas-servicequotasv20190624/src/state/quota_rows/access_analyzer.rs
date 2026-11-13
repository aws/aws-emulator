// SPDX-License-Identifier: CC0-1.0

//! IAM Access Analyzer's published default-quota catalog.

use super::*;

/// IAM Access Analyzer's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `access-analyzer` service code
/// (what `aws service-quotas list-aws-default-service-quotas --service-code
/// access-analyzer` reports, which is the "Quotas for IAM Access Analyzer" table of the
/// IAM User Guide), and the recorded default-quota listing of that code for which
/// `QuotaCode` names which quota.
///
/// Every row is published under the `L-` code AWS mints for it, in code order. The
/// quotas that recorded listing carries under a code whose quota NAME or default VALUE
/// no source available here states — `L-2F63646F`, whose quota that listing counts (the
/// account's analyzers per analyzer type) while the wording of its published name stays
/// withheld, `L-6F85FE0C`, whose quota it names while its value stays withheld, and
/// `L-8750DAE0`, `L-94099EDD` and `L-FD4CD927` — are left to the default-quota-set gap
/// rather than published under a guessed fact, the way [`super::scheduler_quotas`] leaves
/// the rest of that service's catalog. The descriptive stand-in this catalog used to
/// publish the analyzer count under (`AnalyzersPerRegion`) is reported by that listing as
/// a surplus entry, so that row is dropped with them.
pub(in crate::state) fn access_analyzer_quotas() -> Vec<QuotaRow> {
    vec![
        quota("Archive rules per analyzer", 100.0)
            .code("L-1E51937C")
            // The `Description`s this table authors are its own wording, in the form
            // AWS's published quota descriptions take ("The maximum number of <what the
            // quota counts>."): the service's own published text is not among the data
            // available here for those rows, and no rule in the model derives one. The
            // rows whose published text the recorded listing states carry that text.
            .describe("The maximum number of archive rules per analyzer."),
        // The POLICY GENERATION quotas: how many generations may run at once, and how
        // much CloudTrail history one generation may read.
        quota("Concurrent policy generations", 1.0)
            .code("L-4599575C")
            .fixed()
            .describe("The maximum number of concurrent policy generations."),
        quota("CloudTrail log files processed per policy generation", 100000.0)
            .code("L-5EFCE71D")
            .fixed()
            .describe(
                "The maximum number of CloudTrail log files that can be processed \
                 per policy generation.",
            ),
        // A quota measured in DAYS of CloudTrail history, which AWS publishes as not
        // adjustable.
        quota("Policy generation CloudTrail time range", 90.0)
            .code("L-96F55078")
            .fixed()
            .describe(
                "The maximum CloudTrail time range that you can select in days when \
                 you generate a policy.",
            ),
    ]
}
