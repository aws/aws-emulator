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
/// row is published for, so it is passed per row ([`ACO_AUTOMATION_THROTTLE_RATE`] is
/// the rate the evidence states for every cited row alike) and a row whose own limit no
/// source available here states is left unpublished rather than filled in from a
/// neighbour's.
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

/// The requests per second this service publishes the throttle rate of an operation at.
///
/// SOURCE: the recorded default-quota listing of `aco-automation`, which states this
/// same rate for every one of its throttle rows the evidence behind this catalog cites
/// (`L-1501440B`, `L-D0368739`, `L-D0AFC926`, `L-D5E7759E`, `L-E33DC3FB` and the
/// summary-listing and rule rows `L-7EC2BBE4` and `L-AD6FFAA3`) — so it is the rate that
/// class is published at for the operations below rather than one row's own limit. A row
/// whose OPERATION the evidence does not name is still not published: without the
/// operation its code is published for, the row has no identity to publish.
const ACO_AUTOMATION_THROTTLE_RATE: f64 = 5.0;

/// AWS Compute Optimizer Automation's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `aco-automation` service code
/// (what `aws service-quotas list-aws-default-service-quotas --service-code
/// aco-automation` reports), and the recorded default-quota listing of that code for
/// which `QuotaCode` names which operation's throttle rate and what that rate is.
///
/// Every row is published under the `L-` code AWS mints for it, in code order. The rest
/// of that listing's codes name no operation this catalog can cite — the service
/// publishes one throttle row per API operation and the code is the only thing that says
/// which operation a row is for — so they are left to the default-quota-set gap rather
/// than bound to an operation guessed at here.
pub(in crate::state) fn aco_automation_quotas() -> Vec<QuotaRow> {
    let throttle = |api: &str| aco_automation_throttle_rate(api, ACO_AUTOMATION_THROTTLE_RATE);
    vec![
        throttle("ListAutomationRules").code("L-1501440B"),
        // The operations the recorded listing names the code of, each published under
        // that code with the class's window, sentence and adjustability.
        throttle("ListAutomationEventSummaries").code("L-7EC2BBE4"),
        throttle("UpdateEnrollmentConfiguration").code("L-AD63CC14"),
        throttle("DeleteAutomationRule").code("L-AD6FFAA3"),
        throttle("ListRecommendedActionSummaries").code("L-B314103C"),
        throttle("GetAutomationEvent").code("L-D0368739"),
        throttle("ListAutomationEvents").code("L-D0AFC926"),
        throttle("UntagResource").code("L-D5E7759E"),
        throttle("ListRecommendedActions").code("L-E33DC3FB"),
    ]
}
