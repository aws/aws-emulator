// SPDX-License-Identifier: CC0-1.0

//! Amazon EventBridge Scheduler's published default-quota catalog.

use super::*;

/// The name Amazon EventBridge Scheduler rejects throttled requests under, which is also
/// the `AWS/Usage` display name its call counts are published under.
const SCHEDULER_USAGE_SERVICE: &str = "EventBridge Scheduler";

/// The guidance sentence Amazon EventBridge Scheduler publishes on a request-rate quota it
/// will raise: how far an increase can take the rate.
///
/// SOURCE: the recorded default-quota listing of `scheduler`, which carries this sentence
/// after the class's own ([`interval_rate_description`]) on the rate rows it publishes it
/// for, and without it on the rate the service publishes no such guidance for.
const SCHEDULER_RATE_ADJUSTMENT: &str =
    "This is adjustable to tens of thousands of requests per second.";

/// One Amazon EventBridge Scheduler API REQUEST-RATE quota: `<api> request rate`, the
/// most requests to that one operation Scheduler serves per second.
///
/// Scheduler publishes a rate quota for each of its API operations, and every member
/// of such a row but the operation's own name and the value is the same across the
/// class: the `QuotaName` is the operation's name followed by `request rate`, the
/// window is one second, the usage metric is that operation's `AWS/Usage` `CallCount`
/// ([`call_count_metric`]) under the service's `AWS/Usage` display name, and the
/// `Description` is the sentence this service publishes for a request-rate quota
/// ([`interval_rate_description`]) with the operation it throttles named in it.
fn scheduler_request_rate(api_name: &str, requests_per_second: f64) -> QuotaRow {
    quota(format!("{api_name} request rate"), requests_per_second)
        .period(1, "SECOND")
        .metric(call_count_metric(SCHEDULER_USAGE_SERVICE, api_name))
        .describe(&interval_rate_description(
            SCHEDULER_USAGE_SERVICE,
            api_name,
        ))
}

/// One such rate quota that Scheduler publishes its ADJUSTMENT GUIDANCE on as well: the
/// class's sentence followed by how far an increase can take the rate.
fn scheduler_adjustable_request_rate(api_name: &str, requests_per_second: f64) -> QuotaRow {
    let described = format!(
        "{} {SCHEDULER_RATE_ADJUSTMENT}",
        interval_rate_description(SCHEDULER_USAGE_SERVICE, api_name)
    );
    scheduler_request_rate(api_name, requests_per_second).describe(&described)
}

/// Amazon EventBridge Scheduler's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `scheduler` service code (the
/// "Quotas for EventBridge Scheduler" page of the Amazon EventBridge Scheduler User
/// Guide), and the recorded default-quota listing of that code for which `QuotaCode`
/// names which quota, for the rate each is published at and for the sentence published
/// under each: the account's schedule plane and the service's invocation rate beside a
/// per-API request rate for each
/// throttled operation. The rest of the service's published catalog names no quota this
/// catalog can cite under the remaining codes that listing reports, so those rows are
/// left to the default-quota-set gap rather than published under a guessed binding (see
/// [`super::descriptive_quota_code`]).
pub(in crate::state) fn scheduler_quotas() -> Vec<QuotaRow> {
    vec![
        // The TAG-listing rate, which the service publishes far below every other rate
        // beside it. Which of the class's two sentences it publishes is not among the
        // data available here, so it carries the class's own, as the schedule listing
        // below does, rather than the adjustment guidance the write rates carry.
        scheduler_request_rate("ListTagsForResource", 10.0).code("L-1DC60392"),
        // The DeleteSchedule rate, published at the write rate of the Create and Update
        // rows and with the adjustment guidance they carry.
        scheduler_adjustable_request_rate("DeleteSchedule", 1000.0).code("L-53944E21"),
        // The LISTING rate, which the service publishes far below the write rates beside
        // it and without the adjustment guidance they carry.
        scheduler_request_rate("ListSchedules", 50.0).code("L-66629D52"),
        scheduler_adjustable_request_rate("CreateSchedule", 5000.0).code("L-6F4B17DE"),
        // The UpdateSchedule rate, published under the name, the sentence and the rate
        // the recorded listing carries for it: the class's `<api> request rate` name,
        // and the guidance sentence the service's write rates carry.
        scheduler_adjustable_request_rate("UpdateSchedule", 1000.0).code("L-B7845AAE"),
        // The account's schedule plane: a count rather than a rate, so its sentence
        // carries the service's guidance about the one-time schedules that stay counted
        // and about how far the plane can be raised.
        quota("Number of schedules", 10_000_000.0)
            .code("L-EE5D6FF0")
            .describe(
                "The maximum number of schedules per region. This quota includes one-time \
                 schedules that have completed running. We recommend configuring your \
                 schedules to automatically delete after completion using the \
                 ActionAfterCompletion feature. This is adjustable to billions of schedules.",
            ),
        scheduler_adjustable_request_rate("GetSchedule", 1000.0).code("L-FB03E819"),
        // The INVOCATION rate of the service: a throttle on the schedules it runs rather
        // than on an API operation, so no API's call count meters it, and the service
        // names it for the transactions per second it states in its own name. Its
        // sentence is not among the data available here, so it is written in the counting
        // voice this service words its own schedule plane with rather than cited.
        quota("Invocations throttle limit in transactions per second", 1000.0)
            .code("L-F6D2596E")
            .period(1, "SECOND")
            .describe("The maximum number of schedule invocations per second per region."),
    ]
}
