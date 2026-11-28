// SPDX-License-Identifier: CC0-1.0

//! WHICH `Period` values retrieve a datum, decided by the `StorageResolution` it was published
//! with, end to end over the wire.
//!
//! ONE rule, asserted over a table: **the granularities a stored datum can be RETRIEVED at are the
//! multiples of the storage resolution it was published with, and a read at any finer granularity
//! answers with no datapoint for it — "Valid values are 1 and 60. Setting this to 1 specifies this
//! metric as a high-resolution metric, so that CloudWatch stores the metric with sub-minute
//! resolution down to one second. Setting this to 60 specifies this metric as a regular-resolution
//! metric, which CloudWatch stores at 1-minute resolution. … This field is optional, if you do not
//! specify it the default of 60 is used."
//! (`smithy_query(query="doc",shape="MetricDatum",member="StorageResolution")`) against "For
//! metrics with regular resolution, a period can be as short as one minute (60 seconds) and must be
//! a multiple of 60. For high-resolution metrics that are collected at intervals of less than one
//! minute, the period can be 1, 5, 10, 20, 30, 60, or any multiple of 60. High-resolution metrics
//! are those metrics stored by a `PutMetricData` call that includes a `StorageResolution` of 1
//! second."
//! (`smithy_query(query="doc",shape="GetMetricStatisticsInput",member="Period")`). Asking for a
//! granularity the datum is not stored at is not an error: the same member says "you must specify
//! the period as follows or no data points in that time range is returned", so such a read is a
//! 200 carrying an empty series.**
//!
//! That is what makes the two storage classes distinguishable to a client: a regular-resolution
//! datum answers only the multiples of a minute, a high-resolution one answers every period a read
//! may ask for, and a datum published with neither member answers like the default of 60 it reads
//! as.
//!
//! The rows differ along what the rule generalises over: how the datum declares its class
//! (`StorageResolution` 1, `StorageResolution` 60, and the omitted member), the period the read
//! asks for (every sub-minute value the member above enumerates, the minute itself, and multiples
//! of it up to the hour), where inside its minute the datum is stamped, and which reader asks —
//! `GetMetricStatistics` and `GetMetricData` answer from one fold, so the rule holds for both.
//!
//! Nothing here waits on the clock: every instant asserted is computed from the time stamp the
//! publish carried, so no row's outcome depends on how fast the host runs.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{build_router, now_secs, ok};

/// One minute, in the epoch seconds a published time stamp carries.
const MINUTE: f64 = 60.0;

/// The namespace every series of this suite is published under.
const NAMESPACE: &str = "MyApp/ReadGranularity";

/// The value every observation carries, so a datum that IS retrieved is recognisable by its `Sum`.
const VALUE: f64 = 7.5;

/// The unit every observation is published with, which every datapoint retrieved must report.
const UNIT: &str = "Count";

/// The `StorageResolution` a row publishes with — the two values the member documents ("Valid
/// values are 1 and 60") and the omitted member, which reads as the documented default of 60 — and
/// the resolution in seconds each of them means.
const CLASSES: [(&str, Option<i64>, i64); 3] = [
    ("StorageResolution 1", Some(1), 1),
    ("StorageResolution 60", Some(60), 60),
    ("an omitted StorageResolution", None, 60),
];

/// Every `Period` a read may ask for, from the set `GetMetricStatisticsInput$Period` enumerates:
/// the five sub-minute granularities ("the period can be 1, 5, 10, 20, 30, 60, or any multiple of
/// 60"), the minute itself, and multiples of it up to the hour.
const PERIODS: [i64; 9] = [1, 5, 10, 20, 30, 60, 120, 300, 3_600];

/// Where inside its minute a row stamps its datum: the minute's first second, a second that is a
/// multiple of none of the sub-minute granularities but the finest, and its last second — so a row
/// cannot pass by assuming the publish instant is a window boundary.
const OFFSETS: [f64; 3] = [0.0, 37.0, 59.0];

/// How far back the data is stamped, in minutes: behind the clock, and well inside the three-hour
/// tier a sub-minute period is served from ("Data points with a period of less than 60 seconds are
/// available for 3 hours", `GetMetricStatisticsInput$Period`).
const DISPLACEMENT_MINUTES: f64 = 10.0;

