// SPDX-License-Identifier: CC0-1.0

//! IAM Access Analyzer's published default-quota catalog.

use super::*;

/// IAM Access Analyzer's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `access-analyzer` service code
/// (what `aws service-quotas list-aws-default-service-quotas --service-code
/// access-analyzer` reports, which is the "Quotas for IAM Access Analyzer" table of the
/// IAM User Guide), and the recorded default-quota listing of that code for which
/// `QuotaCode` names which quota, its published name, its default value and its
/// adjustability.
///
/// Every row is published under the `L-` code AWS mints for it, in code order, with the
/// `QuotaName`, `Value`, `Unit`, `Adjustable` and `Description` of the quota published
/// UNDER THAT CODE — the analyzer planes (`L-2F63646F` for the account's analyzers of a
/// type, `L-6F85FE0C` for the organization's) and the policy-generation planes
/// (`L-8750DAE0`, `L-94099EDD`) included.
///
/// The one code that listing carries whose quota this catalog publishes no row for is
/// none: the descriptive stand-in this catalog once published the analyzer count under
/// (`AnalyzersPerRegion`) is reported by that listing as a surplus entry, so it is gone —
/// the two analyzer planes are published under AWS's own codes above.
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
        // The ANALYZER planes, one per zone of trust: how many analyzers of a type an
        // account may have, and how many of them may trust the whole organization. AWS
        // publishes the account plane as fixed and the organization plane as adjustable,
        // and words their sentences alike around the plane each counts — the
        // organization row's text below is the one the recorded listing states verbatim,
        // so the account row carries that same class sentence with its own plane in it.
        quota("Account level analyzer", 1.0)
            .code("L-2F63646F")
            .fixed()
            .describe(
                "The maximum number of account level analyzers that can be created per \
                 analyzer type per AWS Account per Region.",
            ),
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
        quota("Organization level analyzer", 5.0)
            .code("L-6F85FE0C")
            .describe(
                "The maximum number of organization level analyzers that can be created \
                 per analyzer type per AWS Account per Region.",
            ),
        // The access previews one analyzer may run in an hour: a count AWS publishes
        // over no window of its own, with the hour stated in the quota's own name.
        quota("Access previews per analyzer per hour", 1000.0)
            .code("L-8750DAE0")
            .describe("The maximum number of access previews per analyzer per hour."),
        // The CloudTrail history one generation may read, published in GIGABYTES beside
        // the same plane's time range below, and as not adjustable the way that row is.
        // Its published sentence is the one the recorded default-quota listing of this
        // service code states for this code — a SIZE of data rather than a count, which
        // is why neither the counted form nor the time-range row's wording reads for it.
        quota("Policy generation CloudTrail data size", 25.0)
            .code("L-94099EDD")
            .unit("Gigabytes")
            .fixed()
            .describe("The maximum size of CloudTrail data per policy generation."),
        // A quota measured in DAYS of CloudTrail history, which AWS publishes as not
        // adjustable.
        quota("Policy generation CloudTrail time range", 90.0)
            .code("L-96F55078")
            .fixed()
            .describe(
                "The maximum CloudTrail time range that you can select in days when \
                 you generate a policy.",
            ),
        // The policy generations a day allows, the third row of the policy-generation
        // plane and, like the rest of it, published as not adjustable. Its published
        // name and default are AWS's own published quota data for this service code, and
        // its sentence is the counted form every other count of this service publishes
        // (its own name read as the phrase it counts, see [`super::maximum_number_phrase`]).
        quota("Policy generations per day", 50.0)
            .code("L-FD4CD927")
            .fixed()
            .describe(&format!(
                "{}.",
                maximum_number_phrase("policy generations per day")
            )),
    ]
}
