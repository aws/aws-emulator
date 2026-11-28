// SPDX-License-Identifier: CC0-1.0

//! What a SUB-MINUTE read answers with over a series stored at a coarser granularity, end to end
//! over the wire.
//!
//! ONE rule, asserted over a table: **a read is answered from every datapoint its range holds
//! whose own storage resolution divides the period it asked for, at the granularity that datapoint
//! is STORED at, and from no datapoint stored coarser than that period — "For metrics with
//! regular resolution, a period can be as short as one minute (60 seconds) and must be a multiple
//! of 60. For high-resolution metrics that are collected at intervals of less than one minute, the
//! period can be 1, 5, 10, 20, 30, 60, or any multiple of 60."
//! (`smithy_query(query="doc",shape="GetMetricStatisticsInput",member="Period")`) with "Setting
//! this to 60 specifies this metric as regular resolution, which CloudWatch stores at 1-minute
//! resolution" (`MetricDatum$StorageResolution`). A series stored at 1-minute resolution holds one
//! aggregate per whole minute and no sub-minute detail at all, so a read of it at ANY period finer
//! than a minute answers with an empty series — "you must specify the period as follows or no data
//! points in that time range is returned" (the same member) — while the same observations
//! published at one-second resolution answer with the sub-minute buckets that series does hold.**
//!
//! The INVARIANT each row asserts is that: the observations a read answers over are exactly those
//! the period RETRIEVES — for a high-resolution series the `SampleCount` and `Sum` summed across
//! the datapoints of a sub-minute read equal those of the same range read at the minute, and every
//! instant answered lies inside the minute the observations were published in; for a
//! regular-resolution series, which holds one aggregate for the whole minute, a sub-minute read
//! answers with nothing while the minute's own period answers with that one aggregate.
//!
//! The rows differ along what the rule generalises over: the storage resolution published (1, 60
//! and the omitted member, which "if you omit this, the default of 60 is used"), how many
//! observations fall in the minute (one, three and twelve, the shape an Embedded Metric Format
//! producer publishes a minute of requests as), and where the minute sits relative to the clock
//! the read is served at (one that closed before the run, the one the run is in, and one the clock
//! has not reached — `PutMetricData` takes a time stamp as much as two hours ahead). Both readers
//! of a series are driven, because one window fold answers for both, and every instant asserted is
//! computed from the time stamps the publish carried, so no row's outcome depends on how fast the
//! host runs.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{build_router, now_secs, ok};

/// One minute, in the epoch seconds a published time stamp carries.
const MINUTE: f64 = 60.0;

/// The namespace every series of this suite is published under.
const NAMESPACE: &str = "MyApp/SubMinuteReads";

/// The unit every observation is published with, which every datapoint answered must report.
const UNIT: &str = "Count";

/// Every period below a minute a read may ask for — the set
/// `GetMetricStatisticsInput$Period` lists — each one finer than the minute a
/// regular-resolution series is stored at.
const SUB_MINUTE_PERIODS: [i64; 5] = [1, 5, 10, 20, 30];

/// The period a regular-resolution series IS stored at, which every sub-minute read is compared
/// against.
const MINUTE_PERIOD: i64 = 60;

/// The statistics every read asks for, so a row sees the whole aggregate of each window it is
/// answered with.
const STATISTICS: [&str; 4] = ["SampleCount", "Sum", "Minimum", "Maximum"];

/// How many observations a row publishes inside its minute: one, a few, and the twelve a
/// per-second publisher leaves in one minute.
const COUNTS: [usize; 3] = [1, 3, 12];

/// The `StorageResolution` values a row publishes with: the two the member documents, and the
/// omitted member.
const RESOLUTIONS: [(&str, Option<i64>); 3] = [
    ("StorageResolution 60", Some(60)),
    ("an omitted StorageResolution", None),
    ("StorageResolution 1", Some(1)),
];

/// One window of a read's answer, as every assertion here compares them.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Bucket {
    /// The instant the window is stamped with.
    at: f64,
    /// How many observations it stood for.
    sample_count: f64,
    /// Their total.
    sum: f64,
}

/// The minutes a row's observations are published into, relative to the minute the run is in.
///
/// A minute that CLOSED before the run, the one the run is IN (whose aggregate more observations
/// could still be published into), and one the clock has NOT REACHED — "data points with time
/// stamps … as much as two hours in the future" (`PutMetricData`).
const PLACEMENTS: [(&str, f64); 3] = [
    ("a minute that closed before the read", -10.0),
    ("the minute the read runs in", 0.0),
    ("a minute the clock has not reached", 10.0),
];

