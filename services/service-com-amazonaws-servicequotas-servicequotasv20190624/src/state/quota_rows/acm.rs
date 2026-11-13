// SPDX-License-Identifier: CC0-1.0

//! AWS Certificate Manager's published default-quota catalog.

use super::*;

/// The `AWS/Usage` display name Amazon CloudWatch publishes AWS Certificate Manager's
/// call counts under, which is the `Service` dimension of every metered ACM quota.
const ACM_USAGE_SERVICE: &str = "Certificate Manager";

/// One ACM API REQUEST-RATE quota: `Rate of <api> API requests`, the most requests to
/// that one operation ACM serves per second.
///
/// ACM publishes a rate quota for each of its API operations, and every member of such
/// a row but the operation's own name and the limit is the same across the class: the
/// `QuotaName` names the operation whose requests the rate throttles, the window is one
/// second, the usage metric is that operation's `AWS/Usage` `CallCount`
/// ([`call_count_metric`]) under the service's `AWS/Usage` display name, and the
/// `Description` is the sentence the class is published with ([`api_rate_description`],
/// the one construction of that sentence in this catalog). Those are the quota's
/// PUBLISHED members, so one constructor fills them and two rows of the class cannot
/// drift apart.
///
/// The per-second LIMIT is not a fact of the class: it belongs to the one operation the
/// row is published for, so it is passed per row and a row whose own limit no source
/// available here states is left unpublished rather than filled in from a neighbour's.
///
/// SOURCE: the recorded default-quota listing of `acm`, which this catalog's
/// `L-DD60D132` row is cited from — the row that recording matches member for member —
/// for the class's name form, its one-second window, its call-count metric, the sentence
/// above and that row's own rate.
fn acm_api_request_rate(api_name: &str, requests_per_second: f64) -> QuotaRow {
    quota(
        format!("Rate of {api_name} API requests"),
        requests_per_second,
    )
    .period(1, "SECOND")
    .metric(call_count_metric(ACM_USAGE_SERVICE, api_name))
    .describe(&api_rate_description(api_name))
}

/// AWS Certificate Manager's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `acm` service code (what `aws
/// service-quotas list-aws-default-service-quotas --service-code acm` reports, which is
/// the "Quotas" page of the AWS Certificate Manager User Guide), and the recorded
/// default-quota listing of that code for which `QuotaCode` names which operation's
/// rate and what that rate is.
///
/// Every row is published under the `L-` code AWS mints for it. The certificate-COUNT
/// quotas this catalog used to publish under descriptive stand-in codes
/// (`ACMCertificates`, `ImportedCertificates`) are reported by that recording as surplus
/// entries and it states no AWS code for either quota, so those rows are dropped rather
/// than paired with a code this catalog would be guessing at. The rest of the service's
/// rate codes have no per-operation rate this catalog can cite, and ACM publishes a
/// different rate per operation, so they stay in the default-quota-set gap.
pub(in crate::state) fn acm_quotas() -> Vec<QuotaRow> {
    vec![acm_api_request_rate("ListAcmeDomainValidations", 50.0).code("L-DD60D132")]
}
