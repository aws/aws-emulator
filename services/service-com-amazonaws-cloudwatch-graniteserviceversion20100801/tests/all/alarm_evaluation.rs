// SPDX-License-Identifier: CC0-1.0

//! The METRIC-ALARM EVALUATION slice, end to end over the wire: a metric alarm's
//! `StateValue` computed from the datapoints `PutMetricData` stored, the history item the
//! transition records, and the SNS action the transition fires — plus the namespace a
//! publisher may not write into.
//!
//! Two rules are asserted here, each over a TABLE whose rows differ along what the rule
//! generalises over rather than along a name:
//!
//! 1. `PutMetricData` refuses a `Namespace` in the AWS-reserved namespace space (one
//!    beginning with `AWS/`) with HTTP 400 `InvalidParameterValue` and stores nothing, and
//!    accepts every other namespace. Rows differ in which reserved namespace is named, how
//!    the prefix is spelled and where it appears.
//! 2. A metric alarm's "M out of N" predicate has two directions, each with a minimum built
//!    from its configuration: it is in `ALARM` once at least `DatapointsToAlarm` of the
//!    datapoints its evaluation found in the range it searches satisfy its `ComparisonOperator`
//!    against `Threshold`, in `OK` once at least `EvaluationPeriods - DatapointsToAlarm + 1` of
//!    them do not, and in `INSUFFICIENT_DATA` where the datapoints it has reach neither minimum
//!    — unless the missing-data treatment is `ignore`, which keeps the state it has. Rows
//!    differ in the comparison, the statistic, the datapoint values and their count, how many of
//!    the evaluated periods carry one, the age of a datapoint, its unit, its dimension set and
//!    the `TreatMissingData` setting.
//! 3. The `StateReason` of an evaluated state explains how that state was decided: it NAMES
//!    the datapoints the evaluation graded, each value with the instant of the window it
//!    aggregates, and a window that held no datapoint of the metric is reported as the hole it
//!    was, naming the missing-data arm applied to it, rather than being reported as a
//!    datapoint the evaluation graded. Rows differ in the arm in force, how many of the
//!    evaluated windows hold a real datapoint, the number of evaluation periods and whether
//!    the comparison held. The datapoints a reason names are a table of their own
//!    (`alarm_reason_datapoints`).
//!
//! The recorded case the issue was filed about (a `SearchableDocuments` datapoint driven
//! past an alarm's threshold, and a forged `AWS/ES` datapoint) is one row of each table.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    build_router, call, element, emulator, error_code, graded_window_stamp, grant_sns_delivery,
    merged, ok, rpc, sns, wait_for_evaluation, wait_for_period_room,
};

/// The customer namespace the alarm rows below watch.
const NAMESPACE: &str = "Custom/OpenSearch";

/// The metric the alarm rows below watch.
const METRIC: &str = "SearchableDocuments";

/// The shortest `Period` an alarm may be evaluated on: "Valid values are 10, 20, 30, and any
/// multiple of 60." (`smithy_query(query="doc",shape="PutMetricAlarmInput",member="Period")`).
///
/// The rows that wait out a span measured in evaluation periods use it, so they wait the
/// shortest span an alarm can be configured for.
const SHORTEST_PERIOD_SECS: i32 = 10;

/// How many periods per evaluation period an alarm's EVALUATION RANGE covers in this emulator
/// (`crate::state::alarm_evaluation::EVALUATION_RANGE_FACTOR`): CloudWatch "attempts to
/// retrieve a higher number of data points than the number specified as Evaluation Periods"
/// and publishes no number for how many, so the rows that drive a datapoint OUT of the range
/// are written against the emulator's own factor.
const EVALUATION_RANGE_PERIODS: i32 = 3;

/// The span of clock that range reaches back over at the least, in seconds
/// (`crate::state::alarm_evaluation::MINIMUM_EVALUATION_RANGE_SPAN_SECS`): how many datapoints
/// CloudWatch attempts to retrieve depends on the LENGTH of the alarm's period as well as on its
/// look-back, so an alarm whose periods are finer than the shortest regular one searches more of
/// them rather than a shorter stretch of its series — which is what the rows below have to reach
/// past to drive a datapoint out of the range.
const MINIMUM_EVALUATION_RANGE_SPAN_SECS: i32 = 180;

/// How many period windows the evaluation range of an alarm of `periods` evaluation periods on
/// `period_secs`-second periods covers: the wider of the look-back's own multiple
/// ([`EVALUATION_RANGE_PERIODS`]) and the windows one [`MINIMUM_EVALUATION_RANGE_SPAN_SECS`] takes.
fn evaluation_range_periods(periods: i32, period_secs: i32) -> i32 {
    // The ceiling of that division, written out: `i32::div_ceil` is unstable on this toolchain.
    let covering_the_minimum_span = MINIMUM_EVALUATION_RANGE_SPAN_SECS / period_secs
        + i32::from(MINIMUM_EVALUATION_RANGE_SPAN_SECS % period_secs != 0);
    (EVALUATION_RANGE_PERIODS * periods).max(covering_the_minimum_span)
}

/// Block until the clock is just past a `period`-second boundary.
///
/// A metric is aggregated into periods anchored at multiples of the period, so a row that
/// needs its datapoint to stay in one period across a reading — or to leave one after a known
/// wait — has to know where in the period it is publishing: just past a boundary is where
/// almost the whole window is still ahead ([`wait_for_period_room`]).
async fn wait_for_period_start(period: f64) {
    /// How far into a period is close enough to its start for a reading to follow inside it.
    const CLOSE_ENOUGH_SECS: f64 = 2.0;

    wait_for_period_room(period, period - CLOSE_ENOUGH_SECS).await;
}

/// The `Period` the base alarm below is evaluated on, and so the window [`publish`] places a
/// row's datapoints in.
pub(crate) const BASE_PERIOD_SECS: f64 = 60.0;

/// The alarm every row of the evaluation table starts from: one 60-second period, one
/// evaluation period, a static threshold of 10 and a `GreaterThanThreshold` comparison.
///
/// Each row watches its OWN dimension set — the one its `DomainName` dimension names — so
/// every row has its own time series and the rows share one emulator.
pub(crate) fn base_alarm(name: &str) -> Value {
    json!({
        "AlarmName": name,
        "Namespace": NAMESPACE,
        "MetricName": METRIC,
        "Dimensions": [{"Name": "DomainName", "Value": name}],
        "Statistic": "Average",
        "Period": 60,
        "EvaluationPeriods": 1,
        "Threshold": 10.0,
        "ComparisonOperator": "GreaterThanThreshold",
    })
}

/// One datapoint a row publishes: its value, how many seconds before the newest window the
/// alarm's evaluation grades its time stamp lies, and the extra members that make the row what
/// it is (another `Unit`, another dimension set).
#[derive(Clone, Debug)]
pub(crate) struct Datum {
    value: f64,
    age_secs: f64,
    extra: Value,
}

