// SPDX-License-Identifier: CC0-1.0

//! WHICH datapoints a requested range selects, and which bucket each selected one is reported
//! under, end to end over the wire.
//!
//! ONE rule, asserted over a table: **a read selects every datapoint whose STORED SPAN reaches
//! into the requested range AND whose storage resolution divides the requested period — "The
//! value specified is inclusive; results include data points with the specified time stamp"
//! (`GetMetricStatisticsInput$StartTime`) against "The value specified is exclusive" (`$EndTime`)
//! — where a datapoint's stored span is the one its own `StorageResolution` aggregates it into:
//! its own second for a high-resolution datapoint ("Setting this to 1 specifies this metric as a
//! high-resolution metric, so that CloudWatch stores the metric with sub-minute resolution down to
//! one second") and the whole minute it was published in for a regular-resolution one ("Setting
//! this to 60 specifies this metric as regular resolution, which CloudWatch stores at 1-minute
//! resolution", `smithy_query(query="doc",shape="MetricDatum",member="StorageResolution")`). Each
//! selected datapoint is reported under the period window that span BEGINS in — so a
//! regular-resolution datapoint is reported under its whole minute even where that minute began
//! before the range did, that minute having no other instant to be reported at.**
//!
//! So the storage granularity decides what a range is judged against, which periods can retrieve
//! the datapoint at all and the bucket a selected one is reported at: a regular-resolution
//! datapoint stands for the minute it is stored in, not for the instant inside that minute it
//! happened to be published at, and a read asking for a period finer than that minute is answered
//! with no datapoint for it — "you must specify the period as follows or no data points in that
//! time range is returned" (`GetMetricStatisticsInput$Period`), the half of the rule the
//! `metric_storage_resolution` and `per_second_period_selection` suites table over.
//!
//! The rows differ along what the rule generalises over: where inside its minute the observation
//! is published (the first second, the middle, and the last second — the phase that decides
//! whether the minute it is stored in began before the range did), the resolution it is
//! published at (1, 60 and omitted, which is documented to read as 60), and the period the read
//! asks for (every sub-minute period `MetricStat$Period` admits, plus one minute). Both readers
//! of a series are driven, because one fold answers for both.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{build_router, ok};

/// One minute, in the epoch seconds a published time stamp carries.
const MINUTE: f64 = 60.0;

/// The namespace every series of this suite is published under.
const NAMESPACE: &str = "MyApp/RangeSelection";

/// The value every observation carries, so a selected datapoint's `Sum` is the published value
/// itself.
const VALUE: f64 = 42.0;

/// How far either side of the publish instant the read's range reaches, in seconds: the window a
/// client that has just published one observation reads it back over, and narrow enough that the
/// minute a regular-resolution observation is stored in begins before the range does for most
/// phases of a minute.
const HALF_WINDOW: f64 = 30.0;

/// The sub-minute `Period` values a read may ask for — "the period can be 1, 5, 10, 20, 30, 60,
/// or any multiple of 60" (`GetMetricStatisticsInput$Period`) — plus the minute itself. Every
/// one of them divides a minute, so a bucket boundary of either storage resolution is also a
/// boundary the documented start-time rounding puts a window on.
const PERIODS: [i64; 6] = [1, 5, 10, 20, 30, 60];

/// The offsets into a minute an observation is published at: its first second, two phases that
/// straddle the half-window, and its last second.
const PHASES: [f64; 5] = [0.0, 20.0, 31.0, 50.0, 59.0];

/// The `StorageResolution` values a publish carries: the two the member documents, and the
/// omitted member, which "if you do not specify it the default of 60 is used".
const RESOLUTIONS: [Option<i64>; 3] = [Some(1), Some(60), None];

/// A minute-aligned instant ten minutes in the past: recent enough to sit inside every retention
/// tier, and aligned so the phase a row publishes at is the whole of the observation's offset
/// inside its minute.
fn base_minute() -> f64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64();
    (now / MINUTE).floor() * MINUTE - 10.0 * MINUTE
}

