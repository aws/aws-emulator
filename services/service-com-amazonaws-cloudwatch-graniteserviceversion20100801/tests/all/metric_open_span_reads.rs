// SPDX-License-Identifier: CC0-1.0

//! Reading a series whose datapoints are stored in a span the clock has NOT left yet, at the
//! period that span is stored at and at periods finer than it, end to end over the wire.
//!
//! ONE rule, asserted over a table: **whether the clock has left the span a datapoint is stored in
//! decides nothing about a read: the aggregate the store holds for that span is answered as it
//! stands at the period the span IS stored at, and the periods a datapoint can be retrieved at are
//! the multiples of its own storage resolution and no finer ones — "For metrics with regular
//! resolution, a period can be as short as one minute (60 seconds) and must be a multiple of 60.
//! For high-resolution metrics that are collected at intervals of less than one minute, the period
//! can be 1, 5, 10, 20, 30, 60, or any multiple of 60. High-resolution metrics are those metrics
//! stored by a `PutMetricData` call that includes a `StorageResolution` of 1 second."
//! (`smithy_query(query="doc",shape="GetMetricStatisticsInput",member="Period")`) with "you must
//! specify the period as follows or no data points in that time range is returned" (the same
//! member) against "Setting this to 60 specifies this metric as regular resolution, which
//! CloudWatch stores at 1-minute resolution" (`MetricDatum$StorageResolution`). So a
//! regular-resolution datapoint is answered at the minute it is stored in and at multiples of it,
//! whether or not the clock has left that minute, and a sub-minute read of it answers with no
//! datapoint at all — a 200 with an empty series, not a refusal.**
//!
//! What a sub-minute read answers with is therefore decided by the STORAGE CLASS and not by the
//! clock: the high-resolution twin of the very same instant, in the very same open minute, is
//! answered at every sub-minute period, in the window of the second it was published in. That twin
//! is the control on every row, so no row can pass by answering nothing at all.
//!
//! The rows differ along what the rule generalises over: the period the read asks for (every
//! sub-minute value `MetricStat$Period` admits, the minute itself and a multiple of it), the
//! storage resolution the datapoint was published at (1, 60 and omitted — "if you omit this, the
//! default of 60 is used"), and whether the span the datapoint is stored in is one the clock has
//! left.
//!
//! Nothing here waits on the clock and no row's outcome depends on how fast the host runs: every
//! instant a row asserts is computed from the time stamp its publish carried, and the open span
//! lies minutes AHEAD of this run (`PutMetricData` takes a time stamp "no more than two hours in
//! the future") while the closed one is a minute that passed well before it started.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{build_router, ok};

/// One minute, in the epoch seconds a published time stamp carries.
const MINUTE: f64 = 60.0;

/// The namespace every series of this suite is published under.
const NAMESPACE: &str = "MyApp/OpenSpan";

/// The value every observation carries, so a datapoint that is answered is recognisable by its
/// `Sum` as well as by its instant.
const VALUE: f64 = 42.0;

/// How far from this run's own minute the two spans lie: far enough that a host taking minutes
/// over this suite still reads the open span as open and the closed one as closed, and well
/// inside both the two-hour limit on a published time stamp and the three-hour tier a sub-minute
/// period is served from.
const SPAN_DISTANCE_MINUTES: f64 = 10.0;

/// Every period a read may ask for below a minute — the high-resolution set
/// `GetMetricStatisticsInput$Period` lists — each one finer than a regular-resolution
/// datapoint's own span, and so none of the granularities such a datapoint is retrievable at.
const SUB_MINUTE_PERIODS: [i64; 5] = [1, 5, 10, 20, 30];

/// The one sub-minute period below the divisions of a minute a read's own windows are laid on:
/// the period that resolves the per-second detail only high-resolution storage keeps.
const PER_SECOND_PERIOD: i64 = 1;

/// The periods a regular-resolution datapoint IS retrievable at: the minute it is stored at and a
/// multiple of it.
const MINUTE_OR_MORE_PERIODS: [i64; 2] = [60, 300];

/// The first of [`MINUTE_OR_MORE_PERIODS`]: the minute a regular-resolution datapoint is stored
/// at, which is also the window it is folded into when a read asks for exactly that period.
const MINUTE_PERIOD: i64 = 60;

/// The two spellings of regular resolution, which "if you omit this, the default of 60 is used"
/// makes one class: `StorageResolution` 60, and the member left out.
const REGULAR_RESOLUTIONS: [(&str, Option<i64>); 2] = [
    ("StorageResolution 60", Some(60)),
    ("an omitted StorageResolution", None),
];