/// A datapoint of `value`, stamped `age_secs` before the middle of the newest window the
/// evaluation grades ([`crate::harness::graded_window_stamp`]).
pub(crate) fn datum(value: f64, age_secs: f64) -> Datum {
    Datum {
        value,
        age_secs,
        extra: json!({}),
    }
}

impl Datum {
    /// The same datapoint with `extra` merged into its `MetricDatum`.
    fn with(mut self, extra: Value) -> Self {
        self.extra = extra;
        self
    }

    /// This datapoint as a `MetricDatum` of the series `domain` names, stamped against `base` —
    /// the middle of the newest window an evaluation grades, because an alarm grades only the
    /// periods that have closed.
    ///
    /// Published at high resolution: the rows that evaluate a sub-minute period need a series
    /// stored at that resolution, because regular resolution stores a datapoint at 1-minute
    /// granularity ("Setting this to 1 specifies this metric as a high-resolution metric […]
    /// Setting this to 60 specifies this metric as regular resolution, which CloudWatch stores
    /// at 1-minute resolution", `MetricDatum$StorageResolution`). A row evaluating 60-second
    /// periods reads the same windows either way.
    fn rendered(&self, base: f64, domain: &str) -> Value {
        let mut rendered = json!({
            "MetricName": METRIC,
            "Dimensions": [{"Name": "DomainName", "Value": domain}],
            "Value": self.value,
            "StorageResolution": 1,
            "Timestamp": base - self.age_secs,
        });
        for (member, value) in self.extra.as_object().cloned().unwrap_or_default() {
            rendered[member] = value;
        }
        rendered
    }
}

/// Publish `data` as the series the `DomainName` dimension `domain` names, in one call, for an
/// alarm evaluated on the base alarm's [`BASE_PERIOD_SECS`] period.
pub(crate) async fn publish(router: &MultiServiceRouter, domain: &str, data: &[Datum]) {
    publish_in_period(router, domain, BASE_PERIOD_SECS, data).await;
}

/// The same publish for a row whose alarm is evaluated on `period` seconds: a row's datapoint
/// ages are measured from the newest window THAT alarm's evaluation grades, so a row with
/// another period places its datapoints in its own windows.
pub(crate) async fn publish_in_period(
    router: &MultiServiceRouter,
    domain: &str,
    period: f64,
    data: &[Datum],
) {
    if data.is_empty() {
        return;
    }
    let base = graded_window_stamp(period);
    let rendered: Vec<Value> = data
        .iter()
        .map(|datum| datum.rendered(base, domain))
        .collect();
    ok(
        router,
        "PutMetricData",
        &json!({"Namespace": NAMESPACE, "MetricData": rendered}),
    )
    .await;
}

/// Apply a row's patch to the base alarm: every member the row names replaces the base one,
/// and a member the row names `null` is REMOVED — an alarm carries `Statistic` or
/// `ExtendedStatistic`, never both, so a row that names one has to drop the other.
pub(crate) fn patched(name: &str, patch: &Value) -> Value {
    merged(base_alarm(name), patch)
}

/// Publish `data` for the alarm a row's `patch` configures: the datapoint ages of a row are
/// measured from the newest window THAT alarm's evaluation grades, so a row naming its own
/// `Period` places its datapoints in its own windows.
pub(crate) async fn publish_patched(
    router: &MultiServiceRouter,
    domain: &str,
    patch: &Value,
    data: &[Datum],
) {
    let period = patch
        .get("Period")
        .and_then(Value::as_f64)
        .unwrap_or(BASE_PERIOD_SECS);
    publish_in_period(router, domain, period, data).await;
}

