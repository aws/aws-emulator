// SPDX-License-Identifier: CC0-1.0

//! WHICH data a one-second `Period` selects, and where each of them is reported, end to end over
//! the wire.
//!
//! ONE rule, asserted over a table: **a read is answered from every datum of its range whose own
//! storage class DIVIDES the period asked for, each at the span that class stores it in — "Setting
//! this to 1 specifies this metric as a high-resolution metric, so that CloudWatch stores the
//! metric with sub-minute resolution down to one second. Setting this to 60 specifies this metric
//! as a regular-resolution metric, which CloudWatch stores at 1-minute resolution… This field is
//! optional, if you do not specify it the default of 60 is used."
//! (`smithy_query(query="doc",shape="MetricDatum",member="StorageResolution")`) against "For
//! metrics with regular resolution, a period can be as short as one minute (60 seconds) and must be
//! a multiple of 60. For high-resolution metrics that are collected at intervals of less than one
//! minute, the period can be 1, 5, 10, 20, 30, 60, or any multiple of 60."
//! (`smithy_query(query="doc",shape="GetMetricStatisticsInput",member="Period")`). A read at the
//! one-second period resolves per-second detail, which only high-resolution storage keeps, so that
//! class is reported at the second it was published in while a regular-resolution datum is not
//! answered at all — "you must specify the period as follows or no data points in that time range
//! is returned" (the same member). Neither read is an error: the sub-minute read of a
//! regular-resolution datum is a 200 carrying an empty series.**
//!
//! So the storage class a datum was published with decides WHICH periods answer from it, and it is
//! a property of the DATUM rather than of the series or of the call: two data published by one
//! `PutMetricData` are answered differently by one read, which is the recorded shape this suite
//! opens with.
//!
//! The rows differ along what the rule generalises over: the storage class published (1, 60, and
//! the omitted member that is documented to read as 60), the period the read asks for (every
//! sub-minute value `MetricStat$Period` admits, the minute itself and a multiple of it), and where
//! the publish instant sits relative to the clock (this instant, a minute that closed well before
//! this run, and a minute still ahead of it — `PutMetricData` takes a time stamp "as much as 2
//! hours after the current day and time"). Both readers of a series are driven, because one fold
//! answers for both.
//!
//! Nothing here waits on the clock and no row's outcome depends on how fast the host runs: every
//! expectation follows from the time stamp the publish carried.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{build_router, now_secs, ok};

/// One minute, in the epoch seconds a published time stamp carries.
const MINUTE: f64 = 60.0;

/// The namespace every datum of this suite is published under.
const NAMESPACE: &str = "MyApp/PerSecondPeriod";

/// The value every observation carries, so a datum that IS answered is recognisable by its `Sum`.
const VALUE: f64 = 42.0;

/// How far either side of the publish instant every read's range reaches, in seconds: the window a
/// client that has just published one observation reads it back over.
const HALF_WINDOW: f64 = 30.0;

/// How far from this run's own minute the two displaced instants lie, in minutes: far enough that
/// a host taking minutes over this suite still reads the one as past and the other as ahead, and
/// well inside both the two-hour limit on a published time stamp and the three-hour tier a
/// sub-minute period is served from.
const DISPLACEMENT_MINUTES: f64 = 10.0;

/// The storage class a datum is published with, and whether it is the HIGH-resolution one: the two
/// values `MetricDatum$StorageResolution` documents, plus the omitted member — "if you do not
/// specify it the default of 60 is used".
const PUBLISHED_CLASSES: [(&str, Option<i64>, bool); 3] = [
    ("StorageResolution 1", Some(1), true),
    ("StorageResolution 60", Some(60), false),
    ("an omitted StorageResolution", None, false),
];

/// The periods a read asks for, each asserted down to the instant the datum is reported at: every
/// sub-minute value `MetricStat$Period` admits ("the period can be 1, 5, 10, 20, 30, 60, or any
/// multiple of 60") and the minute itself. Each divides a minute, so the window grid of each is
/// laid on the boundaries both storage classes store their data at.
const PERIODS: [i64; 6] = [1, 5, 10, 20, 30, 60];

/// A period COARSER than the minute, where what is asserted is that the datum is answered at all:
/// which window a period wider than the range the caller named folds it into is the business of
/// each reader's own window grid (`metric_range_selection`), not of this rule.
const COARSER_PERIOD: i64 = 300;

/// The one period that resolves the per-second detail only high-resolution storage keeps.
const PER_SECOND_PERIOD: i64 = 1;

/// Publish one datum per storage class, all at `timestamp` and all in ONE `PutMetricData` call —
/// the recorded shape — each under a metric name of its own so a read answers from one of them.
async fn publish_one_of_each_class(router: &MultiServiceRouter, prefix: &str, timestamp: f64) {
    let data: Vec<Value> = PUBLISHED_CLASSES
        .iter()
        .map(|(_, resolution, _)| {
            let mut datum = json!({
                "MetricName": metric_name(prefix, *resolution),
                "Value": VALUE,
                "Unit": "Count",
                "Timestamp": timestamp,
            });
            if let Some(resolution) = resolution {
                datum["StorageResolution"] = json!(resolution);
            }
            datum
        })
        .collect();
    ok(
        router,
        "PutMetricData",
        &json!({"Namespace": NAMESPACE, "MetricData": data}),
    )
    .await;
}

