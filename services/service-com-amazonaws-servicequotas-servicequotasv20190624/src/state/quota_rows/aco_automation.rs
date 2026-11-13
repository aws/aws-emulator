// SPDX-License-Identifier: CC0-1.0

//! AWS Compute Optimizer Automation's published default-quota catalog.

use super::*;

/// One AWS Compute Optimizer Automation API THROTTLE-RATE quota: `Throttle rate limit
/// for <api>`, the most requests to that one operation the service serves per second
/// per account in a Region.
///
/// The service publishes a throttle rate for each of its API operations, and every
/// member of such a row but the operation's own name and the limit is the same across
/// the class: the `QuotaName` names the operation the limit is for, the window is one
/// second, the unit is the countable `None`, AWS publishes the row as not adjustable,
/// and the `Description` is the sentence the class is published with. Those are the
/// quota's PUBLISHED members, so one constructor fills them and two rows of the class
/// cannot drift apart.
///
/// The per-second LIMIT is not a fact of the class: it belongs to the one operation the
/// row is published for, so it is passed per row and a row whose own limit no source
/// available here states is left unpublished rather than filled in from a neighbour's.
///
/// SOURCE: the recorded default-quota listing of `aco-automation`, which this catalog's
/// `L-1501440B` row is cited from — the row that recording matches member for member —
/// for the class's name form, its one-second window, its adjustability, the sentence
/// above and that row's own rate.
fn aco_automation_throttle_rate(api_name: &str, requests_per_second: f64) -> QuotaRow {
    quota(
        format!("Throttle rate limit for {api_name}"),
        requests_per_second,
    )
    .fixed()
    .period(1, "SECOND")
    .describe(&format!(
        "Maximum number of {api_name} requests you can make per second per account in the \
         current region."
    ))
}

/// AWS Compute Optimizer Automation's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `aco-automation` service code
/// (what `aws service-quotas list-aws-default-service-quotas --service-code
/// aco-automation` reports), and the recorded default-quota listing of that code for
/// which `QuotaCode` names which operation's throttle rate and what that rate is.
///
/// The rest of that listing's codes have no per-operation throttle rate this catalog can
/// cite, so they are left to the default-quota-set gap rather than published at a limit
/// borrowed from the row below.
pub(in crate::state) fn aco_automation_quotas() -> Vec<QuotaRow> {
    vec![aco_automation_throttle_rate("ListAutomationRules", 5.0).code("L-1501440B")]
}