#[tokio::test]
async fn an_alarms_state_is_evaluated_from_the_datapoints_it_watches() {
    // (case, the members this row changes on the base alarm, the datapoints it publishes,
    //  the state the evaluation must put the alarm in)
    let rows: Vec<(&str, Value, Vec<Datum>, &str)> = vec![
        (
            "a datapoint past a GreaterThanThreshold threshold",
            json!({}),
            vec![datum(42.0, 0.0)],
            "ALARM",
        ),
        (
            "a datapoint exactly at a GreaterThanThreshold threshold",
            json!({}),
            vec![datum(10.0, 0.0)],
            "OK",
        ),
        (
            "a datapoint exactly at a GreaterThanOrEqualToThreshold threshold",
            json!({"ComparisonOperator": "GreaterThanOrEqualToThreshold"}),
            vec![datum(10.0, 0.0)],
            "ALARM",
        ),
        (
            "a datapoint under a LessThanThreshold threshold",
            json!({"ComparisonOperator": "LessThanThreshold"}),
            vec![datum(3.0, 0.0)],
            "ALARM",
        ),
        (
            "a datapoint exactly at a LessThanOrEqualToThreshold threshold",
            json!({"ComparisonOperator": "LessThanOrEqualToThreshold"}),
            vec![datum(10.0, 0.0)],
            "ALARM",
        ),
        (
            "a datapoint over a LessThanOrEqualToThreshold threshold",
            json!({"ComparisonOperator": "LessThanOrEqualToThreshold"}),
            vec![datum(11.0, 0.0)],
            "OK",
        ),
        (
            "the Average of two datapoints of one period crosses",
            json!({}),
            vec![datum(2.0, 0.0), datum(30.0, 0.0)],
            "ALARM",
        ),
        (
            "the Minimum of the same two datapoints does not",
            json!({"Statistic": "Minimum"}),
            vec![datum(2.0, 0.0), datum(30.0, 0.0)],
            "OK",
        ),
        (
            "the Maximum of the same two datapoints does",
            json!({"Statistic": "Maximum"}),
            vec![datum(2.0, 0.0), datum(30.0, 0.0)],
            "ALARM",
        ),
        (
            "the Sum of the same two datapoints does",
            json!({"Statistic": "Sum"}),
            vec![datum(2.0, 0.0), datum(30.0, 0.0)],
            "ALARM",
        ),
        (
            "the SampleCount of two datapoints is under a threshold of 10",
            json!({"Statistic": "SampleCount"}),
            vec![datum(2.0, 0.0), datum(30.0, 0.0)],
            "OK",
        ),
        (
            "the p90 extended statistic of ten datapoints crosses",
            json!({"Statistic": Value::Null, "ExtendedStatistic": "p90"}),
            (0..10).map(|i| datum(f64::from(i) * 3.0, 0.0)).collect(),
            "ALARM",
        ),
        (
            "the p10 extended statistic of the same ten datapoints does not",
            json!({"Statistic": Value::Null, "ExtendedStatistic": "p10"}),
            (0..10).map(|i| datum(f64::from(i) * 3.0, 0.0)).collect(),
            "OK",
        ),
        (
            "one of three periods breaches, and one datapoint is enough",
            json!({"EvaluationPeriods": 3, "DatapointsToAlarm": 1}),
            vec![datum(42.0, 0.0), datum(1.0, 70.0), datum(1.0, 130.0)],
            "ALARM",
        ),
        (
            "one of three periods breaches, and two datapoints are needed",
            json!({"EvaluationPeriods": 3, "DatapointsToAlarm": 2}),
            vec![datum(42.0, 0.0), datum(1.0, 70.0), datum(1.0, 130.0)],
            "OK",
        ),
        (
            "two of three periods breach, and two datapoints are needed",
            json!({"EvaluationPeriods": 3, "DatapointsToAlarm": 2}),
            vec![datum(42.0, 0.0), datum(43.0, 70.0), datum(1.0, 130.0)],
            "ALARM",
        ),
        (
            "every period breaches, and no DatapointsToAlarm is named",
            json!({"EvaluationPeriods": 2}),
            vec![datum(42.0, 0.0), datum(43.0, 70.0)],
            "ALARM",
        ),
        (
            "only the newest of two periods breaches, and no DatapointsToAlarm is named",
            json!({"EvaluationPeriods": 2}),
            vec![datum(42.0, 0.0), datum(1.0, 70.0)],
            "OK",
        ),
        (
            "no datapoint at all, with the default missing-data treatment",
            json!({}),
            Vec::new(),
            "INSUFFICIENT_DATA",
        ),
        (
            "no datapoint at all, with missing data treated as breaching",
            json!({"TreatMissingData": "breaching"}),
            Vec::new(),
            "ALARM",
        ),
        (
            "no datapoint at all, with missing data treated as not breaching",
            json!({"TreatMissingData": "notBreaching"}),
            Vec::new(),
            "OK",
        ),
        (
            // "ignore — the current alarm state is maintained" speaks about MISSING data: a
            // datapoint that is present still decides the state.
            "a breaching period beside a missing one, with missing data ignored",
            json!({"EvaluationPeriods": 2, "DatapointsToAlarm": 1, "TreatMissingData": "ignore"}),
            vec![datum(42.0, 0.0)],
            "ALARM",
        ),
        (
            // "ignore — the current alarm state is maintained" speaks about a window the data
            // present cannot decide: one clean datapoint is short of the two an alarm needs to
            // clear a 1-of-2 predicate, so the alarm keeps the state its creation left it in.
            "a non-breaching period beside a missing one, with missing data ignored",
            json!({"EvaluationPeriods": 2, "DatapointsToAlarm": 1, "TreatMissingData": "ignore"}),
            vec![datum(1.0, 0.0)],
            "INSUFFICIENT_DATA",
        ),
        (
            "a breaching period beside a missing one, with missing data missing",
            json!({"EvaluationPeriods": 2, "DatapointsToAlarm": 1, "TreatMissingData": "missing"}),
            vec![datum(42.0, 0.0)],
            "ALARM",
        ),
        (
            // The missing-data treatments that substitute NOTHING leave the evaluation with
            // fewer datapoints than the alarm's N, and a window that short reaches neither
            // minimum: two breaching datapoints are one short of this alarm's M, and no
            // datapoint at all satisfies the OK direction, so nothing is decided.
            "two of three periods breach and the third is missing, with three needed",
            json!({"EvaluationPeriods": 3, "DatapointsToAlarm": 3}),
            vec![datum(42.0, 0.0), datum(43.0, 70.0)],
            "INSUFFICIENT_DATA",
        ),
        (
            // The same shape with M defaulting to N: one breaching datapoint of two periods
            // reaches neither the two breaching the ALARM direction needs nor the one clean
            // datapoint the OK direction needs.
            "one of two periods breaches and the other is missing, with no DatapointsToAlarm",
            json!({"EvaluationPeriods": 2}),
            vec![datum(42.0, 0.0)],
            "INSUFFICIENT_DATA",
        ),
        (
            // … and one breaching datapoint IS enough where the alarm's M is one: the window
            // being short of N does not change the minimum the direction needs.
            "one of two periods breaches and the other is missing, with one datapoint needed",
            json!({"EvaluationPeriods": 2, "DatapointsToAlarm": 1}),
            vec![datum(42.0, 0.0)],
            "ALARM",
        ),
        (
            // `notBreaching` SUBSTITUTES a datapoint for the missing window, so the
            // evaluation has both of its datapoints and the alarm's M is asked for in full.
            "one of two periods breaches and the other is treated as not breaching",
            json!({"EvaluationPeriods": 2, "TreatMissingData": "notBreaching"}),
            vec![datum(42.0, 0.0)],
            "OK",
        ),
        (
            // The evaluation range reaches back past the look-back, so a series published a
            // little slower than the alarm's period is still evaluated on what it published.
            "a breaching datapoint one period older than the look-back",
            json!({}),
            vec![datum(42.0, 90.0)],
            "ALARM",
        ),
        (
            // … and it is the MOST RECENT datapoints of the range that are evaluated: the
            // older breaching one is not among the one datapoint this alarm evaluates.
            "a non-breaching datapoint newer than a breaching one",
            json!({}),
            vec![datum(1.0, 0.0), datum(42.0, 90.0)],
            "OK",
        ),
        (
            "a breaching datapoint older than the whole evaluation range",
            json!({}),
            vec![datum(42.0, 180.0)],
            "INSUFFICIENT_DATA",
        ),
        (
            "a breaching datapoint of the unit the alarm watches",
            json!({"Unit": "Count"}),
            vec![datum(42.0, 0.0).with(json!({"Unit": "Count"}))],
            "ALARM",
        ),
        (
            "a breaching datapoint of another unit than the alarm watches",
            json!({"Unit": "Count"}),
            vec![datum(42.0, 0.0).with(json!({"Unit": "Bytes"}))],
            "INSUFFICIENT_DATA",
        ),
        (
            // A publisher that declares no unit publishes under the value the enumerated type
            // declares for a measurement that has none, so the alarm selecting THAT value is
            // the one that grades the series (`crate::state::UNIT_NONE`).
            "a breaching datapoint with no declared unit, watched by the alarm naming None",
            json!({"Unit": "None"}),
            vec![datum(42.0, 0.0)],
            "ALARM",
        ),
        (
            "a breaching datapoint with no declared unit, watched by an alarm naming another",
            json!({"Unit": "Count"}),
            vec![datum(42.0, 0.0)],
            "INSUFFICIENT_DATA",
        ),
        (
            "a breaching datapoint declaring a unit, watched by the alarm naming None",
            json!({"Unit": "None"}),
            vec![datum(42.0, 0.0).with(json!({"Unit": "Count"}))],
            "INSUFFICIENT_DATA",
        ),
        (
            "a breaching datapoint of another dimension set than the alarm watches",
            json!({}),
            vec![datum(42.0, 0.0).with(json!({
                "Dimensions": [{"Name": "DomainName", "Value": "another-domain"}]
            }))],
            "INSUFFICIENT_DATA",
        ),
        (
            "a breaching datapoint of a metric the alarm does not name",
            json!({"MetricName": "SearchLatency"}),
            vec![datum(42.0, 0.0)],
            "INSUFFICIENT_DATA",
        ),
        (
            // An anomaly-detection band is compared against a model this emulator does not
            // compute, so the alarm keeps the state its creation left it in rather than
            // being answered with a state nothing evaluated.
            "an anomaly-detection band comparison",
            json!({
                "ComparisonOperator": "GreaterThanUpperThreshold",
                "Threshold": Value::Null,
                "ThresholdMetricId": "ad1",
            }),
            vec![datum(42.0, 0.0)],
            "INSUFFICIENT_DATA",
        ),
    ];

    // Every row's alarm watches its own series (its `DomainName` dimension is its own name),
    // so all of them are configured on ONE emulator and the evaluation delay is waited out
    // once rather than per row.
    let router = build_router();
    let mut names: Vec<String> = Vec::with_capacity(rows.len());
    for (index, (_, patch, data, _)) in rows.iter().enumerate() {
        let name = format!("alarm-{index}");
        ok(&router, "PutMetricAlarm", &patched(&name, patch)).await;
        publish(&router, &name, data).await;
        names.push(name);
    }
    wait_for_evaluation();

    for ((case, _, _, expected), name) in rows.iter().zip(&names) {
        let described = ok(&router, "DescribeAlarms", &json!({"AlarmNames": [name]})).await;
        let reported = &described["MetricAlarms"][0];
        assert_eq!(
            reported.get("StateValue").and_then(Value::as_str),
            Some(*expected),
            "case {case:?}: expected StateValue {expected}, got {reported}"
        );
    }
}

