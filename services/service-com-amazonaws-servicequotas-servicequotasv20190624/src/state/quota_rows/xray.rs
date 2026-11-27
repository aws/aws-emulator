// SPDX-License-Identifier: CC0-1.0

//! AWS X-Ray's published default-quota catalog.

use super::*;

/// AWS X-Ray's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `xray` service code (what `aws
/// service-quotas list-aws-default-service-quotas --service-code xray` reports, which is
/// the "AWS X-Ray endpoints and quotas" page of the AWS General Reference), and the
/// `QuotaCode`s that catalog is keyed on.
///
/// X-Ray publishes the sampling and grouping planes of the account beside the request
/// rates of the APIs its agents and consoles call, which is what the rows built by
/// [`request_rate`] are: a rate, so each carries the one-second window AWS publishes it
/// over and the `AWS/Usage` `CallCount` of the API it throttles.
///
/// The recorded listing states the quota published under `L-8C0C998A` (the account's
/// custom sampling rules in the Region, 25), under `L-DB51D338` (the tags of one custom
/// sampling rule, 50, NOT adjustable), under `L-428FB8E0` (the dynamic upper limit on a
/// trace document, 500 Kilobytes, NOT adjustable), under `L-C6B6F05D` (the annotations
/// indexed within one trace, with the sentence the service words it with), under
/// `L-E2DD2778` (the tags of one group, 50, NOT adjustable), under `L-E75A2BBF` (the
/// segments per second an account may send, 2600), under `L-D781C0FD` (the size of one
/// segment document), under `L-998BFF16` (the days X-Ray retains trace and service graph
/// data, 30, NOT adjustable), under `L-7F259013` (the GROUPS of an account, 25, NOT
/// adjustable) and under `L-5DB17969` (the days during which trace data may still be
/// modified, 7, NOT adjustable). The `BatchGetTraces`, `PutTraceSegments`,
/// `GetTraceSummaries`, `PutTelemetryRecords` and `GetServiceGraph` rates this catalog used
/// to key on those codes are NOT published here: no source available here states AWS's code
/// for them, so they are left to the default-quota-set gap rather than served under another
/// quota's code. A row whose published `Description` no source available here states
/// carries none, rather than a sentence this emulator words itself.
///
/// THE CODE NO SOURCE BINDS. `L-3B8EFF88` is a code this catalog publishes whose own quota
/// no source available here names, so it carries a plane one of the bindings above
/// displaced — the `GetSamplingRules` rate — as the module header says such a row does.
/// That is not a claim that AWS publishes that rate under that code.
pub(in crate::state) fn xray_quotas() -> Vec<QuotaRow> {
    let rate = |code: &str, api: &str, per_second: f64| {
        request_rate(code, XRAY_USAGE_SERVICE, api, per_second)
    };
    vec![
        // A code whose own published quota no source available here states, carrying a
        // plane one of the sourced bindings displaced (see the header).
        rate("L-3B8EFF88", "GetSamplingRules", 10.0),
        quota("Trace document size (dynamic upper limit)", 500.0)
            .code("L-428FB8E0")
            .unit("Kilobytes")
            .fixed(),
        // The period during which trace data may still be modified, which the service
        // publishes in days, with the unit in the quota's own name, as NOT adjustable.
        quota("Trace data modification period in days", 7.0)
            .code("L-5DB17969")
            .fixed(),
        // The GROUPS of an account, which the recorded listing publishes under this code
        // as NOT adjustable.
        quota("Groups in an account", 25.0)
            .code("L-7F259013")
            .fixed()
            .describe("The maximum number of groups in this account in the current Region."),
        quota("Custom sampling rules per region", 25.0).code("L-8C0C998A"),
        // The retention of trace and service graph data, which the service publishes in
        // days, with the unit in the quota's own name, as NOT adjustable.
        quota("Trace and service graph retention in days", 30.0)
            .code("L-998BFF16")
            .fixed(),
        quota("Indexed annotations per trace", 50.0)
            .code("L-C6B6F05D")
            .fixed()
            .describe(
                "The maximum number of annotations that can be indexed within a single \
                 trace.",
            ),
        quota("Segment document size", 64.0)
            .code("L-D781C0FD")
            .unit("Kilobytes")
            .fixed()
            .describe("The maximum size for segment documents."),
        quota("Tags per custom sampling rule", 50.0)
            .code("L-DB51D338")
            .fixed(),
        quota("Tags per group", 50.0)
            .code("L-E2DD2778")
            .fixed()
            .describe("The maximum number of tags per group."),
        // The segments an account may send to X-Ray per second, which the service
        // publishes as a plain count rather than over a window of its own.
        quota("Segments per second", 2600.0)
            .code("L-E75A2BBF")
            .fixed()
            .describe(
                "The maximum number of segments per second you can send to X-Ray.",
            ),
    ]
}

/// The name `AWS/Usage` publishes AWS X-Ray's API call counts under.
///
/// It is the `Service` dimension value a `GetMetricData` query has to name for one of
/// these counts, so it is held once here rather than spelled at each row.
const XRAY_USAGE_SERVICE: &str = "X-Ray";
