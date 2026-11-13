// SPDX-License-Identifier: CC0-1.0

//! Amazon Timestream's published default-quota catalog.

use super::*;

/// One Amazon Timestream API REQUEST-RATE quota: the requests per second the service
/// serves for one of its APIs.
///
/// SOURCE: the recorded default-quota listing of `timestream`, which publishes every row
/// of this class under the name `<Api> request rate`, described by the sentence of the
/// class with that API named in it, in `Unit` `None`, NOT adjustable, over the one-second
/// window the rate is published over, and metered by the `AWS/Usage` `CallCount` of the
/// API it throttles ([`call_count_metric`]).
///
/// It is Timestream's OWN wording of the class, not the `Rate of <Api> requests` wording
/// other services publish their rates under ([`request_rate`]), so every row of this
/// service's class is built here and two of them cannot drift apart.
fn timestream_request_rate(code: &str, api_name: &str, per_second: f64) -> QuotaRow {
    quota(format!("{api_name} request rate"), per_second)
        .code(code)
        .fixed()
        .period(1, "SECOND")
        .metric(call_count_metric(TIMESTREAM_USAGE_SERVICE, api_name))
        .describe(&format!(
            "Maximum number of {api_name} requests per second."
        ))
}

/// Amazon Timestream's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `timestream` service code (what
/// `aws service-quotas list-aws-default-service-quotas --service-code timestream` reports,
/// which is the "Quotas" page of the Amazon Timestream Developer Guide), and the
/// `QuotaCode`s that catalog is keyed on. The recorded default-quota listing of
/// `timestream` states which API's rate is published under `L-F07A9AB4` (`ListBatchLoadTasks`,
/// 5 per second), under `L-798CB19D` (`CreateScheduledQuery`, 1 per second) and under
/// `L-C49D398A` (`CreateBatchLoadTask`), beside the wording and the adjustability of the
/// whole rate class ([`timestream_request_rate`]).
///
/// Timestream publishes the planes of its data model — the databases, the tables, the
/// shape of one record and the retention of each store — beside the request rate of each
/// of its control-plane and query APIs. A rate row is built by
/// [`timestream_request_rate`], so it carries the one-second window AWS publishes it over
/// and the `AWS/Usage` `CallCount` of the API it throttles; a row that measures something
/// carries the unit AWS publishes it in rather than being a bare count.
///
/// The `WriteRecords` request rate, the dimension-name length and the records one
/// `WriteRecords` request takes are NOT published here: the codes this catalog used to key
/// them on are the three the recorded listing publishes the batch-load and scheduled-query
/// rates under, and no source available here states AWS's code for them, so they are left
/// to the default-quota-set gap rather than served under another quota's code.
pub(in crate::state) fn timestream_quotas() -> Vec<QuotaRow> {
    let rate = timestream_request_rate;
    vec![
        quota("Active queries per account", 1000.0)
            .code("L-00158752")
            .describe(
                "The maximum number of queries running at once in this account in the \
                 current Region.",
            ),
        rate("L-05CF2F29", "CancelQuery", 10.0),
        rate("L-1222731D", "CreateDatabase", 1.0),
        rate("L-279D7B08", "CreateTable", 1.0),
        quota("Databases per account", 500.0)
            .code("L-3F031494")
            .describe(
                "The maximum number of databases in this account in the current Region.",
            ),
        rate("L-6044E01C", "DeleteDatabase", 1.0),
        rate("L-6B1915C6", "DeleteTable", 1.0),
        rate("L-6C25862A", "DescribeDatabase", 10.0),
        rate("L-71D80F70", "DescribeEndpoints", 10.0),
        rate("L-7565C2A8", "DescribeTable", 10.0),
        // The scheduled-query and batch-load rates the recorded listing names, each under
        // the code THAT listing publishes it under. `CreateBatchLoadTask` is published at
        // the one request per second Timestream publishes its create APIs at, which is the
        // rate the recorded `CreateScheduledQuery` row carries.
        rate("L-798CB19D", "CreateScheduledQuery", 1.0),
        quota("Dimensions per series", 128.0)
            .code("L-7C9FE50E")
            .fixed()
            .describe("The maximum number of dimensions of one time series."),
        rate("L-8892F7F6", "ListDatabases", 10.0),
        rate("L-89E3B36D", "ListTables", 10.0),
        rate("L-8BF444C0", "ListTagsForResource", 10.0),
        quota("Magnetic store retention", 73_000.0)
            .code("L-8DFD99E7")
            .unit("Days")
            .describe(
                "The maximum time, in days, that a table retains data in its magnetic \
                 store.",
            ),
        quota("Measure name length", 256.0)
            .code("L-8FD9593E")
            .unit("Bytes")
            .fixed()
            .describe("The maximum length, in bytes, of one measure name."),
        quota("Measure values per record", 256.0)
            .code("L-9F20DF3A")
            .fixed()
            .describe("The maximum number of measure values of one multi-measure record."),
        quota("Memory store retention", 8766.0)
            .code("L-A83B13DB")
            .unit("Hours")
            .describe(
                "The maximum time, in hours, that a table retains data in its memory store.",
            ),
        rate("L-AE604CE2", "Query", 10.0),
        quota("Query string length", 262_144.0)
            .code("L-BFCD0C9E")
            .unit("Bytes")
            .fixed()
            .describe("The maximum length, in bytes, of one query string."),
        quota("Query timeout", 60.0)
            .code("L-C0E9F2ED")
            .unit("Minutes")
            .fixed()
            .describe("The time, in minutes, after which a running query is cancelled."),
        rate("L-C49D398A", "CreateBatchLoadTask", 1.0),
        rate("L-F07A9AB4", "ListBatchLoadTasks", 5.0),
        quota("Scheduled queries per account", 10_000.0)
            .code("L-F1AC16A2")
            .describe(
                "The maximum number of scheduled queries in this account in the current \
                 Region.",
            ),
        quota("Tables per database", 50_000.0)
            .code("L-F98EACE7")
            .describe("The maximum number of tables in one database."),
        quota("Unique measure names per table", 1024.0)
            .code("L-FA296006")
            .fixed()
            .describe("The maximum number of distinct measure names in one table."),
        rate("L-FD5A0C1A", "UpdateTable", 1.0),
        quota("Batch load tasks per account", 100.0)
            .code("L-FEB34FC6")
            .describe(
                "The maximum number of batch load tasks in this account in the current \
                 Region.",
            ),
    ]
}

/// The name `AWS/Usage` publishes Amazon Timestream's API call counts under.
///
/// It is the `Service` dimension value a `GetMetricData` query has to name for one of
/// these counts, so it is held once here rather than spelled at each row.
const TIMESTREAM_USAGE_SERVICE: &str = "Timestream";