/// One row of the `StateReason` table.
struct ReasonRow {
    /// What this row is about, named by every assertion it makes.
    case: &'static str,
    /// The members this row changes on the base alarm.
    patch: Value,
    /// The datapoints it publishes into its own series.
    data: Vec<Datum>,
    /// The state the evaluation must put the alarm in.
    state: &'static str,
    /// The fragments its `StateReason` must carry.
    reported: Vec<&'static str>,
    /// The fragments its `StateReason` must NOT carry.
    not_reported: Vec<&'static str>,
}

/// One row of the `StateReason` table, in the order the table below reads.
fn reason_row(
    case: &'static str,
    patch: Value,
    data: Vec<Datum>,
    state: &'static str,
    reported: Vec<&'static str>,
    not_reported: Vec<&'static str>,
) -> ReasonRow {
    ReasonRow {
        case,
        patch,
        data,
        state,
        reported,
        not_reported,
    }
}

#[tokio::test]
async fn an_evaluated_state_reason_reports_an_empty_window_as_a_hole_and_names_the_arm() {
    // The rows differ in the missing-data arm in force, how many of the evaluated windows hold
    // a real datapoint, how many periods the alarm evaluates and whether the comparison held —
    // the things the rule generalises over. The recorded case the issue was filed about (one
    // evaluation period, `notBreaching`, no datapoint published at all) is the first row.
    let rows: Vec<ReasonRow> = vec![
        reason_row(
            "no datapoint at all over one period, missing data treated as not breaching",
            json!({"TreatMissingData": "notBreaching"}),
            Vec::new(),
            "OK",
            vec![
                "no datapoints were received for 1 period",
                "1 missing datapoint was treated as [NonBreaching]",
            ],
            vec!["out of the last"],
        ),
        reason_row(
            "no datapoint at all over three periods, missing data treated as not breaching",
            json!({"EvaluationPeriods": 3, "TreatMissingData": "notBreaching"}),
            Vec::new(),
            "OK",
            vec![
                "no datapoints were received for 3 periods",
                "3 missing datapoints were treated as [NonBreaching]",
            ],
            vec!["out of the last"],
        ),
        reason_row(
            "no datapoint at all, missing data treated as breaching",
            json!({"TreatMissingData": "breaching"}),
            Vec::new(),
            "ALARM",
            vec![
                "no datapoints were received for 1 period",
                "1 missing datapoint was treated as [Breaching]",
            ],
            vec!["out of the last", "NonBreaching"],
        ),
        reason_row(
            "one real non-breaching datapoint beside a window treated as not breaching",
            json!({"EvaluationPeriods": 2, "TreatMissingData": "notBreaching"}),
            vec![datum(1.0, 0.0)],
            "OK",
            vec![
                "2 out of the last 2 datapoints",
                "1 missing datapoint was treated as [NonBreaching]",
            ],
            vec!["no datapoints were received"],
        ),
        reason_row(
            "one real breaching datapoint beside a window treated as breaching",
            json!({"EvaluationPeriods": 2, "TreatMissingData": "breaching"}),
            vec![datum(42.0, 0.0)],
            "ALARM",
            vec![
                "2 out of the last 2 datapoints",
                "1 missing datapoint was treated as [Breaching]",
            ],
            vec!["no datapoints were received"],
        ),
        reason_row(
            "every evaluated datapoint is real and breaching",
            json!({}),
            vec![datum(42.0, 0.0)],
            "ALARM",
            vec!["Threshold Crossed: 1 out of the last 1 datapoints"],
            vec!["missing datapoint", "no datapoints were received"],
        ),
        reason_row(
            "every evaluated datapoint is real and none breaches",
            json!({}),
            vec![datum(1.0, 0.0)],
            "OK",
            vec![
                "Threshold Crossed: 1 out of the last 1 datapoints",
                "was not greater than the threshold (10.0)",
            ],
            vec![
                "Threshold not Crossed",
                "missing datapoint",
                "no datapoints were received",
            ],
        ),
        reason_row(
            // `missing` substitutes no datapoint, so the two real ones are all the evaluation
            // had — one short of this alarm's M and with no clean datapoint for the OK
            // direction, so the window decides nothing: the state the alarm reports is the one
            // its creation left it in, and so is the explanation of it.
            "two real breaching datapoints and a third window the default treatment leaves out",
            json!({"EvaluationPeriods": 3, "DatapointsToAlarm": 3}),
            vec![datum(42.0, 0.0), datum(43.0, 70.0)],
            "INSUFFICIENT_DATA",
            vec!["Unchecked: Initial alarm creation"],
            vec!["out of the last", "missing datapoint"],
        ),
        reason_row(
            // The same two real breaching datapoints with an M of two: the ALARM direction's
            // minimum IS reached, and the reason counts the datapoints it was reached over.
            "two real breaching datapoints and a third window left out, with two needed",
            json!({"EvaluationPeriods": 3, "DatapointsToAlarm": 2}),
            vec![datum(42.0, 0.0), datum(43.0, 70.0)],
            "ALARM",
            vec![
                "2 out of the last 2 datapoints",
                "(minimum 2 datapoints for OK -> ALARM transition).",
            ],
            vec!["missing datapoint", "no datapoints were received"],
        ),
    ];

    // Every row's alarm watches its own series, so all of them are configured on ONE emulator
    // and the evaluation delay is waited out once rather than per row.
    let router = build_router();
    let mut names: Vec<String> = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let name = format!("reason-{index}");
        ok(&router, "PutMetricAlarm", &patched(&name, &row.patch)).await;
        publish(&router, &name, &row.data).await;
        names.push(name);
    }
    wait_for_evaluation();

    for (row, name) in rows.iter().zip(&names) {
        let case = row.case;
        let state = row.state;
        let described = ok(&router, "DescribeAlarms", &json!({"AlarmNames": [name]})).await;
        let reported = &described["MetricAlarms"][0];
        assert_eq!(
            reported.get("StateValue").and_then(Value::as_str),
            Some(state),
            "case {case:?}: expected StateValue {state}, got {reported}"
        );
        let reason = reported
            .get("StateReason")
            .and_then(Value::as_str)
            .unwrap_or_default();
        for fragment in &row.reported {
            assert!(
                reason.contains(fragment),
                "case {case:?}: expected StateReason to report {fragment:?}, got {reason:?}"
            );
        }
        for fragment in &row.not_reported {
            assert!(
                !reason.contains(fragment),
                "case {case:?}: expected StateReason not to claim {fragment:?}, got {reason:?}"
            );
        }
        // `StateReason` is `@length 0..1023`
        // (`smithy_query(query="shape",shape="StateReason")`).
        assert!(
            reason.len() <= 1023,
            "case {case:?}: StateReason must stay inside its modelled length, got {} \
             characters: {reason:?}",
            reason.len()
        );
    }
}

