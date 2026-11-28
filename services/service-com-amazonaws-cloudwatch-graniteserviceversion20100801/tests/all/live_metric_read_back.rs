// SPDX-License-Identifier: CC0-1.0

//! Reading back a datapoint published at the CURRENT instant, end to end over the wire.
//!
//! ONE rule, asserted over a table: **a read is answered from every datapoint of its range whose
//! own storage resolution divides the period asked for, at the granularity that datapoint is
//! STORED at, straight away — no wait is needed for a datapoint to become readable, and a period
//! finer than the span a datapoint is stored in retrieves it not at all. "For metrics with regular
//! resolution, a period can be as short as one minute (60 seconds) and must be a multiple of 60.
//! For high-resolution metrics that are collected at intervals of less than one minute, the period
//! can be 1, 5, 10, 20, 30, 60, or any multiple of 60. High-resolution metrics are those metrics
//! stored by a `PutMetricData` call that includes a `StorageResolution` of 1 second."
//! (`smithy_query(query="doc",shape="GetMetricStatisticsInput",member="Period")`) — so a
//! regular-resolution datapoint ("CloudWatch stores at 1-minute resolution",
//! `MetricDatum$StorageResolution`) published now is read back now at the minute it is stored in,
//! reported at that minute, and is answered with NO datapoint at any of the sub-minute divisions a
//! read's windows are laid on, while its high-resolution twin is reported at the window of the
//! second it was published at, at every one of those periods.**
//!
//! What a period the series is not stored at answers with is nothing, not a refusal: "you must
//! specify the period as follows or no data points in that time range is returned" (the same
//! member) — both reads are 200, and only the storage class decides which answers with a
//! datapoint.
//!
//! The rows differ along what the rule generalises over: the period the read asks for (every
//! sub-minute value `MetricStat$Period` admits, plus the minute itself), and the storage
//! resolution the datapoint was published at (1, 60 and omitted — "if you omit this, the default
//! of 60 is used"). Both readers of a series are driven, because one fold answers for both, and
//! nothing here waits on the clock: every instant a row asserts is computed from the time stamp
//! the publish carried, so a slow host moves no row's expectation.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{build_router, now_secs, ok};

/// One minute, in the epoch seconds a published time stamp carries.
const MINUTE: f64 = 60.0;

/// The namespace every series of this suite is published under.
const NAMESPACE: &str = "MyApp/LiveReadBack";

/// The value every observation carries, so a datapoint that is answered is recognisable by its
/// `Sum` as well as by its instant.
const VALUE: f64 = 42.0;

/// The periods a read of a live datapoint asks for: the divisions of a minute a read's windows
/// are laid on — "If you set Period to 5, 10, 20, or 30, the start time of your request is
/// rounded down to the nearest time that corresponds to even 5-, 10-, 20-, or 30-second divisions
/// of a minute." (`GetMetricStatisticsInput$StartTime`) — the minute itself, the period a
/// regular-resolution series IS stored at, and the one period below those divisions, which
/// resolves the per-second detail only high-resolution storage keeps
/// (`MetricDatum$StorageResolution`).
const SERVED_PERIODS: [i64; 6] = [1, 5, 10, 20, 30, 60];

/// The one period of [`SERVED_PERIODS`] a regular-resolution series IS retrievable at: "a period
/// can be as short as one minute (60 seconds) and must be a multiple of 60"
/// (`GetMetricStatisticsInput$Period`).
const MINUTE_PERIOD: i64 = 60;

/// The two spellings of regular resolution, which "if you omit this, the default of 60 is used"
/// makes one class: `StorageResolution` 60, and the member left out.
const REGULAR_RESOLUTIONS: [(&str, Option<i64>); 2] = [
    ("StorageResolution 60", Some(60)),
    ("an omitted StorageResolution", None),
];