/// The metric name the datum of `resolution` is published under, inside the series group `prefix`.
fn metric_name(prefix: &str, resolution: Option<i64>) -> String {
    format!("{prefix}Res{}", resolution.unwrap_or(0))
}

/// The start of the `span`-wide clock window `instant` falls in.
fn window_of(instant: f64, span: f64) -> f64 {
    (instant / span).floor() * span
}

/// The `(Timestamp, Sum)` pairs `GetMetricStatistics` answers with for `metric` over
/// `[start, end)` at `period`, oldest datapoint first.
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
                    "Metric": {"Namespace": NAMESPACE, "MetricName": metric},
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

/// RULE: a period answers from the storage classes it is a multiple of, each datum at the span its
/// own class stores it in — the second for high resolution, the whole minute for regular
/// resolution, and nothing at all for a regular-resolution datum read below the minute.
#[tokio::test]
async fn a_per_second_read_answers_from_either_class_at_the_span_it_stores_it_in() {
    let router = build_router();
    let this_minute = window_of(now_secs(), MINUTE);

    // (case, the instant the data is published at) — this instant, the minute that closed well
    // before this run, and a minute still ahead of it.
    let instants: [(&str, f64); 3] = [
        ("published at this instant", now_secs()),
        (
            "published in a minute that has closed",
            this_minute - DISPLACEMENT_MINUTES * MINUTE + 30.0,
        ),
        (
            "published in a minute still ahead",
            this_minute + DISPLACEMENT_MINUTES * MINUTE + 30.0,
        ),
    ];

    for (index, (phase, stamped)) in instants.into_iter().enumerate() {
        let prefix = format!("Twin{index}");
        publish_one_of_each_class(&router, &prefix, stamped).await;
        let (start, end) = (stamped - HALF_WINDOW, stamped + HALF_WINDOW);

        for (class, resolution, high_resolution) in PUBLISHED_CLASSES {
            let metric = metric_name(&prefix, resolution);
            // The instant the datum is STORED at, which decides the window every read reports it
            // under: its own second where that detail is kept, and the whole minute where it is
            // not.
            let stored_at = if high_resolution {
                window_of(stamped, 1.0)
            } else {
                window_of(stamped, MINUTE)
            };
            for period in PERIODS {
                // A period the datum's own storage class divides is answered from it, in the
                // window of that period the span it is stored in begins in; a finer period is
                // answered with no datapoint at all.
                #[allow(clippy::cast_precision_loss)]
                let expected = if high_resolution || period % 60 == 0 {
                    vec![(window_of(stored_at, period as f64), VALUE)]
                } else {
                    Vec::new()
                };
                let case = format!("{class} {phase}, read at Period {period}");
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
            // A period wider than the whole range is answered from the datum too, with the one
            // observation's value as its `Sum`.
            let case = format!("{class} {phase}, read at Period {COARSER_PERIOD}");
            for (reader, got) in [
                (
                    "GetMetricStatistics",
                    statistics_points(&router, &metric, start, end, COARSER_PERIOD).await,
                ),
                (
                    "GetMetricData",
                    metric_data_points(&router, &metric, start, end, COARSER_PERIOD).await,
                ),
            ] {
                let sums: Vec<f64> = got.iter().map(|(_, sum)| *sum).collect();
                assert_eq!(
                    sums,
                    vec![VALUE],
                    "case {case:?}: expected {reader} to answer the one observation's value, got \
                     {got:?}"
                );
            }
        }
    }
}

/// The two data of ONE `PutMetricData` call are told apart by the storage class each carried, not
/// by the call: the recorded shape, read at the one period that tells them apart and at the period
/// a regular-resolution series is stored at.
#[tokio::test]
async fn one_call_can_publish_data_of_both_classes_and_a_per_second_read_sees_only_the_high_one() {
    let router = build_router();
    let stamped = now_secs();
    let prefix = "Pair";
    publish_one_of_each_class(&router, prefix, stamped).await;
    let (start, end) = (stamped - HALF_WINDOW, stamped + HALF_WINDOW);

    let high = metric_name(prefix, Some(1));
    let regular = metric_name(prefix, Some(60));
    let defaulted = metric_name(prefix, None);

    // At the per-second period the high-resolution datum of that call is reported at the second it
    // was published in, and the regular-resolution ones not at all — the one read tells the two
    // classes apart, and the minute period below shows the series the empty answer belongs to does
    // hold the datum.
    assert_eq!(
        statistics_points(&router, &high, start, end, PER_SECOND_PERIOD).await,
        vec![(window_of(stamped, 1.0), VALUE)],
        "case {high:?}: the high-resolution datum of the call is reported at the second it was \
         published in"
    );
    for metric in [&regular, &defaulted] {
        let got = statistics_points(&router, metric, start, end, PER_SECOND_PERIOD).await;
        assert!(
            got.is_empty(),
            "case {metric:?}: a regular-resolution datum of the same call keeps no per-second \
             detail, so a per-second read is answered with no datapoint at all, got {got:?}"
        );
    }

    // At the minute a regular-resolution datum IS stored at, every datum of the call answers under
    // that same minute.
    for metric in [&high, &regular, &defaulted] {
        let got = statistics_points(&router, metric, start, end, 60).await;
        assert_eq!(
            got,
            vec![(window_of(stamped, MINUTE), VALUE)],
            "case {metric:?}: every datum of the call is answered at the minute, got {got:?}"
        );
    }
}