#[tokio::test]
async fn an_evaluation_with_nothing_to_grade_reports_the_datapoints_it_did_not_have() {
    // (case, the alarm's `EvaluationPeriods`, the reason the evaluation must report once its
    //  datapoint has fallen out of the range)
    //
    // The shortest period an alarm may be evaluated on (`Period`: "Valid values are 10, 20,
    // 30, and any multiple of 60."), with each row's datapoint published close to the end of
    // the evaluation range its alarm searches — so the range it falls out of is short enough
    // to wait out: no datapoint is left in it, the default `missing` treatment substitutes
    // none, and the evaluation reports the datapoints it does not have instead of naming an
    // arm.
    // The rows differ in how many evaluation periods the alarm covers; each names an M of one,
    // so the single datapoint it publishes decides the alarm while it is in the range and the
    // reason reports the periods it has no datapoint for once that datapoint has aged out.
    let rows = [
        (
            "one evaluation period",
            1,
            "Insufficient Data: 1 datapoint was unknown.",
        ),
        (
            "two evaluation periods",
            2,
            "Insufficient Data: 2 datapoints were unknown.",
        ),
    ];

    let router = build_router();
    let mut names: Vec<String> = Vec::with_capacity(rows.len());
    // A period is anchored at a multiple of the period, so the boundary that carries a
    // datapoint out of the range falls at a known instant: publishing just after one boundary
    // leaves each row's datapoint inside its range for the reading below, and the poll that
    // follows ends on the state the boundary after it leaves the alarm in.
    wait_for_period_start(f64::from(SHORTEST_PERIOD_SECS)).await;
    for (index, (_, periods, _)) in rows.iter().enumerate() {
        let name = format!("aged-out-{index}");
        let alarm = patched(
            &name,
            &json!({
                "Period": SHORTEST_PERIOD_SECS,
                "EvaluationPeriods": periods,
                "DatapointsToAlarm": 1,
            }),
        );
        ok(&router, "PutMetricAlarm", &alarm).await;
        // The OLDEST period the alarm's evaluation range still searches: the range covers the
        // wider of `EVALUATION_RANGE_PERIODS` periods per evaluation period and the windows one
        // minimum range span takes ([`evaluation_range_periods`]), counting back from the newest
        // period that has closed, so the oldest of them is that many periods less one behind it.
        let age = f64::from(evaluation_range_periods(*periods, SHORTEST_PERIOD_SECS) - 1)
            * f64::from(SHORTEST_PERIOD_SECS);
        publish_in_period(
            &router,
            &name,
            f64::from(SHORTEST_PERIOD_SECS),
            &[datum(42.0, age)],
        )
        .await;
        names.push(name);
    }
    wait_for_evaluation();

    // The datapoint is still in the range, so each alarm is evaluated into ALARM first.
    for ((case, _, _), name) in rows.iter().zip(&names) {
        let described = ok(&router, "DescribeAlarms", &json!({"AlarmNames": [name]})).await;
        assert_eq!(
            described["MetricAlarms"][0]
                .get("StateValue")
                .and_then(Value::as_str),
            Some("ALARM"),
            "case {case:?}: the breaching datapoint must put the alarm in ALARM, got {described}"
        );
    }

    // At the next period boundary every row's datapoint is a period older than the oldest period
    // its alarm searches, so every period the evaluation looks at is empty. That boundary is the
    // CLOCK's move, so each row looks again until the alarm reports the state no datapoint in its
    // range leaves it in ([`awaited_state`]) rather than waiting out a span.
    for ((case, _, expected), name) in rows.iter().zip(&names) {
        let reported = awaited_state(&router, name, "INSUFFICIENT_DATA").await;
        assert_eq!(
            reported.get("StateValue").and_then(Value::as_str),
            Some("INSUFFICIENT_DATA"),
            "case {case:?}: with no datapoint left in the range the alarm must report \
             INSUFFICIENT_DATA, got {reported}"
        );
        let reason = reported
            .get("StateReason")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            reason, *expected,
            "case {case:?}: expected StateReason {expected:?}, got {reason:?}"
        );
    }
}

/// Look again at the alarm `name` until `DescribeAlarms` reports `wanted`, answering the alarm it
/// last read.
///
/// What such a wait is for is a move the CLOCK makes — a datapoint leaving the evaluation range at
/// a period boundary, and the evaluation that follows it — so it waits for the CONDITION rather
/// than for a span of the clock. The deadline is generous, and is only there so a state that never
/// arrives fails the row on the value it last read instead of hanging it.
async fn awaited_state(router: &MultiServiceRouter, name: &str, wanted: &str) -> Value {
    /// How long one such wait keeps looking, and how long it leaves between looks.
    const DEADLINE: Duration = Duration::from_secs(90);
    const LOOK_AGAIN_AFTER: Duration = Duration::from_millis(100);

    let giving_up_at = Instant::now() + DEADLINE;
    loop {
        let described = ok(router, "DescribeAlarms", &json!({"AlarmNames": [name]})).await;
        let reported = described["MetricAlarms"][0].clone();
        if reported.get("StateValue").and_then(Value::as_str) == Some(wanted)
            || Instant::now() >= giving_up_at
        {
            return reported;
        }
        tokio::time::sleep(LOOK_AGAIN_AFTER).await;
    }
}