/// Publish ONE observation of `metric` at `stamp`, at the row's resolution.
async fn publish(
    router: &MultiServiceRouter,
    metric: &str,
    resolution: Option<i64>,
    stamp: f64,
) {
    let mut datum = json!({
        "MetricName": metric,
        "Dimensions": [{"Name": "Service", "Value": "api"}],
        "Value": VALUE,
        "Unit": "Count",
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

/// The `(Timestamp, Sum)` datapoints `GetMetricStatistics` answers with over `[start, end)` at
/// `period`, oldest first.
async fn statistics_points(
    router: &MultiServiceRouter,
    metric: &str,
    period: i64,
    start: f64,
    end: f64,
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

/// The same datapoints as `GetMetricData` answers with, oldest first.
async fn metric_data_points(
    router: &MultiServiceRouter,
    metric: &str,
    period: i64,
    start: f64,
    end: f64,
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
    stamps.into_iter().zip(values).collect()
}

#[tokio::test]
async fn a_read_selects_by_the_publish_instant_and_reports_at_the_stored_bucket() {
    let router = build_router();
    let base = base_minute();

    for (index, (phase, (resolution, period))) in PHASES
        .iter()
        .flat_map(|phase| {
            RESOLUTIONS
                .iter()
                .flat_map(move |resolution| PERIODS.iter().map(move |period| (phase, (resolution, period))))
        })
        .enumerate()
    {
        let (phase, resolution, period) = (*phase, *resolution, *period);
        let case = format!(
            "published {phase} s into its minute at StorageResolution {resolution:?}, read at \
             Period {period}"
        );
        let metric = format!("Selected{index}");
        let stamp = base + phase;
        publish(&router, &metric, resolution, stamp).await;

        // The bucket the rule puts the observation in: the period window of its own publish
        // instant where the sub-minute detail is stored, and the whole minute it was published
        // in otherwise — and nothing at all where the period asked for is finer than the minute
        // that observation is stored at, no such period being one it can be retrieved at
        // (`crate::state::windows_by_unit`, and the `per_second_period_selection` suite). Every
        // period here divides a minute, so the two agree at one minute.
        let expected: Vec<(f64, f64)> = if resolution == Some(1) {
            #[allow(clippy::cast_precision_loss)]
            let span = period as f64;
            vec![((stamp / span).floor() * span, VALUE)]
        } else if period % 60 == 0 {
            vec![(base, VALUE)]
        } else {
            Vec::new()
        };

        for (reader, got) in [
            (
                "GetMetricStatistics",
                statistics_points(
                    &router,
                    &metric,
                    period,
                    stamp - HALF_WINDOW,
                    stamp + HALF_WINDOW,
                )
                .await,
            ),
            (
                "GetMetricData",
                metric_data_points(
                    &router,
                    &metric,
                    period,
                    stamp - HALF_WINDOW,
                    stamp + HALF_WINDOW,
                )
                .await,
            ),
        ] {
            assert_eq!(
                got, expected,
                "case {case:?}: expected {reader} to report the one published observation as \
                 {expected:?}, got {got:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_range_the_stored_span_does_not_reach_selects_nothing() {
    // The other side of the same rule, and the case that tells the stored SPAN from the publish
    // instant: a range that closes before a high-resolution observation was published selects
    // nothing, because the second it is stored in lies outside that range — while the same
    // range over a regular-resolution series selects the observation, because the MINUTE it is
    // stored in reaches into the range, and reports it under that minute at the periods that
    // minute can be retrieved at.
    let router = build_router();
    let base = base_minute();

    // (case, the StorageResolution published, whether the span it is stored as reaches the
    // range) — the span that reaches it holds the observation, and reports it under that
    // span's own start at every period its own resolution divides.
    let rows: [(&str, Option<i64>, bool); 3] = [
        ("high resolution", Some(1), false),
        ("regular resolution", Some(60), true),
        ("an omitted StorageResolution", None, true),
    ];

    for (index, (case, resolution, reaches)) in rows.into_iter().enumerate() {
        for period in PERIODS {
            let case = format!("{case} read at Period {period}");
            let metric = format!("Missed{index}x{period}");
            // Fifty seconds into the minute, read over the first thirty seconds of that same
            // minute: the minute the observation is stored in reaches into the range; the
            // second it is stored in does not.
            publish(&router, &metric, resolution, base + 50.0).await;
            let expected: Vec<(f64, f64)> = if reaches && period % 60 == 0 {
                vec![(base, VALUE)]
            } else {
                Vec::new()
            };
            for (reader, got) in [
                (
                    "GetMetricStatistics",
                    statistics_points(&router, &metric, period, base, base + 30.0).await,
                ),
                (
                    "GetMetricData",
                    metric_data_points(&router, &metric, period, base, base + 30.0).await,
                ),
            ] {
                assert_eq!(
                    got, expected,
                    "case {case:?}: expected {reader} to select {expected:?}, got {got:?}"
                );
            }
        }
    }
}
