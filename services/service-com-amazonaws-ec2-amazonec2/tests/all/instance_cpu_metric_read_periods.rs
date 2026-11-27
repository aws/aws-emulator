// SPDX-License-Identifier: CC0-1.0

//! What a CloudWatch READER sees of the `AWS/EC2` series a running instance publishes: the
//! observations one 5-minute read window holds, the stamps those windows carry, and the series
//! `ListMetrics` reports.
//!
//! THE RULE: a running EC2 instance publishes each of its own metrics into `AWS/EC2`,
//! dimensioned on exactly `{InstanceId}` and in that metric's own unit, once per period of the
//! cadence its MONITORING STATE selects — five minutes for a guest measurement while detailed
//! monitoring is off ("Basic monitoring | Status check metrics are available in 1-minute
//! periods. All other metrics are available in 5-minute periods"), one minute for the same
//! measurement while it is on ("Detailed monitoring | You can get metrics in 1-minute periods")
//! and one minute for a status-check metric in either state (Amazon EC2 User Guide, instance
//! monitoring). A reader sees that cadence because a read is of whole periods, laid from the start
//! time CloudWatch rounded the request's to and each datapoint stamped with the start of the
//! window it aggregates: HOW MANY observations one 300-second window holds is what tells the two
//! cadences apart — one under the 5-minute cadence and one per minute under the 1-minute one — and
//! every stamp is a whole number of periods from that rounded start, whatever instant the read
//! began at. A series that has been published into is a metric of the account, so `ListMetrics`
//! reports it, under that one dimension and no other.
//!
//! This is the reader's side of the cadence: the sibling suite `instance_metric_publication`
//! asserts the periods each series carries, read one minute at a time; here the same publication
//! is read at the 300-second granularity a client comparing the two cadences asks for, and from
//! an instant in the MIDDLE of a period, where the cadence shows up as the SampleCount and the
//! stamp of each window rather than as the number of stamps.
//!
//! The rows span what the rule generalises over: the monitoring state (basic and detailed), the
//! cadence class (guest measurements and a status check) and the unit (a percentage, bytes and
//! a count). Every expected name, unit, period and count is spelled out here rather than
//! imported, so a wrong constant in the implementation cannot make a case pass.
//!
//! The harness is the sibling suite's: Amazon CloudWatch mounted BESIDE Amazon EC2 on one engine,
//! as the assembled router mounts them, with the cadence driven by advancing the sampler's own
//! clock over a span of periods rather than by waiting for those periods to pass. Every datapoint
//! is read back through CloudWatch's own `GetMetricStatistics` and `ListMetrics`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::instance_metric_publication::{
    cloudwatch_ok, ec2_ok, emulator, launch, p, poll_until_reported, sample_the_span, span_base,
    FIVE_MINUTES, INSTANCE_ID_DIMENSION, MINUTE, NAMESPACE, SPAN_MINUTES,
};

/// An instance id no launch here minted, for the read that must find nothing.
const ABSENT_INSTANCE_ID: &str = "i-0fedcba9876543210";

/// How far into the first window of the span one case begins its read: a second that is a
/// multiple of neither cadence, so the window it begins inside is entered in the middle.
const OFF_GRID_OFFSET_SECS: i64 = 137;

/// How far two folded values may differ and still be the same number: the slack a sum folded
/// one datapoint at a time carries over the product of its average and its count.
const VALUE_EPSILON: f64 = 1e-9;

/// One row of the table: a metric a running instance publishes, the unit it is measured in and
/// how many observations one 300-second read window of its series holds in each monitoring
/// state.
///
/// The catalog, the units and the two cadences come from the metrics Amazon EC2 documents for
/// one INSTANCE (Amazon EC2 User Guide, "Instance metrics" and "Status check metrics") and from
/// the monitoring sentences the module docs quote; they are documented metrics rather than
/// Smithy shapes, so there is no model member to read them from.
struct ReadRow {
    /// The `MetricName` the series is published under.
    metric: &'static str,
    /// The `StandardUnit` its datapoints must carry.
    unit: &'static str,
    /// The highest value the unit admits, or `None` where the unit bounds none: a percentage is
    /// "the percentage of allocated EC2 compute units that are currently in use", so it cannot
    /// exceed 100, while a count of bytes or operations has no ceiling.
    ceiling: Option<f64>,
    /// How many observations one 300-second window holds while detailed monitoring is OFF.
    basic_observations: f64,
    /// How many it holds while detailed monitoring is ON.
    detailed_observations: f64,
}

