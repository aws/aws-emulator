// SPDX-License-Identifier: CC0-1.0

//! What `GetMetricData` answers for a series holding BOTH storage resolutions, at every period a
//! query may ask for and under every statistic one window is folded by, end to end over the wire.
//!
//! ONE rule, asserted over a table: **`GetMetricData` answers a query from every datum of the
//! series its range holds whose own storage resolution DIVIDES the requested `Period`, each
//! reported at the start of the span that resolution stores it in — "Setting this to 1 specifies
//! this metric as a high-resolution metric, so that CloudWatch stores the metric with sub-minute
//! resolution down to one second. Setting this to 60 specifies this metric as regular resolution,
//! which CloudWatch stores at 1-minute resolution… This field is optional, if you do not specify
//! it the default of 60 is used."
//! (`smithy_query(query="doc",shape="MetricDatum",member="StorageResolution")`) — and the data
//! sharing one window of the requested `Period` are folded into one value by the query's `Stat`.**
//!
//! So a sub-minute `Period` — a granularity only high-resolution storage keeps, "For metrics with
//! regular resolution, a period can be as short as one minute (60 seconds) and must be a multiple
//! of 60. For high-resolution metrics … the period can be 1, 5, 10, 20, 30, 60, or any multiple of
//! 60" (`smithy_query(query="doc",shape="MetricStat",member="Period")`) — answers from the
//! high-resolution datum at its own second and from the regular-resolution one NOT AT ALL, that
//! period being none of the granularities a datum stored at one-minute resolution is retrievable
//! at: "you must specify the period as follows or no data points in that time range is returned"
//! (the same member). At a period of a minute or more the two share one window, so the `Sum` of
//! that window counts both observations and its `SampleCount` is 2.
//!
//! The rows differ along what the rule generalises over: the `Period` asked for (every sub-minute
//! value `MetricStat$Period` admits, the minute itself, and a multiple of it), the `Stat` the
//! window is folded by (one summing, one counting, one extremal, one averaging), how the
//! regular-resolution datum declares its class (the explicit 60 and the omitted member documented
//! to read as 60), and where in its minute each datum is stamped — including a pair whose two data
//! share a window at some periods and not at others, so no row can pass by reporting a fixed
//! number of points. Each datum carries a value of its own, so which data a window folded is
//! visible in the value it answers with.
//!
//! Nothing here waits on the clock: every expectation follows from the time stamps the publish
//! carried.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{build_router, now_secs, ok};

/// One minute, in the epoch seconds a published time stamp carries.
const MINUTE: f64 = 60.0;

/// The namespace every series of this suite is published under.
const NAMESPACE: &str = "MyApp/MixedResolutionFolds";

/// The widest `Period` any row reads, in seconds: the grid the base instant sits on, so every
/// row's window grid opens exactly at that instant.
const WIDEST_PERIOD: f64 = 300.0;

/// How far in the past the data is stamped, in minutes: inside the three-hour tier a sub-minute
/// period is served from ("Data points with a period of less than 60 seconds are available for 3
/// hours", `GetMetricStatisticsInput$Period`) and behind the clock, so no row reads a window the
/// publish is ahead of.
const DISPLACEMENT_MINUTES: f64 = 10.0;

/// The span a HIGH-resolution datum is stored in: "sub-minute resolution down to one second".
const HIGH_RESOLUTION_SPAN: f64 = 1.0;

/// The span a REGULAR-resolution datum is stored in: "1-minute resolution".
const REGULAR_RESOLUTION_SPAN: f64 = MINUTE;

/// The periods each series is read at: every sub-minute value `MetricStat$Period` admits, the
/// minute a regular-resolution series is stored at, and a multiple of it.
const PERIODS: [i32; 7] = [1, 5, 10, 20, 30, 60, 300];

/// The statistics one window is folded by, each a `Statistic` enum value
/// (`smithy_query(query="shape",shape="Statistic")`): one summing, one counting, one extremal and
/// one averaging, so a window holding two observations is told from one holding a single
/// observation of the same total.
const STATS: [&str; 4] = ["Sum", "SampleCount", "Maximum", "Average"];

/// The start of the `span`-wide clock window `instant` falls in.
fn window_of(instant: f64, span: f64) -> f64 {
    (instant / span).floor() * span
}

/// The instant every row's publish and read is laid out from: a boundary of the widest period any
/// row asks for, [`DISPLACEMENT_MINUTES`] behind the clock.
fn base_minute() -> f64 {
    window_of(now_secs(), WIDEST_PERIOD) - DISPLACEMENT_MINUTES * MINUTE
}

/// One datum of a published pair.
struct Datum {
    /// How far into the base minute it is stamped.
    offset: f64,
    /// The value it carries, which is what tells the data of one window apart.
    value: f64,
    /// `StorageResolution` as the datum declares it, or `None` for the omitted member — "if you
    /// do not specify it the default of 60 is used".
    declared_resolution: Option<i64>,
}