/// The start of the minute this run is in.
fn this_minute() -> f64 {
    (now_secs() / MINUTE).floor() * MINUTE
}

/// The value the `index`th observation of a row carries, so a row's `Sum`, `Minimum` and
/// `Maximum` are each distinct from its `SampleCount`.
#[allow(clippy::cast_precision_loss)]
fn value_of(index: usize) -> f64 {
    (index + 1) as f64
}

/// Publish `count` observations of `metric`, one per second from the start of `minute`, at
/// `resolution` seconds of storage resolution (omitting the member where `resolution` is `None`).
async fn publish(
    router: &MultiServiceRouter,
    metric: &str,
    minute: f64,
    count: usize,
    resolution: Option<i64>,
) {
    let data: Vec<Value> = (0..count)
        .map(|index| {
            #[allow(clippy::cast_precision_loss)]
            let offset = index as f64;
            let mut datum = json!({
                "MetricName": metric,
                "Dimensions": [{"Name": "Service", "Value": "api"}],
                "Value": value_of(index),
                "Unit": UNIT,
                "Timestamp": minute + offset,
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

/// The windows `GetMetricStatistics` answers with over `[start, end)` at `period`, oldest first,
/// each asserted to report the unit its observations were published with.
async fn statistics_buckets(
    router: &MultiServiceRouter,
    case: &str,
    metric: &str,
    period: i64,
    start: f64,
    end: f64,
) -> Vec<Bucket> {
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
            "Statistics": STATISTICS,
        }),
    )
    .await;
    let points = body["Datapoints"].as_array().cloned().unwrap_or_default();
    let mut buckets: Vec<Bucket> = points
        .iter()
        .map(|point| {
            assert_eq!(
                point["Unit"].as_str(),
                Some(UNIT),
                "case {case:?} at Period {period}: a datapoint reports the unit its observations \
                 were published with, got {point}"
            );
            let (minimum, maximum) = (
                point["Minimum"].as_f64().unwrap_or_default(),
                point["Maximum"].as_f64().unwrap_or_default(),
            );
            assert!(
                minimum <= maximum,
                "case {case:?} at Period {period}: expected Minimum <= Maximum, got {minimum} and \
                 {maximum} from {point}"
            );
            Bucket {
                at: point["Timestamp"].as_f64().unwrap_or_default(),
                sample_count: point["SampleCount"].as_f64().unwrap_or_default(),
                sum: point["Sum"].as_f64().unwrap_or_default(),
            }
        })
        .collect();
    buckets.sort_by(|left, right| left.at.total_cmp(&right.at));
    buckets
}

/// The same windows as `GetMetricData` answers with, folded by `SampleCount` and `Sum` — the two
/// aggregates the invariant is summed over — oldest first.
async fn metric_data_buckets(
    router: &MultiServiceRouter,
    case: &str,
    metric: &str,
    period: i64,
    start: f64,
    end: f64,
) -> Vec<Bucket> {
    let body = ok(
        router,
        "GetMetricData",
        &json!({
            "MetricDataQueries": [
                {
                    "Id": "counts",
                    "MetricStat": {
                        "Metric": {
                            "Namespace": NAMESPACE,
                            "MetricName": metric,
                            "Dimensions": [{"Name": "Service", "Value": "api"}],
                        },
                        "Period": period,
                        "Stat": "SampleCount",
                    },
                },
                {
                    "Id": "sums",
                    "MetricStat": {
                        "Metric": {
                            "Namespace": NAMESPACE,
                            "MetricName": metric,
                            "Dimensions": [{"Name": "Service", "Value": "api"}],
                        },
                        "Period": period,
                        "Stat": "Sum",
                    },
                },
            ],
            "StartTime": start,
            "EndTime": end,
            "ScanBy": "TimestampAscending",
        }),
    )
    .await;
    let series = |id: &str| -> (Vec<f64>, Vec<f64>) {
        let result = body["MetricDataResults"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|result| result["Id"].as_str() == Some(id))
            .unwrap_or(Value::Null);
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
            "case {case:?} at Period {period}: a result's Timestamps and Values stay in step, got \
             {result}"
        );
        (stamps, values)
    };
    let (count_stamps, counts) = series("counts");
    let (sum_stamps, sums) = series("sums");
    assert_eq!(
        count_stamps, sum_stamps,
        "case {case:?} at Period {period}: two statistics of one series are answered over the \
         same windows"
    );
    let mut buckets: Vec<Bucket> = count_stamps
        .into_iter()
        .zip(counts)
        .zip(sums)
        .map(|((at, sample_count), sum)| Bucket {
            at,
            sample_count,
            sum,
        })
        .collect();
    buckets.sort_by(|left, right| left.at.total_cmp(&right.at));
    buckets
}

/// How many observations, and how much, a read's whole answer stood for.
fn totals(buckets: &[Bucket]) -> (f64, f64) {
    buckets.iter().fold((0.0, 0.0), |(count, sum), bucket| {
        (count + bucket.sample_count, sum + bucket.sum)
    })
}

#[tokio::test]
async fn a_sub_minute_read_answers_only_from_a_series_stored_at_that_granularity() {
    let router = build_router();

    for (placement, minutes_away) in PLACEMENTS {
        let minute = this_minute() + minutes_away * MINUTE;
        // A range reaching a minute either side of the one published into, so what decides each
        // row is the granularity the series is stored at and not the range's ends.
        let (start, end) = (minute - MINUTE, minute + 2.0 * MINUTE);

        for (resolution_case, resolution) in RESOLUTIONS {
            for count in COUNTS {
                let case = format!(
                    "{count} observation(s) at {resolution_case} in {placement}, read over \
                     [{start}, {end})"
                );
                let metric = format!(
                    "Sub{}x{count}x{}",
                    resolution.unwrap_or_default(),
                    minutes_away
                );
                publish(&router, &metric, minute, count, resolution).await;

                // What the series holds, read at the granularity it is stored at: one window per
                // minute, the whole minute's aggregate in it.
                #[allow(clippy::cast_precision_loss)]
                let expected_samples = count as f64;
                let expected_sum: f64 = (0..count).map(value_of).sum();
                let reference = statistics_buckets(
                    &router,
                    &case,
                    &metric,
                    MINUTE_PERIOD,
                    start,
                    end,
                )
                .await;
                assert_eq!(
                    reference,
                    vec![Bucket {
                        at: minute,
                        sample_count: expected_samples,
                        sum: expected_sum,
                    }],
                    "case {case:?} at Period {MINUTE_PERIOD}: the minute holds one window \
                     carrying every observation published into it, got {reference:?}"
                );

                for period in SUB_MINUTE_PERIODS {
                    for (reader, got) in [
                        (
                            "GetMetricStatistics",
                            statistics_buckets(&router, &case, &metric, period, start, end).await,
                        ),
                        (
                            "GetMetricData",
                            metric_data_buckets(&router, &case, &metric, period, start, end).await,
                        ),
                    ] {
                        if resolution == Some(1) {
                            // Only a high-resolution series stores the sub-minute detail a finer
                            // period resolves, and there the invariant holds: a finer period
                            // answers over the SAME observations, so nothing the minute holds is
                            // selected out of the result.
                            assert_eq!(
                                totals(&got),
                                (expected_samples, expected_sum),
                                "case {case:?} at Period {period}: {reader} must answer over \
                                 every observation the minute holds ({expected_samples} of them, \
                                 summing to {expected_sum}), got {got:?}"
                            );
                            assert!(
                                got.iter().all(|bucket| bucket.at >= minute
                                    && bucket.at < minute + MINUTE),
                                "case {case:?} at Period {period}: every instant {reader} answers \
                                 with lies inside the minute the observations were published in \
                                 ({minute}), got {got:?}"
                            );
                            // The observations lie one second apart from the minute's start, so
                            // the windows they fall in are the `period`-wide ones that span them.
                            let span = usize::try_from(period).expect("a positive period");
                            let expected_windows = (count - 1) / span + 1;
                            assert_eq!(
                                got.len(),
                                expected_windows,
                                "case {case:?} at Period {period}: a high-resolution series holds \
                                 one window per {period}-second division the observations fall \
                                 in, expected {expected_windows}, got {got:?}"
                            );
                        } else {
                            // A regular-resolution series holds one aggregate for the whole
                            // minute and nothing finer, so no sub-minute period retrieves it.
                            assert!(
                                got.is_empty(),
                                "case {case:?} at Period {period}: a regular-resolution series is \
                                 retrievable only at multiples of the minute it is stored at, so \
                                 {reader} must answer with no datapoint, got {got:?}"
                            );
                        }
                    }
                }
            }
        }
    }
}