/// The start of the minute this run is in.
fn this_minute() -> f64 {
    (epoch_now() / MINUTE).floor() * MINUTE
}

/// This instant, in the epoch seconds a published time stamp carries.
fn epoch_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// The start of the `period`-wide clock window `instant` falls in, which is the stamp a datapoint
/// folded out of that window carries.
fn window_of(instant: f64, period: f64) -> f64 {
    (instant / period).floor() * period
}

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
/// `[start, end)` at `period`, oldest first.
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

#[tokio::test]
async fn a_sub_minute_read_of_an_open_minute_answers_from_the_class_that_stores_it() {
    let router = build_router();
    // A minute the clock has NOT left, by a margin no host can run through.
    let minute = this_minute() + SPAN_DISTANCE_MINUTES * MINUTE;
    let stamped = minute + 30.0;
    // A range reaching a minute either side of that one, so the minute's own boundary is inside
    // it: what decides each row is the span the datapoint is stored in, not the range's ends.
    let (start, end) = (minute - MINUTE, minute + 2.0 * MINUTE);

    for (case, resolution) in REGULAR_RESOLUTIONS {
        let metric = format!("Open{}", resolution.unwrap_or_default());
        publish(&router, &metric, stamped, resolution).await;
        // The control published at the SAME instant into the SAME open minute, whose span is its
        // own second: the two differ only in the class each was stored at.
        let control = format!("{metric}HighRes");
        publish(&router, &control, stamped, Some(1)).await;

        // No period finer than the minute the series is stored at retrieves it, whether or not
        // the clock has left that minute: such a read is answered with an empty series.
        for period in SUB_MINUTE_PERIODS {
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
                assert!(
                    got.is_empty(),
                    "case {case:?} at Period {period}: a datapoint stored at 1-minute resolution \
                     is retrievable only at multiples of a minute, so {reader} must answer with \
                     no datapoint, got {got:?}"
                );
            }
            // The high-resolution twin stores the second it was published in, so that second's
            // own window is where it is reported — in the same open minute, at the same period.
            #[allow(clippy::cast_precision_loss)]
            let expected = window_of(window_of(stamped, 1.0), period as f64);
            for (reader, got) in [
                (
                    "GetMetricStatistics",
                    datapoints(&router, &control, start, end, period).await,
                ),
                (
                    "GetMetricData",
                    metric_data(&router, &control, start, end, period).await,
                ),
            ] {
                assert_eq!(
                    got,
                    vec![(expected, VALUE)],
                    "case {case:?} at Period {period}: the high-resolution twin of the same \
                     instant is stored at its own second, so {reader} must answer with it in the \
                     window of that second ({expected}), got {got:?}"
                );
            }
        }

        // The other half of the rule: at the span's OWN granularity, and at any coarser period,
        // the window being folded is that span or covers it, so the datapoint of a minute still
        // running is answered from as it stands.
        for period in MINUTE_OR_MORE_PERIODS {
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
                    got.len(),
                    1,
                    "case {case:?} at Period {period}: the datapoint of a running minute is read \
                     at the span it is stored at, so {reader} must answer with it, got {got:?}"
                );
                let (at, sum) = got[0];
                assert!(
                    (sum - VALUE).abs() < f64::EPSILON,
                    "case {case:?} at Period {period}: expected {reader} to answer the published \
                     value {VALUE}, got {sum}"
                );
                // At the minute the datapoint is stored at, the window it is folded into IS
                // that minute, so that is the instant it is reported under. Which window a
                // COARSER period folds it into is the business of the read's own window grid
                // (`metric_range_selection`), not of this rule.
                if period == MINUTE_PERIOD {
                    assert!(
                        (at - minute).abs() < f64::EPSILON,
                        "case {case:?} at Period {period}: expected {reader} to report the \
                         datapoint under the minute it is stored at ({minute}), got {at}"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn a_sub_minute_read_of_a_closed_minute_answers_from_the_class_that_stores_it() {
    let router = build_router();
    // A minute that closed well before this run, and recent enough to sit inside the tier a
    // sub-minute period is served from ("Data points with a period of less than 60 seconds are
    // available for 3 hours").
    let closed = this_minute() - SPAN_DISTANCE_MINUTES * MINUTE;
    let stamped = closed + 30.0;
    let (start, end) = (closed - MINUTE, closed + 2.0 * MINUTE);

    for (case, resolution) in REGULAR_RESOLUTIONS {
        let metric = format!("Closed{}", resolution.unwrap_or_default());
        publish(&router, &metric, stamped, resolution).await;

        // A closed minute's aggregate is complete, and still retrievable only at the multiples
        // of the minute it is stored at: so the sub-minute reads answer with nothing and the
        // minute's own period answers with that whole aggregate.
        for period in SUB_MINUTE_PERIODS {
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
                assert!(
                    got.is_empty(),
                    "case {case:?} at Period {period}: a closed minute's aggregate is stored at \
                     1-minute resolution, so a finer period retrieves nothing and {reader} must \
                     answer with no datapoint, got {got:?}"
                );
            }
        }
        for (reader, got) in [
            (
                "GetMetricStatistics",
                datapoints(&router, &metric, start, end, MINUTE_PERIOD).await,
            ),
            (
                "GetMetricData",
                metric_data(&router, &metric, start, end, MINUTE_PERIOD).await,
            ),
        ] {
            assert_eq!(
                got,
                vec![(closed, VALUE)],
                "case {case:?} at Period {MINUTE_PERIOD}: the aggregate of the closed minute is \
                 answered at that minute's own boundary, so {reader} must report it, got {got:?}"
            );
        }
    }
}

/// The RECORDED shape of the one-second half of the rule: a datum published at the CURRENT
/// instant, read at the per-second period straight afterwards.
///
/// The minute the publish falls in is the minute now running, and a regular-resolution datum of it
/// is not retrievable at a period finer than that minute, so the read is answered with an empty
/// series while the minute's own period answers with the aggregate it holds. The high-resolution
/// twin of the same instant is the control that keeps the row from passing vacuously. Which
/// instant each expectation names comes from the time stamp the publish carried, so nothing here
/// depends on how fast the host runs or on whether the minute closes between the publish and the
/// read.
#[tokio::test]
async fn a_one_second_read_of_the_running_minute_answers_only_the_high_resolution_class() {
    let router = build_router();

    for (case, resolution) in REGULAR_RESOLUTIONS {
        let metric = format!("Running{}", resolution.unwrap_or_default());
        // Published at THIS instant, as a client publishing live metrics does, and read over a
        // range reaching a minute either side of the minute it falls in.
        let published_at = epoch_now();
        let minute = window_of(published_at, MINUTE);
        publish(&router, &metric, published_at, resolution).await;
        let control = format!("{metric}HighRes");
        publish(&router, &control, published_at, Some(1)).await;
        let (start, end) = (minute - MINUTE, minute + 2.0 * MINUTE);

        for (reader, got) in [
            (
                "GetMetricStatistics",
                datapoints(&router, &metric, start, end, PER_SECOND_PERIOD).await,
            ),
            (
                "GetMetricData",
                metric_data(&router, &metric, start, end, PER_SECOND_PERIOD).await,
            ),
        ] {
            assert!(
                got.is_empty(),
                "case {case:?} at Period {PER_SECOND_PERIOD}: the minute {minute} holds its \
                 aggregate at 1-minute resolution, which no per-second read retrieves, so \
                 {reader} must answer with no datapoint, got {got:?}"
            );
        }
        // The same datum IS answered at the minute it is stored at, so what the per-second read
        // reports nothing about is the granularity and not the series.
        let at_the_minute = datapoints(&router, &metric, start, end, MINUTE_PERIOD).await;
        assert_eq!(
            at_the_minute,
            vec![(minute, VALUE)],
            "case {case:?}: the datum of the running minute is answered at Period \
             {MINUTE_PERIOD}, got {at_the_minute:?}"
        );

        // The high-resolution twin stores exactly the detail that period asks for, so it is
        // reported at the second it was published in.
        let second = window_of(published_at, 1.0);
        for (reader, got) in [
            (
                "GetMetricStatistics",
                datapoints(&router, &control, start, end, PER_SECOND_PERIOD).await,
            ),
            (
                "GetMetricData",
                metric_data(&router, &control, start, end, PER_SECOND_PERIOD).await,
            ),
        ] {
            assert_eq!(
                got,
                vec![(second, VALUE)],
                "case {case:?} at Period {PER_SECOND_PERIOD}: the high-resolution twin stores the \
                 second it was published in, so {reader} must answer with it at that second \
                 ({second}), got {got:?}"
            );
        }
    }
}
