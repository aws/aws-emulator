// SPDX-License-Identifier: CC0-1.0

//! A series holding BOTH storage resolutions, read at every period a read may ask for, end to
//! end over the wire.
//!
//! ONE rule, asserted over a table: **a read answers from each datapoint only at the
//! granularities that datapoint is STORED at — the multiples of its own `StorageResolution` — and
//! places it at the start of the span that resolution stores it in: "Setting this to 1 specifies
//! this metric as a high-resolution metric, so that CloudWatch stores the metric with sub-minute
//! resolution down to one second. Setting this to 60 specifies this metric as regular resolution,
//! which CloudWatch stores at 1-minute resolution."
//! (`smithy_query(query="doc",shape="MetricDatum",member="StorageResolution")`) — so a period
//! FINER than a datapoint's storage granularity answers with no datapoint for it, while a period
//! that granularity divides answers from it, bucketed by the span it is stored in (the whole
//! minute for a regular-resolution one, its own second for a high-resolution one). That holds at
//! EVERY period a read may ask for, the one-second period included: it resolves the per-second
//! detail only high-resolution storage keeps — "High-resolution metrics are those metrics stored
//! by a PutMetricData call that includes a StorageResolution of 1 second."
//! (`GetMetricStatisticsInput$Period`) — so there the high-resolution half stands at its own
//! second and the regular-resolution half is not answered at all.**
//!
//! The series under test carries one datapoint of each resolution inside the same minute, which
//! is what tells the two apart: at a division of a minute, and at the one-second period, the read
//! answers with the high-resolution datapoint alone, and at a period of a minute or more the two
//! fold together. A read that answered the regular-resolution datapoint at a sub-minute period, or
//! dropped either datapoint at a period of a minute or more, fails every row.
//!
//! The rows differ along the period the read asks for: every sub-minute value `MetricStat$Period`
//! admits (1, 5, 10, 20, 30), the minute itself and a multiple of it. Both readers of a series
//! are driven, because one fold answers for both.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{build_router, ok};

/// One minute, in the epoch seconds a published time stamp carries.
const MINUTE: f64 = 60.0;

/// The namespace the series is published under.
const NAMESPACE: &str = "MyApp/MixedResolution";

/// The metric the series is published under.
const METRIC: &str = "MixRes";

/// How far into its minute the HIGH-resolution datapoint is published: a second that is a
/// multiple of none of the sub-minute periods but the finest, so a row reading at 5, 10, 20 or
/// 30 seconds reports a bucket boundary rather than the publish instant itself.
const HIGH_RESOLUTION_OFFSET: f64 = 37.0;

/// How far into the same minute the REGULAR-resolution datapoint is published: another second
/// of that minute, so the two datapoints are distinct instants of one minute.
const REGULAR_RESOLUTION_OFFSET: f64 = 11.0;

/// An instant ten minutes in the past, on a boundary of every period the rows read: recent
/// enough to sit inside every retention tier ("Data points with a period of less than 60 seconds
/// are available for 3 hours"), and on the clock boundary of the widest of those periods so that
/// a read's own window grid begins exactly where this base is rather than one window earlier.
fn base_minute() -> f64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64();
    (now / WIDEST_PERIOD).floor() * WIDEST_PERIOD - 10.0 * MINUTE
}

/// The widest `Period` any row reads, in seconds: the grid [`base_minute`] sits on.
const WIDEST_PERIOD: f64 = 300.0;

/// Publish the mixed series: one datapoint at each storage resolution, inside one minute.
async fn publish(router: &MultiServiceRouter, base: f64) {
    ok(
        router,
        "PutMetricData",
        &json!({
            "Namespace": NAMESPACE,
            "MetricData": [
                {
                    "MetricName": METRIC,
                    "Dimensions": [{"Name": "Service", "Value": "api"}],
                    "Value": 1.0,
                    "Unit": "Count",
                    "StorageResolution": 1,
                    "Timestamp": base + HIGH_RESOLUTION_OFFSET,
                },
                {
                    "MetricName": METRIC,
                    "Dimensions": [{"Name": "Service", "Value": "api"}],
                    "Value": 1.0,
                    "Unit": "Count",
                    "StorageResolution": 60,
                    "Timestamp": base + REGULAR_RESOLUTION_OFFSET,
                },
            ],
        }),
    )
    .await;
}

