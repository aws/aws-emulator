// SPDX-License-Identifier: CC0-1.0

//! AWS Private Certificate Authority's published default-quota catalog.

use super::*;

/// One AWS Private CA API REQUEST-RATE quota: `Rate of <api> requests`, the most
/// requests to that one operation Private CA performs in an account in a Region each
/// second.
///
/// AWS Private CA publishes a rate quota for each of its API operations, and every
/// member of such a row but the operation's own name and the limit is the same across
/// the class: the `QuotaName` names the operation the rate is of, the window is one
/// second, AWS publishes the rate as not adjustable, and the `Description` is the
/// sentence the class is published with, with the throttled operation named in it.
/// Those are the quota's PUBLISHED members, so one constructor fills them and two rows
/// of the class cannot drift apart.
///
/// SOURCE: the recorded default-quota listing of `acm-pca`, which this catalog's
/// `L-01B8608D` row is cited from — the row the recording matches member for member —
/// for the class's name form, its one-second window, its adjustability and the sentence
/// above; the per-operation rate is the one that listing states for the row.
fn acm_pca_request_rate(api_name: &str, requests_per_second: f64) -> QuotaRow {
    quota(format!("Rate of {api_name} requests"), requests_per_second)
        .fixed()
        .period(1, "SECOND")
        .describe(&format!(
            "The maximum number of {api_name} requests that can be performed in an account in \
             the current Region each second."
        ))
}

/// AWS Private Certificate Authority's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `acm-pca` service code (what
/// `aws service-quotas list-aws-default-service-quotas --service-code acm-pca` reports,
/// which is the "Quotas" page of the AWS Private Certificate Authority User Guide), and
/// the recorded default-quota listing of that code for which `QuotaCode` names which
/// operation's rate and what that rate is.
///
/// Every row is published under the `L-` code AWS mints for it, in code order. The
/// private-CA COUNT quota this catalog used to publish under a descriptive stand-in
/// code (`CertificateAuthorities`) is reported by that recording as a surplus entry and
/// the recording states no AWS code for it, so the row is dropped rather than paired
/// with a code this catalog would be guessing at; the rate rows whose own per-second
/// limit no source available here states are likewise left to the default-quota-set
/// gap.
pub(in crate::state) fn acm_pca_quotas() -> Vec<QuotaRow> {
    vec![
        acm_pca_request_rate("CreateCertificateAuthority", 1.0).code("L-01B8608D"),
        acm_pca_request_rate("DeleteCertificateAuthority", 10.0).code("L-4065FDCB"),
        acm_pca_request_rate("PutPolicy", 5.0).code("L-506CC328"),
        acm_pca_request_rate("ListCertificateAuthorities", 20.0).code("L-5A3CF806"),
        acm_pca_request_rate("CreatePermission", 1.0).code("L-773738D3"),
    ]
}