/// How many observations one 300-second window of a 5-minute series holds: the one reading of
/// that period.
const ONE_PER_FIVE_MINUTES: f64 = 1.0;

/// How many it holds of a 1-minute series: one per minute of the window.
const ONE_PER_MINUTE: f64 = 5.0;

/// The rows this suite reads: a guest measurement per unit the catalog uses, and one
/// status-check metric, whose cadence is the same in both states.
const READ_ROWS: &[ReadRow] = &[
    ReadRow {
        metric: "CPUUtilization",
        unit: "Percent",
        ceiling: Some(100.0),
        basic_observations: ONE_PER_FIVE_MINUTES,
        detailed_observations: ONE_PER_MINUTE,
    },
    ReadRow {
        metric: "NetworkIn",
        unit: "Bytes",
        ceiling: None,
        basic_observations: ONE_PER_FIVE_MINUTES,
        detailed_observations: ONE_PER_MINUTE,
    },
    ReadRow {
        metric: "DiskWriteOps",
        unit: "Count",
        ceiling: None,
        basic_observations: ONE_PER_FIVE_MINUTES,
        detailed_observations: ONE_PER_MINUTE,
    },
    // A status check is "available in 1-minute periods" under basic monitoring too, so its
    // window holds a reading per minute whatever the monitoring state is.
    ReadRow {
        metric: "StatusCheckFailed_System",
        unit: "Count",
        ceiling: None,
        basic_observations: ONE_PER_MINUTE,
        detailed_observations: ONE_PER_MINUTE,
    },
];

/// One folded window as this suite asserts on it.
#[derive(Debug, Clone, PartialEq)]
struct Window {
    /// The start of the window, which is the stamp its datapoint carries.
    at: i64,
    /// The unit its observations were measured in.
    unit: String,
    /// How many observations it folded.
    sample_count: f64,
    /// What they averaged, and the extremes and the total they folded to.
    average: f64,
    minimum: f64,
    maximum: f64,
    sum: f64,
}

/// The `period`-wide windows of `metric` for `instance_id` inside `[start, end)`, oldest first.
async fn windows(
    router: &MultiServiceRouter,
    case: &str,
    instance_id: &str,
    metric: &str,
    period: i64,
    start: i64,
    end: i64,
) -> Vec<Window> {
    let answer = cloudwatch_ok(
        router,
        case,
        "GetMetricStatistics",
        &json!({
            "Namespace": NAMESPACE,
            "MetricName": metric,
            "Dimensions": [{"Name": INSTANCE_ID_DIMENSION, "Value": instance_id}],
            "StartTime": start,
            "EndTime": end,
            "Period": period,
            "Statistics": ["SampleCount", "Average", "Minimum", "Maximum", "Sum"],
        }),
    )
    .await;
    let mut points: Vec<Window> = answer["Datapoints"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|point| Window {
            at: point["Timestamp"].as_f64().unwrap_or_default().round() as i64,
            unit: point["Unit"].as_str().unwrap_or_default().to_string(),
            sample_count: point["SampleCount"].as_f64().unwrap_or_default(),
            average: point["Average"].as_f64().unwrap_or_default(),
            minimum: point["Minimum"].as_f64().unwrap_or_default(),
            maximum: point["Maximum"].as_f64().unwrap_or_default(),
            sum: point["Sum"].as_f64().unwrap_or_default(),
        })
        .collect();
    points.sort_by_key(|point| point.at);
    points
}