/// The `(offset from `base`, SampleCount)` buckets `GetMetricStatistics` answers over the two
/// minutes from `base` at `period`, oldest first.
async fn statistics_buckets(
    router: &MultiServiceRouter,
    base: f64,
    period: i64,
) -> Vec<(f64, f64)> {
    let body = ok(
        router,
        "GetMetricStatistics",
        &json!({
            "Namespace": NAMESPACE,
            "MetricName": METRIC,
            "Dimensions": [{"Name": "Service", "Value": "api"}],
            "StartTime": base,
            "EndTime": base + MINUTE,
            "Period": period,
            "Statistics": ["SampleCount"],
        }),
    )
    .await;
    let mut buckets: Vec<(f64, f64)> = body["Datapoints"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|point| {
            (
                point["Timestamp"].as_f64().unwrap_or_default() - base,
                point["SampleCount"].as_f64().unwrap_or_default(),
            )
        })
        .collect();
    buckets.sort_by(|left, right| left.0.total_cmp(&right.0));
    buckets
}

/// The same buckets as `GetMetricData` answers with, oldest first.
async fn metric_data_buckets(
    router: &MultiServiceRouter,
    base: f64,
    period: i64,
) -> Vec<(f64, f64)> {
    let body = ok(
        router,
        "GetMetricData",
        &json!({
            "MetricDataQueries": [{
                "Id": "mix",
                "MetricStat": {
                    "Metric": {
                        "Namespace": NAMESPACE,
                        "MetricName": METRIC,
                        "Dimensions": [{"Name": "Service", "Value": "api"}],
                    },
                    "Period": period,
                    "Stat": "SampleCount",
                },
            }],
            "StartTime": base,
            "EndTime": base + MINUTE,
            "ScanBy": "TimestampAscending",
        }),
    )
    .await;
    let result = &body["MetricDataResults"][0];
    let stamps: Vec<f64> = result["Timestamps"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(Value::as_f64)
        .collect();
    let values: Vec<f64> = result["Values"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(Value::as_f64)
        .collect();
    assert_eq!(
        stamps.len(),
        values.len(),
        "a result's Timestamps and Values stay in step, got {result}"
    );
    let mut buckets: Vec<(f64, f64)> = stamps
        .into_iter()
        .map(|stamp| stamp - base)
        .zip(values)
        .collect();
    buckets.sort_by(|left, right| left.0.total_cmp(&right.0));
    buckets
}

/// One row: what it is, the period it reads at, the `(offset from the base, SampleCount)` buckets
/// the rule requires, and how many of the series' two observations that period reports.
type Row = (&'static str, i64, Vec<(f64, f64)>, f64);

/// The period each row reads at, and the buckets the rule requires: every period below a minute
/// answers from the high-resolution datapoint alone, in the window of its own second, and the
/// one-minute row folds both into the minute.
fn rows() -> Vec<Row> {
    let window = |period: f64, offset: f64| (offset / period).floor() * period;
    vec![
        (
            "one second, the finest granularity a read may ask for",
            1,
            vec![(window(1.0, HIGH_RESOLUTION_OFFSET), 1.0)],
            1.0,
        ),
        (
            "five seconds",
            5,
            vec![(window(5.0, HIGH_RESOLUTION_OFFSET), 1.0)],
            1.0,
        ),
        (
            "ten seconds",
            10,
            vec![(window(10.0, HIGH_RESOLUTION_OFFSET), 1.0)],
            1.0,
        ),
        (
            "twenty seconds",
            20,
            vec![(window(20.0, HIGH_RESOLUTION_OFFSET), 1.0)],
            1.0,
        ),
        (
            "thirty seconds",
            30,
            vec![(window(30.0, HIGH_RESOLUTION_OFFSET), 1.0)],
            1.0,
        ),
        (
            "one minute, where the two resolutions cannot differ",
            60,
            vec![(0.0, 2.0)],
            2.0,
        ),
        (
            "five minutes, a multiple of the minute",
            300,
            vec![(0.0, 2.0)],
            2.0,
        ),
    ]
}

/// RULE: a period finer than a datapoint's storage granularity answers with no datapoint for it,
/// and a period that granularity divides answers from it, bucketed by the span it is stored in.
#[tokio::test]
async fn a_read_answers_each_datapoint_only_at_the_granularities_it_is_stored_at() {
    let router = build_router();
    let base = base_minute();
    publish(&router, base).await;

    for (case, period, expected, reported) in rows() {
        for (reader, got) in [
            (
                "GetMetricStatistics",
                statistics_buckets(&router, base, period).await,
            ),
            (
                "GetMetricData",
                metric_data_buckets(&router, base, period).await,
            ),
        ] {
            assert_eq!(
                got, expected,
                "case {case:?}: expected {reader} at Period {period} to answer {expected:?}, got \
                 {got:?}"
            );
            // The observations the period reports: the whole pair where the period is one the
            // regular-resolution half is stored at, and the high-resolution one alone below it.
            let counted: f64 = got.iter().map(|(_, count)| *count).sum();
            assert_eq!(
                counted, reported,
                "case {case:?}: {reader} at Period {period} must report {reported} of the \
                 series' observations, got {got:?}"
            );
        }
    }
}

/// The REGULAR-resolution datapoint is answered only at the minute it is stored at, and there on
/// that whole minute, which is what says the read reports no detail the series does not hold —
/// asserted over the instants themselves rather than over a bucket list.
#[tokio::test]
async fn the_regular_resolution_datapoint_is_answered_only_at_the_minute_it_is_stored_at() {
    let router = build_router();
    let base = base_minute();
    // A series of its own, carrying only the regular-resolution datapoint, so every instant a
    // read reports is that datapoint's.
    ok(
        &router,
        "PutMetricData",
        &json!({
            "Namespace": NAMESPACE,
            "MetricData": [{
                "MetricName": "RegularOnly",
                "Dimensions": [{"Name": "Service", "Value": "api"}],
                "Value": 1.0,
                "Unit": "Count",
                "StorageResolution": 60,
                "Timestamp": base + REGULAR_RESOLUTION_OFFSET,
            }],
        }),
    )
    .await;

    for period in [1, 5, 10, 20, 30, 60] {
        let body = ok(
            &router,
            "GetMetricStatistics",
            &json!({
                "Namespace": NAMESPACE,
                "MetricName": "RegularOnly",
                "Dimensions": [{"Name": "Service", "Value": "api"}],
                "StartTime": base,
                "EndTime": base + MINUTE,
                "Period": period,
                "Statistics": ["SampleCount"],
            }),
        )
        .await;
        let stamps: Vec<f64> = body["Datapoints"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|point| point["Timestamp"].as_f64())
            .collect();
        // Only the minute the datapoint is stored at retrieves it: a finer period answers with
        // nothing, that period being none of the granularities the series holds.
        let expected = usize::from(period % 60 == 0);
        assert_eq!(
            stamps.len(),
            expected,
            "case {period}: the one regular-resolution datapoint must be reported {expected} \
             time(s) at Period {period}, got {body}"
        );
        for stamp in stamps {
            assert!(
                stamp.rem_euclid(MINUTE) == 0.0,
                "case {period}: a regular-resolution datapoint stands on a whole minute, got \
                 {} seconds past the minute",
                stamp.rem_euclid(MINUTE)
            );
        }
    }
}
