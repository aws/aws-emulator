// SPDX-License-Identifier: CC0-1.0

//! A metric alarm's state while a client's publishing is PAUSED, end to end over the wire.
//!
//! # The rule under test
//!
//! > An evaluation grades the most recent datapoints the series really holds inside the
//! > EVALUATION RANGE — a span of clock wider than the look-back, whose width follows the
//! > alarm's period and the resolution its series is stored at — so an alarm whose newest
//! > non-missing datapoint still stands against its threshold keeps the state that datapoint
//! > decided while nothing is published, and the missing-data treatment decides only the
//! > periods no real datapoint of the range stands for.
//!
//! CloudWatch "attempts to retrieve a higher number of data points than the number specified as
//! Evaluation Periods", "the exact number of data points it attempts to retrieve depends on the
//! length of the alarm period and whether it is based on a metric with standard resolution or high
//! resolution", and "all real data points in the evaluation range are included in the evaluation"
//! — so a pause in publishing is not data: `TreatMissingData` ("Sets how this alarm is to handle
//! missing data points … Valid Values: `breaching | notBreaching | ignore | missing`",
//! `PutMetricAlarmInput$TreatMissingData`) stands in only where the range holds no datapoint at
//! all, and a latched `ALARM` is left only by data that contradicts it.
//!
//! # What the rows span
//!
//! Every row publishes into a series of its own and reads `DescribeAlarms`, so what is asserted
//! is the `StateValue` the evaluation computed from stored datapoints. They differ along what the
//! rule generalises over: how long ago the newest datapoint was published (just now, one minute,
//! two minutes, five minutes), the alarm's `Period` (10, 30 and 60 seconds — "Valid values are 10,
//! 20, 30, and any multiple of 60", `PutMetricAlarmInput$Period`), the `StorageResolution` the
//! series is stored at (1 and 60), the comparison direction, the missing-data arm (`notBreaching`,
//! `breaching`, `missing`) and whether a contradicting datapoint arrives after the breach.
//!
//! Nothing here waits out a span of the clock: each row stamps its datapoints in the windows its
//! own alarm grades ([`crate::harness::graded_window_stamp`]), which is how a pause of any length
//! is driven without sleeping through it.

use serde_json::{json, Value};

use crate::harness::{build_router, graded_window_stamp, ok, wait_for_evaluation};

/// The customer namespace every row below publishes into.
const NAMESPACE: &str = "Custom/PausedPublishing";

/// The metric every row's alarm watches; the rows are told apart by their `Series` dimension.
const METRIC: &str = "CooldownDriver";

/// The threshold every row's alarm carries, and the two values the rows publish: one that stands
/// against a `LessThanThreshold` threshold and one that stands against a `GreaterThanThreshold`
/// one.
const THRESHOLD: f64 = 50.0;
const BELOW: f64 = 45.0;
const ABOVE: f64 = 80.0;

/// One datapoint a row publishes: its value and how many SECONDS before the newest window its
/// alarm's evaluation grades it is stamped.
#[derive(Debug, Clone, Copy)]
struct Datum {
    value: f64,
    age_secs: f64,
}

/// One row: the alarm's shape, the series it reads and the state the evaluation must compute.
struct Row {
    case: &'static str,
    /// The alarm's `Period`, in seconds.
    period: i64,
    /// The `StorageResolution` the row's datapoints are published with.
    storage_resolution: i64,
    /// The alarm's `ComparisonOperator`.
    comparison: &'static str,
    /// The alarm's `TreatMissingData`.
    treat_missing: &'static str,
    /// What the row publishes, newest last.
    data: &'static [Datum],
    /// The `StateValue` the evaluation must report.
    expected: &'static str,
}

/// A datapoint of `value` stamped `age_secs` before the newest graded window.
const fn datum(value: f64, age_secs: f64) -> Datum {
    Datum { value, age_secs }
}

/// How long a pause the rows drive, in seconds: a minute of silence (the span a client that stops
/// publishing leaves behind), two minutes, and one long enough that the newest datapoint has left
/// the evaluation range of every period below.
const ONE_MINUTE: f64 = 60.0;
const TWO_MINUTES: f64 = 120.0;
const PAST_THE_RANGE: f64 = 600.0;