/// The start of the `span`-wide clock window `instant` falls in.
fn window_of(instant: f64, span: f64) -> f64 {
    (instant / span).floor() * span
}

/// The minute-aligned instant every read's range opens at, ten minutes behind the clock.
///
/// Minute-aligned, so the requested start is its own rounded start at every period a row reads at
/// — "Start time less than 15 days ago - Round down to the nearest whole minute" and "If you set
/// Period to 5, 10, 20, or 30, the start time of your request is rounded down to the nearest time
/// that corresponds to even 5-, 10-, 20-, or 30-second divisions of a minute"
/// (`GetMetricStatisticsInput$StartTime`) — and the window grid of each therefore runs in whole
/// periods from it.
fn base_minute() -> f64 {
    window_of(now_secs(), MINUTE) - DISPLACEMENT_MINUTES * MINUTE
}

/// Publish ONE observation of `metric` at `stamp`, declaring `resolution` where a row names one.
async fn publish(
    router: &MultiServiceRouter,
    metric: &str,
    stamp: f64,
    resolution: Option<i64>,
) {
    let mut datum = json!({
        "MetricName": metric,
        "Dimensions": [{"Name": "Service", "Value": "api"}],
        "Value": VALUE,
        "Unit": UNIT,
        "Timestamp": stamp,
    });
    if let Some(resolution) = resolution {
        datum["StorageResolution"] = json!(resolution);
    }
    ok(
        router,
        "PutMetricData",
        &json!({"Namespace": NAMESPACE, "MetricData": [datum]}),
    )
    .await;
}

/// The `(Timestamp, Sum)` pairs `GetMetricStatistics` answers for `metric` over `[start, end)` at
/// `period`, oldest first.
async fn statistics_points(
    router: &MultiServiceRouter,
    metric: &str,
    start: f64,
    end: f64,
    period: i64,
) -> Vec<(f64, f64)> {
    let body = ok(
        router,
        "GetMetricStatistics",
        &json!({
            "Namespace": NAMESPACE,
            "MetricName": metric,
            "Dimensions": [{"Name": "Service", "Value": "api"}],
            "StartTime": start,
            "EndTime": end,
            "Period": period,
            "Statistics": ["Sum"],
        }),
    )
    .await;
    let mut points: Vec<(f64, f64)> = body["Datapoints"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|point| {
            (
                point["Timestamp"].as_f64().unwrap_or_default(),
                point["Sum"].as_f64().unwrap_or_default(),
            )
        })
        .collect();
    points.sort_by(|left, right| left.0.total_cmp(&right.0));
    points
}