impl Datum {
    /// The span the datum is STORED in, which is where the read reports it from: its own second
    /// for the high-resolution class, the whole minute for the regular one — and for the omitted
    /// member the minute the default it reads as stores it in.
    fn storage_span(&self) -> f64 {
        match self.declared_resolution {
            Some(1) => HIGH_RESOLUTION_SPAN,
            _ => REGULAR_RESOLUTION_SPAN,
        }
    }

    /// How far into the base minute the span it is stored in BEGINS.
    fn stored_offset(&self) -> f64 {
        window_of(self.offset, self.storage_span())
    }
}

/// One series: the metric it is published under and the pair of data ONE `PutMetricData` writes
/// into it — the recorded shape, where both classes arrive in a single call.
struct Series {
    case: &'static str,
    metric: &'static str,
    data: [Datum; 2],
}

/// The series under test: each pair carries one high-resolution datum and one regular-resolution
/// one inside the same minute, which is what tells the two classes apart.
fn series() -> Vec<Series> {
    vec![
        Series {
            case: "a regular-resolution datum declaring StorageResolution 60 beside a \
                   high-resolution one later in the minute",
            metric: "MixResDeclared",
            data: [
                Datum {
                    offset: 11.0,
                    value: 1.0,
                    declared_resolution: Some(60),
                },
                Datum {
                    offset: 37.0,
                    value: 2.0,
                    declared_resolution: Some(1),
                },
            ],
        },
        Series {
            case: "a regular-resolution datum with no StorageResolution member at all beside a \
                   high-resolution one seconds later",
            metric: "MixResDefaulted",
            data: [
                Datum {
                    offset: 2.0,
                    value: 1.0,
                    declared_resolution: None,
                },
                Datum {
                    offset: 7.0,
                    value: 2.0,
                    declared_resolution: Some(1),
                },
            ],
        },
        Series {
            case: "a high-resolution datum stamped BEFORE the regular-resolution one of the same \
                   call",
            metric: "MixResHighFirst",
            data: [
                Datum {
                    offset: 23.0,
                    value: 4.0,
                    declared_resolution: Some(1),
                },
                Datum {
                    offset: 49.0,
                    value: 8.0,
                    declared_resolution: None,
                },
            ],
        },
    ]
}

/// Publish one series' pair of data in ONE `PutMetricData` call, stamped inside the base minute.
async fn publish(router: &MultiServiceRouter, base: f64, series: &Series) {
    let data: Vec<Value> = series
        .data
        .iter()
        .map(|datum| {
            let mut published = json!({
                "MetricName": series.metric,
                "Value": datum.value,
                "Timestamp": base + datum.offset,
            });
            if let Some(resolution) = datum.declared_resolution {
                published["StorageResolution"] = json!(resolution);
            }
            published
        })
        .collect();
    ok(
        router,
        "PutMetricData",
        &json!({"Namespace": NAMESPACE, "MetricData": data}),
    )
    .await;
}

/// The `(offset from `base`, value)` points `GetMetricData` answers with for `metric` over
/// `[base, base + 60)` at `period`, folded by `stat` and scanned as `scan_by` asks — in the order
/// the result lists them, so the scan order is asserted along with the values.
async fn metric_data_points(
    router: &MultiServiceRouter,
    base: f64,
    metric: &str,
    period: i32,
    stat: &str,
    scan_by: &str,
) -> Vec<(f64, f64)> {
    let body = ok(
        router,
        "GetMetricData",
        &json!({
            "MetricDataQueries": [{
                "Id": "mix",
                "MetricStat": {
                    "Metric": {"Namespace": NAMESPACE, "MetricName": metric},
                    "Period": period,
                    "Stat": stat,
                },
                "ReturnData": true,
            }],
            "StartTime": base,
            "EndTime": base + MINUTE,
            "ScanBy": scan_by,
        }),
    )
    .await;
    let result = &body["MetricDataResults"][0];
    // A result that answered the whole query says so, and says nothing else: the query is served
    // from the store, so a period finer than the series' own granularity is not a partial answer.
    assert_eq!(
        result["StatusCode"].as_str(),
        Some("Complete"),
        "case {metric:?} at Period {period} Stat {stat}: expected a complete result, got {result}"
    );
    assert_eq!(
        result["Messages"].as_array().map(Vec::len).unwrap_or(0),
        0,
        "case {metric:?} at Period {period} Stat {stat}: expected no messages, got {result}"
    );
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
        "case {metric:?} at Period {period} Stat {stat}: a result's Timestamps and Values stay in \
         step, got {result}"
    );
    stamps
        .into_iter()
        .map(|stamp| stamp - base)
        .zip(values)
        .collect()
}

/// What the rule requires of one read: the `(offset from the base, value)` points of the series'
/// data, each in the window of `period` the span it is STORED in begins in, folded by `stat`, in
/// ascending order of their instants.
fn expected_points(data: &[Datum; 2], period: i32, stat: &str) -> Vec<(f64, f64)> {
    let span = f64::from(period);
    let mut windows: BTreeMap<i64, Vec<f64>> = BTreeMap::new();
    for datum in data {
        if !readable_at(datum, period) {
            continue;
        }
        #[allow(clippy::cast_possible_truncation)]
        let index = (datum.stored_offset() / span).floor() as i64;
        windows.entry(index).or_default().push(datum.value);
    }
    windows
        .into_iter()
        .map(|(index, observations)| {
            #[allow(clippy::cast_precision_loss)]
            let offset = index as f64 * span;
            (offset, folded(&observations, stat))
        })
        .collect()
}