#[tokio::test]
async fn an_evaluated_transition_is_recorded_in_the_alarm_history() {
    let router = build_router();
    let alarm = base_alarm("searchable-documents-high");
    ok(&router, "PutMetricAlarm", &alarm).await;
    publish(&router, "searchable-documents-high", &[datum(42.0, 0.0)]).await;
    wait_for_evaluation();

    let described = ok(
        &router,
        "DescribeAlarms",
        &json!({"AlarmNames": ["searchable-documents-high"]}),
    )
    .await;
    let reported = &described["MetricAlarms"][0];
    assert_eq!(
        reported.get("StateValue").and_then(Value::as_str),
        Some("ALARM"),
        "the evaluation must move the alarm into ALARM, got {described}"
    );
    let reason = reported
        .get("StateReason")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        reason.contains("1 out of the last 1 datapoints"),
        "the state reason must report what the evaluation counted, got {reason:?}"
    );
    let reason_data: Value = serde_json::from_str(
        reported
            .get("StateReasonData")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    )
    .expect("StateReasonData is JSON");
    assert_eq!(
        reason_data["evaluatedDatapoints"][0]["value"].as_f64(),
        Some(42.0),
        "the reason data must carry the datapoint that was evaluated, got {reason_data}"
    );

    let history = ok(
        &router,
        "DescribeAlarmHistory",
        &json!({"AlarmName": "searchable-documents-high", "HistoryItemType": "StateUpdate"}),
    )
    .await;
    let items = history["AlarmHistoryItems"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        items.len(),
        1,
        "the transition must record exactly one StateUpdate item, got {history}"
    );
    let data: Value = serde_json::from_str(
        items[0]
            .get("HistoryData")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    )
    .expect("HistoryData is JSON");
    assert_eq!(
        data["oldState"]["stateValue"].as_str(),
        Some("INSUFFICIENT_DATA"),
        "the item must record the state the alarm moved out of, got {data}"
    );
    assert_eq!(
        data["newState"]["stateValue"].as_str(),
        Some("ALARM"),
        "the item must record the state the alarm moved into, got {data}"
    );
}

#[tokio::test]
async fn a_forced_state_holds_until_the_next_evaluation_is_due() {
    let router = build_router();
    // The shortest period an alarm may be evaluated on (`Period`: "Valid values are 10, 20, 30,
    // and any multiple of 60."), so the datapoint published below lands in the window the
    // evaluation that reclaims the state grades.
    let alarm = patched("forced", &json!({"Period": SHORTEST_PERIOD_SECS}));
    ok(&router, "PutMetricAlarm", &alarm).await;
    ok(
        &router,
        "SetAlarmState",
        &json!({"AlarmName": "forced", "StateValue": "ALARM", "StateReason": "testing"}),
    )
    .await;

    let described = ok(&router, "DescribeAlarms", &json!({"AlarmNames": ["forced"]})).await;
    assert_eq!(
        described["MetricAlarms"][0]
            .get("StateValue")
            .and_then(Value::as_str),
        Some("ALARM"),
        "a forced state must be visible to the caller that reads straight after it, got \
         {described}"
    );

    // The state a caller forces stands for its own span, whatever the series the alarm
    // watches holds: a datapoint arriving meanwhile does not cut it short — it is what the
    // evaluation at the end of that span grades. "Metric alarms returns to their actual state
    // quickly, often within seconds." (`SetAlarmState`) — a datapoint under the threshold puts
    // the alarm in OK once the span has passed.
    publish_in_period(
        &router,
        "forced",
        f64::from(SHORTEST_PERIOD_SECS),
        &[datum(1.0, 0.0)],
    )
    .await;
    let still_forced = ok(&router, "DescribeAlarms", &json!({"AlarmNames": ["forced"]})).await;
    assert_eq!(
        still_forced["MetricAlarms"][0]
            .get("StateValue")
            .and_then(Value::as_str),
        Some("ALARM"),
        "the datapoint must not cut the forced span short, got {still_forced}"
    );
    crate::harness::wait_for_forced_state_reclaim_of(f64::from(SHORTEST_PERIOD_SECS));
    let described = ok(&router, "DescribeAlarms", &json!({"AlarmNames": ["forced"]})).await;
    assert_eq!(
        described["MetricAlarms"][0]
            .get("StateValue")
            .and_then(Value::as_str),
        Some("OK"),
        "once the forced span has passed, the evaluation must take the state back, got \
         {described}"
    );
}

#[tokio::test]
async fn ignored_missing_data_maintains_the_state_but_a_datapoint_still_decides_it() {
    let router = build_router();
    // The shortest period an alarm may be evaluated on, so the evaluation that takes the forced
    // state back runs with the look-back window holding no datapoint at all.
    let alarm = patched(
        "ignoring",
        &json!({"Period": SHORTEST_PERIOD_SECS, "TreatMissingData": "ignore"}),
    );
    ok(&router, "PutMetricAlarm", &alarm).await;
    ok(
        &router,
        "SetAlarmState",
        &json!({"AlarmName": "ignoring", "StateValue": "ALARM", "StateReason": "testing"}),
    )
    .await;
    // Past the span the forced state stands for, so the evaluation has taken the alarm back.
    crate::harness::wait_for_forced_state_reclaim();

    // "ignore — the current alarm state is maintained": with no datapoint to decide on, the
    // evaluation leaves the alarm in the state it has.
    let described = ok(&router, "DescribeAlarms", &json!({"AlarmNames": ["ignoring"]})).await;
    assert_eq!(
        described["MetricAlarms"][0]
            .get("StateValue")
            .and_then(Value::as_str),
        Some("ALARM"),
        "with every datapoint missing and TreatMissingData=ignore, the state must be \
         maintained, got {described}"
    );

    // A datapoint that IS present decides the state, whatever the missing-data setting — at
    // the next evaluation, which the read above re-armed a span away.
    publish_in_period(
        &router,
        "ignoring",
        f64::from(SHORTEST_PERIOD_SECS),
        &[datum(1.0, 0.0)],
    )
    .await;
    wait_for_evaluation();
    let described = ok(&router, "DescribeAlarms", &json!({"AlarmNames": ["ignoring"]})).await;
    assert_eq!(
        described["MetricAlarms"][0]
            .get("StateValue")
            .and_then(Value::as_str),
        Some("OK"),
        "a datapoint under the threshold must decide the state even under \
         TreatMissingData=ignore, got {described}"
    );
}