const ROWS: &[Row] = &[
    Row {
        case: "a breaching datapoint just published, on ten-second periods",
        period: 10,
        storage_resolution: 1,
        comparison: "LessThanThreshold",
        treat_missing: "notBreaching",
        data: &[datum(BELOW, 0.0)],
        expected: "ALARM",
    },
    Row {
        case: "nothing published for a minute after the breach, on ten-second periods",
        period: 10,
        storage_resolution: 1,
        comparison: "LessThanThreshold",
        treat_missing: "notBreaching",
        data: &[datum(BELOW, ONE_MINUTE)],
        expected: "ALARM",
    },
    Row {
        case: "nothing published for two minutes after the breach, on ten-second periods",
        period: 10,
        storage_resolution: 1,
        comparison: "LessThanThreshold",
        treat_missing: "notBreaching",
        data: &[datum(BELOW, TWO_MINUTES)],
        expected: "ALARM",
    },
    Row {
        case: "the breach is older than the whole evaluation range, so notBreaching decides",
        period: 10,
        storage_resolution: 1,
        comparison: "LessThanThreshold",
        treat_missing: "notBreaching",
        data: &[datum(BELOW, PAST_THE_RANGE)],
        expected: "OK",
    },
    Row {
        case: "the breach is older than the whole range under the missing arm",
        period: 10,
        storage_resolution: 1,
        comparison: "LessThanThreshold",
        treat_missing: "missing",
        data: &[datum(BELOW, PAST_THE_RANGE)],
        expected: "INSUFFICIENT_DATA",
    },
    Row {
        case: "a minute of silence on thirty-second periods",
        period: 30,
        storage_resolution: 1,
        comparison: "LessThanThreshold",
        treat_missing: "notBreaching",
        data: &[datum(BELOW, ONE_MINUTE)],
        expected: "ALARM",
    },
    Row {
        case: "a minute of silence on sixty-second periods, stored at regular resolution",
        period: 60,
        storage_resolution: 60,
        comparison: "LessThanThreshold",
        treat_missing: "notBreaching",
        data: &[datum(BELOW, ONE_MINUTE)],
        expected: "ALARM",
    },
    Row {
        case: "sixty-second periods with the breach older than the range",
        period: 60,
        storage_resolution: 60,
        comparison: "LessThanThreshold",
        treat_missing: "notBreaching",
        data: &[datum(BELOW, PAST_THE_RANGE)],
        expected: "OK",
    },
    Row {
        case: "a minute of silence on ten-second periods over a regular-resolution series",
        period: 10,
        storage_resolution: 60,
        comparison: "LessThanThreshold",
        treat_missing: "notBreaching",
        data: &[datum(BELOW, ONE_MINUTE)],
        expected: "ALARM",
    },
    Row {
        case: "a contradicting datapoint published after the breach",
        period: 10,
        storage_resolution: 1,
        comparison: "LessThanThreshold",
        treat_missing: "notBreaching",
        data: &[datum(BELOW, ONE_MINUTE), datum(ABOVE, 0.0)],
        expected: "OK",
    },
    Row {
        case: "a minute of silence after a datapoint that breaches the other direction",
        period: 10,
        storage_resolution: 1,
        comparison: "GreaterThanThreshold",
        treat_missing: "notBreaching",
        data: &[datum(ABOVE, ONE_MINUTE)],
        expected: "ALARM",
    },
    Row {
        case: "a minute of silence after a non-breaching datapoint, under the breaching arm",
        period: 10,
        storage_resolution: 1,
        comparison: "GreaterThanThreshold",
        treat_missing: "breaching",
        data: &[datum(BELOW, ONE_MINUTE)],
        expected: "OK",
    },
];

/// The `Series` dimension that tells one row's series from another's.
fn series(case: &str) -> Value {
    json!([{ "Name": "Series", "Value": case }])
}

/// Put the alarm one row is about: one evaluation period and one datapoint to alarm, so the
/// newest datapoint the range holds is what decides it.
async fn put_alarm(router: &::smithy_server_runtime::MultiServiceRouter, row: &Row, name: &str) {
    ok(
        router,
        "PutMetricAlarm",
        &json!({
            "AlarmName": name,
            "Namespace": NAMESPACE,
            "MetricName": METRIC,
            "Dimensions": series(row.case),
            "Statistic": "Maximum",
            "Period": row.period,
            "EvaluationPeriods": 1,
            "DatapointsToAlarm": 1,
            "Threshold": THRESHOLD,
            "ComparisonOperator": row.comparison,
            "TreatMissingData": row.treat_missing,
        }),
    )
    .await;
}

/// Publish one row's datapoints, each stamped in the window its own alarm grades.
async fn publish(router: &::smithy_server_runtime::MultiServiceRouter, row: &Row) {
    #[allow(clippy::cast_precision_loss)]
    let base = graded_window_stamp(row.period as f64);
    let rendered: Vec<Value> = row
        .data
        .iter()
        .map(|datum| {
            json!({
                "MetricName": METRIC,
                "Dimensions": series(row.case),
                "Value": datum.value,
                "StorageResolution": row.storage_resolution,
                "Timestamp": base - datum.age_secs,
            })
        })
        .collect();
    ok(
        router,
        "PutMetricData",
        &json!({ "Namespace": NAMESPACE, "MetricData": rendered }),
    )
    .await;
}

#[tokio::test]
async fn an_alarm_keeps_what_its_newest_non_missing_datapoint_decided_while_nothing_publishes() {
    let router = build_router();
    let mut names: Vec<String> = Vec::with_capacity(ROWS.len());
    for (index, row) in ROWS.iter().enumerate() {
        let name = format!("paused-{index}");
        put_alarm(&router, row, &name).await;
        publish(&router, row).await;
        names.push(name);
    }
    wait_for_evaluation();

    for (row, name) in ROWS.iter().zip(&names) {
        let described = ok(&router, "DescribeAlarms", &json!({"AlarmNames": [name]})).await;
        let reported = &described["MetricAlarms"][0];
        let state = reported.get("StateValue").and_then(Value::as_str);
        assert_eq!(
            state,
            Some(row.expected),
            "case {:?}: expected StateValue {}, got {state:?} in {reported}",
            row.case,
            row.expected
        );
    }
}