/// Whether a read at `period`-wide windows is answered from `datum` at all.
///
/// The granularities a datum can be retrieved at are the multiples of the resolution it was
/// STORED at — every period for a high-resolution datum, and only the multiples of a minute for a
/// regular-resolution one ("For metrics with regular resolution, a period can be as short as one
/// minute (60 seconds) and must be a multiple of 60", `MetricStat$Period`) — and a query at a
/// finer granularity than a datum's own resolution is answered with no point for it: "you must
/// specify the period as follows or no data points in that time range is returned" (the same
/// member).
fn readable_at(datum: &Datum, period: i32) -> bool {
    #[allow(clippy::cast_possible_truncation)]
    let resolution = datum.storage_span() as i32;
    period >= resolution && period % resolution == 0
}

/// The value `stat` folds one window's observations into.
fn folded(observations: &[f64], stat: &str) -> f64 {
    let sum: f64 = observations.iter().sum();
    #[allow(clippy::cast_precision_loss)]
    let count = observations.len() as f64;
    match stat {
        "Sum" => sum,
        "SampleCount" => count,
        "Maximum" => observations.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        "Average" => sum / count,
        other => panic!("the suite folds no statistic {other:?}"),
    }
}

/// RULE: a `GetMetricData` query is answered from every datum of its range, each at the start of
/// the span its own storage resolution stores it in, folded by the query's `Stat`.
#[tokio::test]
async fn get_metric_data_reports_each_datum_in_the_span_its_resolution_stores_it_in() {
    let router = build_router();
    let base = base_minute();
    for series in series() {
        publish(&router, base, &series).await;
    }

    for series in series() {
        for period in PERIODS {
            for stat in STATS {
                let expected = expected_points(&series.data, period, stat);
                let got =
                    metric_data_points(&router, base, series.metric, period, stat, "TimestampAscending")
                        .await;
                assert_eq!(
                    got, expected,
                    "case {:?}: expected GetMetricData at Period {period} Stat {stat} to answer \
                     {expected:?} as offsets from the minute, got {got:?}",
                    series.case
                );
            }
        }
    }
}

/// The SUM a period answers with totals exactly the data that period RETRIEVES, and its
/// `SampleCount` counts exactly those observations: a period of a minute or more totals both data
/// of the pair, and a sub-minute period totals the high-resolution one alone — which is what a
/// client validating its own publishing path reads back.
#[tokio::test]
async fn the_totals_of_a_mixed_resolution_series_agree_across_the_periods_it_is_read_at() {
    let router = build_router();
    let base = base_minute();
    for series in series() {
        publish(&router, base, &series).await;
    }

    for series in series() {
        for period in PERIODS {
            // The data this period retrieves: the whole pair at a multiple of a minute, and the
            // high-resolution datum alone below it.
            let retrieved: Vec<&Datum> = series
                .data
                .iter()
                .filter(|datum| readable_at(datum, period))
                .collect();
            let retrieved_sum: f64 = retrieved.iter().map(|datum| datum.value).sum();
            #[allow(clippy::cast_precision_loss)]
            let retrieved_count = retrieved.len() as f64;
            for (stat, total) in [("Sum", retrieved_sum), ("SampleCount", retrieved_count)] {
                let got = metric_data_points(
                    &router,
                    base,
                    series.metric,
                    period,
                    stat,
                    "TimestampAscending",
                )
                .await;
                let answered: f64 = got.iter().map(|(_, value)| *value).sum();
                assert_eq!(
                    answered, total,
                    "case {:?}: expected the {stat} answered at Period {period} to total {total} \
                     over the series' observations, got {got:?}",
                    series.case
                );
            }
        }
    }
}

/// The scan order is the only thing `ScanBy` decides: the same points, newest first — "The order
/// in which data points should be returned" (`GetMetricDataInput$ScanBy`), so a client reading a
/// mixed-resolution series descending loses none of it.
#[tokio::test]
async fn a_descending_scan_answers_the_same_mixed_resolution_points_newest_first() {
    let router = build_router();
    let base = base_minute();
    for series in series() {
        publish(&router, base, &series).await;
    }

    for series in series() {
        for period in PERIODS {
            let ascending = expected_points(&series.data, period, "Sum");
            let mut descending = ascending.clone();
            descending.reverse();
            let got = metric_data_points(
                &router,
                base,
                series.metric,
                period,
                "Sum",
                "TimestampDescending",
            )
            .await;
            assert_eq!(
                got, descending,
                "case {:?}: expected the descending scan at Period {period} to answer \
                 {descending:?}, got {got:?}",
                series.case
            );
        }
    }
}