#[tokio::test]
async fn the_alarms_of_one_metric_are_reported_with_the_state_the_evaluation_computed() {
    // The dimension set the matching alarms watch, and the one series a datapoint is
    // published into.
    let domain = "described-for-metric";
    let dimensions = json!([{"Name": "DomainName", "Value": domain}]);
    let other_dimensions = json!([{"Name": "DomainName", "Value": "another-domain"}]);

    let router = build_router();
    // The alarms this metric carries, each differing in exactly one of the members
    // `DescribeAlarmsForMetric` selects on.
    for (name, patch) in [
        ("average-60", json!({})),
        ("maximum-60", json!({"Statistic": "Maximum"})),
        ("average-300", json!({"Period": 300})),
        ("count-unit", json!({"Unit": "Count"})),
        (
            "p90",
            json!({"Statistic": Value::Null, "ExtendedStatistic": "p90"}),
        ),
        ("another-metric", json!({"MetricName": "SearchLatency"})),
        ("another-dimension-set", json!({"Dimensions": other_dimensions})),
        (
            // "It does not return alarms based on math expressions that use the specified
            // metric" — an alarm carrying a `Metrics` query array is one of those.
            "metric-math",
            json!({
                "Statistic": Value::Null,
                "Namespace": Value::Null,
                "MetricName": Value::Null,
                "Dimensions": Value::Null,
                "Period": Value::Null,
                "Metrics": [{
                    "Id": "m1",
                    "MetricStat": {
                        "Metric": {
                            "Namespace": NAMESPACE,
                            "MetricName": METRIC,
                            "Dimensions": dimensions,
                        },
                        "Period": 60,
                        "Stat": "Average",
                    },
                    "ReturnData": true,
                }],
            }),
        ),
    ] {
        let mut alarm = patched(name, &patch);
        if alarm.get("Dimensions").is_some() && name != "another-dimension-set" {
            alarm["Dimensions"] = dimensions.clone();
        }
        ok(&router, "PutMetricAlarm", &alarm).await;
    }
    // One breaching datapoint of the series the matching alarms watch.
    publish(&router, domain, &[datum(42.0, 0.0)]).await;
    wait_for_evaluation();

    // (case, the query, the alarms it must report, newest first by name)
    let rows: &[(&str, Value, &[&str])] = &[
        (
            "every standard alarm of the metric",
            json!({"Namespace": NAMESPACE, "MetricName": METRIC, "Dimensions": dimensions}),
            &["average-300", "average-60", "count-unit", "maximum-60", "p90"],
        ),
        (
            "filtered by statistic",
            json!({
                "Namespace": NAMESPACE,
                "MetricName": METRIC,
                "Dimensions": dimensions,
                "Statistic": "Maximum",
            }),
            &["maximum-60"],
        ),
        (
            "filtered by extended statistic",
            json!({
                "Namespace": NAMESPACE,
                "MetricName": METRIC,
                "Dimensions": dimensions,
                "ExtendedStatistic": "p90",
            }),
            &["p90"],
        ),
        (
            "filtered by period",
            json!({
                "Namespace": NAMESPACE,
                "MetricName": METRIC,
                "Dimensions": dimensions,
                "Period": 300,
            }),
            &["average-300"],
        ),
        (
            "filtered by unit",
            json!({
                "Namespace": NAMESPACE,
                "MetricName": METRIC,
                "Dimensions": dimensions,
                "Unit": "Count",
            }),
            &["count-unit"],
        ),
        (
            "another dimension set names another metric",
            json!({
                "Namespace": NAMESPACE,
                "MetricName": METRIC,
                "Dimensions": other_dimensions,
            }),
            &["another-dimension-set"],
        ),
        (
            "no dimensions at all names the dimensionless metric",
            json!({"Namespace": NAMESPACE, "MetricName": METRIC}),
            &[],
        ),
        (
            "another metric of the same namespace",
            json!({
                "Namespace": NAMESPACE,
                "MetricName": "SearchLatency",
                "Dimensions": dimensions,
            }),
            &["another-metric"],
        ),
        (
            "a metric no alarm watches",
            json!({
                "Namespace": NAMESPACE,
                "MetricName": "IndexingLatency",
                "Dimensions": dimensions,
            }),
            &[],
        ),
        (
            "a period no alarm of the metric uses",
            json!({
                "Namespace": NAMESPACE,
                "MetricName": METRIC,
                "Dimensions": dimensions,
                "Period": 10,
            }),
            &[],
        ),
    ];

    for (case, query, expected) in rows {
        let reported = ok(&router, "DescribeAlarmsForMetric", query).await;
        let names: Vec<String> = reported["MetricAlarms"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|alarm| {
                alarm
                    .get("AlarmName")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            names,
            expected.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "case {case:?}: expected exactly those alarms, got {reported}"
        );
    }

    // The state this reader reports is the EVALUATED one, the same state `DescribeAlarms`
    // reports: which call an alarm is read through cannot change the state a client is told.
    let for_metric = ok(
        &router,
        "DescribeAlarmsForMetric",
        &json!({
            "Namespace": NAMESPACE,
            "MetricName": METRIC,
            "Dimensions": dimensions,
            "Statistic": "Average",
            "Period": 60,
        }),
    )
    .await;
    let described = ok(&router, "DescribeAlarms", &json!({"AlarmNames": ["average-60"]})).await;
    assert_eq!(
        for_metric["MetricAlarms"][0]
            .get("StateValue")
            .and_then(Value::as_str),
        Some("ALARM"),
        "the crossing datapoint must be reported through this reader too, got {for_metric}"
    );
    assert_eq!(
        for_metric["MetricAlarms"][0].get("StateValue"),
        described["MetricAlarms"][0].get("StateValue"),
        "both readers must report one state, got {for_metric} and {described}"
    );
}

#[tokio::test]
async fn a_publish_into_the_reserved_namespace_is_refused_and_stores_nothing() {
    // (case, the namespace published into, whether the publish is accepted)
    //
    // The reserved space is the one `PutMetricDataInput$Namespace` names: "To avoid conflicts
    // with Amazon Web Services service namespaces, you should not specify a namespace that
    // begins with AWS/". The space a namespace names does not change with its spelling's
    // case, so every case of the prefix is inside it; the accepted rows are the ones just
    // outside it.
    let rows: &[(&str, &str, bool)] = &[
        ("the namespace of the recorded case", "AWS/ES", false),
        ("a namespace of another AWS service", "AWS/EC2", false),
        ("a namespace of a third AWS service", "AWS/Lambda", false),
        ("the bare reserved prefix", "AWS/", false),
        ("a deeper reserved namespace", "AWS/ApplicationELB/Target", false),
        ("the reserved prefix in lower case", "aws/es", false),
        ("the reserved prefix in mixed case", "Aws/Es", false),
        ("a customer namespace", "MyApplication", true),
        ("the reserved prefix without its separator", "AWSMyApp", true),
        ("the reserved prefix inside a namespace", "MyApp/AWS/Metrics", true),
        ("a namespace that only starts with the letter A", "A/WS", true),
    ];

    for (case, namespace, accepted) in rows {
        let router = build_router();
        let request = json!({
            "Namespace": namespace,
            "MetricData": [{
                "MetricName": METRIC,
                "Dimensions": [
                    {"Name": "ClientId", "Value": "000000000000"},
                    {"Name": "DomainName", "Value": "obs-opensearch-2"},
                ],
                "Value": 42.0,
            }],
        });
        let (status, body) = call(&router, "PutMetricData", &request).await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: {namespace} must be accepted, got {status} with {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: {namespace} must be refused, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidParameterValueException",
                "case {case:?}: expected the parameter-value fault, got {body}"
            );
        }

        // Nothing is stored for a refused publish, and exactly the one series for an
        // accepted one: the refusal happens before any datapoint is written.
        let listed = ok(&router, "ListMetrics", &json!({"Namespace": namespace})).await;
        let metrics = listed["Metrics"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            metrics.len(),
            usize::from(*accepted),
            "case {case:?}: expected {} stored series, got {listed}",
            usize::from(*accepted)
        );
    }
}