/// Publish one observation of `metric` at `timestamp`, at `resolution` seconds of storage
/// resolution (omitting the member where `resolution` is `None`).
async fn publish(
    router: &MultiServiceRouter,
    metric: &str,
    timestamp: f64,
    resolution: Option<i64>,
) {
    let mut datum = json!({
        "MetricName": metric,
        "Value": VALUE,
        "Unit": "Count",
        "Timestamp": timestamp,
    });
    if let Some(resolution) = resolution {
        datum["StorageResolution"] = json!(resolution);
    }
    ok(
        router,
        "PutMetricData",
        &json!({ "Namespace": NAMESPACE, "MetricData": [datum] }),
    )
    .await;
}

/// The `(Timestamp, Sum)` pairs `GetMetricStatistics` answers with for `metric` over
/// `[start, end)` at `period`, oldest first — the read the recorded client makes, with the empty
/// `Dimensions` list it sends.
async fn datapoints(
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
            "Dimensions": [],
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

/// The same read through `GetMetricData`, whose fold is the same one.
async fn metric_data(
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
                    "Metric": { "Namespace": NAMESPACE, "MetricName": metric },
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
    let mut points: Vec<(f64, f64)> = stamps.into_iter().zip(values).collect();
    points.sort_by(|left, right| left.0.total_cmp(&right.0));
    points
}

/// The start of the `period`-wide clock window `instant` falls in, which is the stamp a datapoint
/// folded out of that window carries.
fn window_of(instant: f64, period: f64) -> f64 {
    (instant / period).floor() * period
}

#[tokio::test]
async fn a_datapoint_published_now_is_read_back_now_at_every_served_period() {
    let router = build_router();

    for (case, resolution) in REGULAR_RESOLUTIONS {
        let metric = format!("Live{}", resolution.unwrap_or_default());
        // Published at THIS instant, as a client publishing live metrics does, and read over the
        // range such a client reads back — a couple of minutes either side of the publish.
        let published_at = now_secs();
        let minute = window_of(published_at, MINUTE);
        publish(&router, &metric, published_at, resolution).await;
        // The twin of the same instant published at high resolution, whose stored span is its own
        // second: the two differ only in where each is reported.
        let high_resolution = format!("{metric}HighRes");
        publish(&router, &high_resolution, published_at, Some(1)).await;
        let (start, end) = (published_at - 2.0 * MINUTE, published_at + 2.0 * MINUTE);

        for period in SERVED_PERIODS {
            // A regular-resolution series holds one aggregate per minute and is retrievable only
            // at the multiples of that minute: the minute period reports it at the minute it is
            // stored in, and every finer period is answered with no datapoint at all.
            let expected = if period % MINUTE_PERIOD == 0 {
                vec![(minute, VALUE)]
            } else {
                Vec::new()
            };
            for (reader, got) in [
                (
                    "GetMetricStatistics",
                    datapoints(&router, &metric, start, end, period).await,
                ),
                (
                    "GetMetricData",
                    metric_data(&router, &metric, start, end, period).await,
                ),
            ] {
                assert_eq!(
                    got, expected,
                    "case {case:?} at Period {period}: a datapoint published at {published_at} is \
                     stored in the minute {minute}, so {reader} must answer with {expected:?}, \
                     got {got:?}"
                );
            }
            // The high-resolution twin is stored at its own second, so it is reported at the
            // window of that second rather than at the minute.
            let second = window_of(published_at, 1.0);
            let expected = window_of(second, period as f64);
            let answered = datapoints(&router, &high_resolution, start, end, period).await;
            assert_eq!(
                answered,
                vec![(expected, VALUE)],
                "case {case:?} at Period {period}: the high-resolution twin of the same instant \
                 is stored at its own second, so it must be answered in the window of that \
                 second ({expected}), got {answered:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_series_a_live_datapoint_created_is_listed() {
    let router = build_router();
    let metric = "LiveListed";
    publish(&router, metric, now_secs(), None).await;

    let body = ok(&router, "ListMetrics", &json!({ "Namespace": NAMESPACE })).await;
    let listed: Vec<String> = body["Metrics"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|entry| entry["MetricName"].as_str().map(ToString::to_string))
        .collect();
    assert!(
        listed.iter().any(|name| name == metric),
        "case {metric:?}: the series a published datapoint created must be listed for its \
         namespace, got {listed:?}"
    );
}