/// The same read through `GetMetricData`, whose window fold is the same one.
async fn metric_data_points(
    router: &MultiServiceRouter,
    metric: &str,
    start: f64,
    end: f64,
    period: i64,
) -> Vec<(f64, f64)> {
    let body = ok(
        router,
        "GetMetricData",
        &json!({
            "MetricDataQueries": [{
                "Id": "m1",
                "MetricStat": {
                    "Metric": {
                        "Namespace": NAMESPACE,
                        "MetricName": metric,
                        "Dimensions": [{"Name": "Service", "Value": "api"}],
                    },
                    "Period": period,
                    "Stat": "Sum",
                },
            }],
            "StartTime": start,
            "EndTime": end,
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
    let mut points: Vec<(f64, f64)> = stamps.into_iter().zip(values).collect();
    points.sort_by(|left, right| left.0.total_cmp(&right.0));
    points
}

/// RULE: a datum is retrieved by exactly the periods its own storage resolution divides, at the
/// window of that period the span it is stored in begins in, and by no finer period at all.
#[tokio::test]
async fn a_datum_is_retrieved_only_at_the_periods_its_storage_resolution_divides() {
    let router = build_router();
    let base = base_minute();
    // A range of two whole minutes from the aligned base: wide enough that every row's publish is
    // inside it at either storage class, and its own rounded start at every period read.
    let (start, end) = (base, base + 2.0 * MINUTE);

    for (class, declared, resolution) in CLASSES {
        for offset in OFFSETS {
            let stamp = base + offset;
            // The span the datum is stored in begins here: its own second at one-second
            // resolution, the whole minute at one-minute resolution.
            #[allow(clippy::cast_precision_loss)]
            let stored_at = window_of(stamp, resolution as f64);
            for period in PERIODS {
                let case =
                    format!("{class} stamped {offset} s into its minute, read at Period {period}");
                // Retrieved when the period is a multiple of the resolution the datum is stored
                // at, and then at the window of that period its stored span begins in — the grid
                // runs in whole periods from the rounded start, which is the base.
                #[allow(clippy::cast_precision_loss)]
                let span = period as f64;
                let expected: Vec<(f64, f64)> = if period % resolution == 0 {
                    vec![(base + window_of(stored_at - base, span), VALUE)]
                } else {
                    Vec::new()
                };
                // One series per row, so each row asserts what its own publish did.
                let metric = format!("Gran{}x{offset}x{period}", declared.unwrap_or_default());
                publish(&router, &metric, stamp, declared).await;

                for (reader, got) in [
                    (
                        "GetMetricStatistics",
                        statistics_points(&router, &metric, start, end, period).await,
                    ),
                    (
                        "GetMetricData",
                        metric_data_points(&router, &metric, start, end, period).await,
                    ),
                ] {
                    assert_eq!(
                        got, expected,
                        "case {case:?}: expected {reader} to answer {expected:?}, got {got:?}"
                    );
                }
            }
        }
    }
}

/// The two classes are told apart inside ONE `PutMetricData` call: two data of the same instant
/// and the same value, one published at one-second resolution and one with no `StorageResolution`
/// member at all, are retrieved differently by one sub-minute read — and the regular-resolution
/// one is still retrieved at the minute it is stored at, so what the empty answer reports is the
/// granularity and not an empty series.
#[tokio::test]
async fn one_call_s_two_classes_are_retrieved_by_different_periods() {
    let router = build_router();
    let stamped = base_minute() + 30.0;
    let high = "TwinHighRes";
    let regular = "TwinStdRes";

    ok(
        &router,
        "PutMetricData",
        &json!({
            "Namespace": NAMESPACE,
            "MetricData": [
                {
                    "MetricName": high,
                    "Dimensions": [{"Name": "Service", "Value": "api"}],
                    "Value": VALUE,
                    "Unit": UNIT,
                    "StorageResolution": 1,
                    "Timestamp": stamped,
                },
                {
                    "MetricName": regular,
                    "Dimensions": [{"Name": "Service", "Value": "api"}],
                    "Value": VALUE,
                    "Unit": UNIT,
                    "Timestamp": stamped,
                },
            ],
        }),
    )
    .await;

    // The sub-minute read: half a minute either side of the publish, at the one period that
    // resolves per-second detail.
    let (narrow_start, narrow_end) = (stamped - 30.0, stamped + 30.0);
    let answered = statistics_points(&router, high, narrow_start, narrow_end, 1).await;
    assert_eq!(
        answered.len(),
        1,
        "case {high:?}: a datum stored at one-second resolution is retrieved by a one-second \
         period, got {answered:?}"
    );
    assert_eq!(
        answered[0].1, VALUE,
        "case {high:?}: expected the published value {VALUE} as the window's Sum, got \
         {answered:?}"
    );
    let withheld = statistics_points(&router, regular, narrow_start, narrow_end, 1).await;
    assert!(
        withheld.is_empty(),
        "case {regular:?}: a datum stored at one-minute resolution is retrieved by no sub-minute \
         period, so the read must answer with no datapoint, got {withheld:?}"
    );

    // The same datum, at the granularity it IS stored at: one datapoint carrying the published
    // value, on the minute it is stored in.
    let (wide_start, wide_end) = (stamped - 90.0, stamped + 90.0);
    let at_the_minute = statistics_points(&router, regular, wide_start, wide_end, 60).await;
    assert_eq!(
        at_the_minute,
        vec![(window_of(stamped, MINUTE), VALUE)],
        "case {regular:?}: the datum is retrieved at the minute it is stored at, got \
         {at_the_minute:?}"
    );

    // Reading at a granularity the series is not stored at changes nothing a later caller sees:
    // the minute's own read answers the same aggregate after it.
    let read_again = statistics_points(&router, regular, wide_start, wide_end, 60).await;
    assert_eq!(
        read_again, at_the_minute,
        "case {regular:?}: a read is a report, so the series must answer the same after a \
         sub-minute read of it, got {read_again:?}"
    );
}