#[tokio::test]
async fn an_alarm_that_crosses_its_threshold_notifies_its_sns_action() {
    let emulator = emulator();

    // A queue subscribed to a topic, the path an alarm notification travels.
    let queue = rpc(
        &emulator,
        "sqs",
        "AmazonSQS",
        "CreateQueue",
        &json!({"QueueName": "alarm-notifications"}),
    )
    .await;
    let queue_url = queue["QueueUrl"]
        .as_str()
        .expect("CreateQueue answers a QueueUrl")
        .to_string();
    let attributes = rpc(
        &emulator,
        "sqs",
        "AmazonSQS",
        "GetQueueAttributes",
        &json!({"QueueUrl": queue_url, "AttributeNames": ["QueueArn"]}),
    )
    .await;
    let queue_arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .expect("GetQueueAttributes answers the QueueArn")
        .to_string();
    // The queue policy an SNS subscriber needs: a notification is an `sqs:SendMessage` the
    // queue's own policy has to grant the SNS service principal, so a queue that grants it
    // nothing receives nothing (`sns_action_queue_policy.rs`).
    rpc(
        &emulator,
        "sqs",
        "AmazonSQS",
        "SetQueueAttributes",
        &json!({
            "QueueUrl": queue_url,
            "Attributes": {"Policy": grant_sns_delivery(&queue_arn)},
        }),
    )
    .await;
    let topic_arn = element(&sns(&emulator, "CreateTopic", &[("Name", "alarms")]).await, "TopicArn");
    sns(
        &emulator,
        "Subscribe",
        &[
            ("TopicArn", &topic_arn),
            ("Protocol", "sqs"),
            ("Endpoint", &queue_arn),
        ],
    )
    .await;

    // An alarm on a customer metric, with the topic as its ALARM action.
    let mut alarm = base_alarm("searchable-documents-high");
    alarm["AlarmActions"] = json!([topic_arn]);
    alarm["AlarmDescription"] = json!("the searchable document count crossed its threshold");
    rpc(
        &emulator,
        "monitoring",
        "GraniteServiceVersion20100801",
        "PutMetricAlarm",
        &alarm,
    )
    .await;

    // The crossing datapoint, published the way the metric's publisher publishes it.
    wait_for_evaluation();
    rpc(
        &emulator,
        "monitoring",
        "GraniteServiceVersion20100801",
        "PutMetricData",
        &json!({
            "Namespace": NAMESPACE,
            "MetricData": [
                datum(4_200.0, 0.0)
                    .rendered(graded_window_stamp(BASE_PERIOD_SECS), "searchable-documents-high")
            ],
        }),
    )
    .await;

    let described = rpc(
        &emulator,
        "monitoring",
        "GraniteServiceVersion20100801",
        "DescribeAlarms",
        &json!({"AlarmNames": ["searchable-documents-high"]}),
    )
    .await;
    assert_eq!(
        described["MetricAlarms"][0]
            .get("StateValue")
            .and_then(Value::as_str),
        Some("ALARM"),
        "the alarm must reach ALARM once the published datapoint crosses its threshold, got \
         {described}"
    );

    // The notification the transition fired, where the subscriber sees it.
    let received = rpc(
        &emulator,
        "sqs",
        "AmazonSQS",
        "ReceiveMessage",
        &json!({"QueueUrl": queue_url, "MaxNumberOfMessages": 10}),
    )
    .await;
    let messages = received["Messages"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        messages.len(),
        1,
        "the alarm's SNS action must deliver exactly one notification to the subscribed \
         queue, got {received}"
    );
    let envelope: Value = serde_json::from_str(
        messages[0]
            .get("Body")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    )
    .expect("the delivered body is the SNS notification envelope");
    assert_eq!(
        envelope["TopicArn"].as_str(),
        Some(topic_arn.as_str()),
        "the envelope must name the topic the alarm published to, got {envelope}"
    );
    let notification: Value = serde_json::from_str(
        envelope["Message"]
            .as_str()
            .expect("the envelope carries the published message"),
    )
    .expect("the alarm notification is JSON");
    assert_eq!(
        notification["AlarmName"].as_str(),
        Some("searchable-documents-high"),
        "the notification must name the alarm, got {notification}"
    );
    assert_eq!(
        notification["NewStateValue"].as_str(),
        Some("ALARM"),
        "the notification must carry the state the alarm moved to, got {notification}"
    );
    assert_eq!(
        notification["OldStateValue"].as_str(),
        Some("INSUFFICIENT_DATA"),
        "the notification must carry the state the alarm moved from, got {notification}"
    );
    assert_eq!(
        notification["Trigger"]["MetricName"].as_str(),
        Some(METRIC),
        "the notification must name the metric that was evaluated, got {notification}"
    );

    // And the alarm's own history records that the action ran.
    let history = rpc(
        &emulator,
        "monitoring",
        "GraniteServiceVersion20100801",
        "DescribeAlarmHistory",
        &json!({"AlarmName": "searchable-documents-high", "HistoryItemType": "Action"}),
    )
    .await;
    let items = history["AlarmHistoryItems"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        items.len(),
        1,
        "executing the one action must record one Action item, got {history}"
    );
    let summary = items[0]
        .get("HistorySummary")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        summary.contains(&topic_arn) && summary.starts_with("Successfully executed action"),
        "the Action item must name the action that ran, got {summary:?}"
    );
}