/// The series `ListMetrics` reports, as `(metric name, dimensions)` pairs with the dimensions in
/// the order they are reported.
pub(crate) async fn listed_series(
    router: &MultiServiceRouter,
    case: &str,
    request: &Value,
) -> Vec<(String, Vec<(String, String)>)> {
    let answer = cloudwatch_ok(router, case, "ListMetrics", request).await;
    let mut listed: Vec<(String, Vec<(String, String)>)> = answer["Metrics"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|metric| {
            let dimensions = metric["Dimensions"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|dimension| {
                    (
                        dimension["Name"].as_str().unwrap_or_default().to_string(),
                        dimension["Value"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect();
            (
                metric["MetricName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                dimensions,
            )
        })
        .collect();
    listed.sort();
    listed
}

/// Rule: the cadence an instance's monitoring state selects is what decides how many
/// observations one 300-second read window of its series holds — one under the 5-minute cadence
/// and one per minute under the 1-minute one — and every window carries that metric's own unit.
#[tokio::test]
async fn a_five_minute_read_window_holds_one_observation_per_period_of_the_cadence() {
    let case = "two instances, one with detailed monitoring";
    let (router, engine) = emulator();
    let ids = launch(&router, case, 2).await;
    let (basic, detailed) = (ids[0].clone(), ids[1].clone());
    ec2_ok(
        &router,
        case,
        "MonitorInstances",
        &[p("InstanceId.1", detailed.clone())],
    )
    .await;
    // Both instances are RUNNING and have settled their monitoring state before the span is
    // sampled, so each column's cadence is the one its own state selects.
    poll_until_reported(&router, case, &basic, "running", "disabled").await;
    poll_until_reported(&router, case, &detailed, "running", "enabled").await;

    let base = span_base(0);
    sample_the_span(&engine, base);
    let end = base + SPAN_MINUTES * MINUTE;
    // Every 300-second window of the span, which is what the read below must answer one
    // datapoint for.
    let expected_windows: Vec<i64> = (0..SPAN_MINUTES * MINUTE / FIVE_MINUTES)
        .map(|index| base + index * FIVE_MINUTES)
        .collect();

    for row in READ_ROWS {
        for (instance_id, observations, monitoring) in [
            (&basic, row.basic_observations, "basic monitoring"),
            (&detailed, row.detailed_observations, "detailed monitoring"),
        ] {
            let case = format!("{} under {monitoring}", row.metric);
            let read = windows(
                &router,
                &case,
                instance_id,
                row.metric,
                FIVE_MINUTES,
                base,
                end,
            )
            .await;
            let stamps: Vec<i64> = read.iter().map(|window| window.at).collect();
            assert_eq!(
                stamps, expected_windows,
                "case {case:?}: expected one datapoint per {FIVE_MINUTES}-second window of \
                 [{base}, {end}), got {read:?}"
            );
            for window in &read {
                assert_eq!(
                    window.sample_count, observations,
                    "case {case:?}: expected {observations} observation(s) per \
                     {FIVE_MINUTES}-second window, got {window:?}"
                );
                assert_eq!(
                    window.unit, row.unit,
                    "case {case:?}: expected the unit {:?}, got {window:?}",
                    row.unit
                );
                assert!(
                    window.minimum <= window.average && window.average <= window.maximum,
                    "case {case:?}: the average of a window lies between its extremes, got \
                     {window:?}"
                );
                assert!(
                    (window.sum - window.average * window.sample_count).abs() < VALUE_EPSILON,
                    "case {case:?}: the sum is the average over the observations folded, so \
                     {} was expected, got {window:?}",
                    window.average * window.sample_count
                );
                assert!(
                    window.minimum >= 0.0,
                    "case {case:?}: none of these metrics measures a negative quantity, got \
                     {window:?}"
                );
                if let Some(ceiling) = row.ceiling {
                    assert!(
                        window.maximum <= ceiling,
                        "case {case:?}: a {} is at most {ceiling}, got {window:?}",
                        row.unit
                    );
                }
            }
        }
    }
}

/// Rule: a client reading from ANY instant sees the cadence — a 5-minute read begun in the middle
/// of a period lays its windows from the start time rounded down to the whole minute ("Start time
/// less than 15 days ago - Round down to the nearest whole minute",
/// `GetMetricStatisticsInput$StartTime`), so the window that opens there holds the one reading of
/// the 5-minute cadence and the reading of the period before it, which the read did not ask for,
/// is not answered — while the same range read a minute at a time shows the 1-minute publication
/// at minutes that are not multiples of five.
#[tokio::test]
async fn a_read_begun_inside_a_period_lays_its_windows_from_the_rounded_start() {
    let case = "two instances, read from the middle of a period";
    let (router, engine) = emulator();
    let ids = launch(&router, case, 2).await;
    let (basic, detailed) = (ids[0].clone(), ids[1].clone());
    ec2_ok(
        &router,
        case,
        "MonitorInstances",
        &[p("InstanceId.1", detailed.clone())],
    )
    .await;
    poll_until_reported(&router, case, &basic, "running", "disabled").await;
    poll_until_reported(&router, case, &detailed, "running", "enabled").await;

    let base = span_base(0);
    sample_the_span(&engine, base);
    let end = base + SPAN_MINUTES * MINUTE;
    // A start inside the FIRST 5-minute window of the span, on no boundary of either cadence.
    let off_grid_start = base + OFF_GRID_OFFSET_SECS;

    let basic_read = windows(
        &router,
        case,
        &basic,
        "CPUUtilization",
        FIVE_MINUTES,
        off_grid_start,
        end,
    )
    .await;
    // Where the grid opens: the requested start rounded down to the whole minute, the first
    // window of the read running the 5 minutes from there.
    let anchor = off_grid_start - off_grid_start.rem_euclid(MINUTE);
    assert_eq!(
        basic_read
            .iter()
            .map(|window| (window.at, window.sample_count))
            .collect::<Vec<(i64, f64)>>(),
        vec![(anchor, ONE_PER_FIVE_MINUTES)],
        "case {case:?}: a read begun at {off_grid_start} lays its 5-minute windows from \
         {anchor}, so the one reading of the cadence inside [{anchor}, {}) is answered there and \
         the reading of the period before it is not answered at all, got {basic_read:?}",
        anchor + FIVE_MINUTES
    );
    for window in &basic_read {
        assert_eq!(
            (window.at - anchor).rem_euclid(FIVE_MINUTES),
            0,
            "case {case:?}: a window is stamped with its own start, which is a whole number of \
             5-minute periods from the rounded start {anchor}, got {window:?}"
        );
        assert_eq!(
            window.unit, "Percent",
            "case {case:?}: CPUUtilization is a percentage, got {window:?}"
        );
    }

    // The same window of the DETAILED instance, read a minute at a time: the 1-minute
    // publication shows up at minutes that are not multiples of five, which is what tells the
    // shortened cadence from the basic one.
    let detailed_read = windows(
        &router,
        case,
        &detailed,
        "CPUUtilization",
        MINUTE,
        off_grid_start,
        end,
    )
    .await;
    let off_the_five_minute_grid: Vec<&Window> = detailed_read
        .iter()
        .filter(|window| window.at.rem_euclid(FIVE_MINUTES) != 0)
        .collect();
    assert!(
        !off_the_five_minute_grid.is_empty(),
        "case {case:?}: detailed monitoring publishes 1-minute periods, so a minute-wide read \
         must answer stamps off the 5-minute grid, got {detailed_read:?}"
    );
    for window in &detailed_read {
        assert_eq!(
            window.at.rem_euclid(MINUTE),
            0,
            "case {case:?}: a 1-minute window begins on a whole minute, got {window:?}"
        );
    }
}

/// Rule: a series that has been published into is a metric of the account, so `ListMetrics`
/// reports it — under exactly the `{InstanceId}` dimension of the instance that published it,
/// and for no instance that published nothing.
#[tokio::test]
async fn list_metrics_reports_each_published_series_under_its_instance_dimension() {
    let case = "the series of one running instance";
    let (router, engine) = emulator();
    let instance_id = launch(&router, case, 1).await.remove(0);
    poll_until_reported(&router, case, &instance_id, "running", "disabled").await;
    sample_the_span(&engine, span_base(0));

    // Narrowed by the metric: the one series of that metric, dimensioned on that instance
    // alone.
    let cpu = listed_series(
        &router,
        case,
        &json!({"Namespace": NAMESPACE, "MetricName": "CPUUtilization"}),
    )
    .await;
    assert_eq!(
        cpu,
        vec![(
            "CPUUtilization".to_string(),
            vec![(INSTANCE_ID_DIMENSION.to_string(), instance_id.clone())]
        )],
        "case {case:?}: the published CPU series is listed under {INSTANCE_ID_DIMENSION} alone, \
         got {cpu:?}"
    );

    // Narrowed by the dimension: every metric this instance published, each under that same
    // one dimension.
    let by_instance = listed_series(
        &router,
        case,
        &json!({
            "Namespace": NAMESPACE,
            "Dimensions": [{"Name": INSTANCE_ID_DIMENSION, "Value": instance_id}],
        }),
    )
    .await;
    for row in READ_ROWS {
        let expected = (
            row.metric.to_string(),
            vec![(INSTANCE_ID_DIMENSION.to_string(), instance_id.clone())],
        );
        assert!(
            by_instance.contains(&expected),
            "case {case:?}: {} must be listed for {instance_id}, got {by_instance:?}",
            row.metric
        );
    }
    for (metric, dimensions) in &by_instance {
        assert_eq!(
            dimensions,
            &vec![(INSTANCE_ID_DIMENSION.to_string(), instance_id.clone())],
            "case {case:?}: {metric} is dimensioned on that instance alone, got {dimensions:?}"
        );
    }

    // And an instance nothing launched published nothing, so it names no series at all.
    let absent = listed_series(
        &router,
        case,
        &json!({
            "Namespace": NAMESPACE,
            "Dimensions": [{"Name": INSTANCE_ID_DIMENSION, "Value": ABSENT_INSTANCE_ID}],
        }),
    )
    .await;
    assert!(
        absent.is_empty(),
        "case {case:?}: {ABSENT_INSTANCE_ID} published nothing, so it names no series, got \
         {absent:?}"
    );
}
