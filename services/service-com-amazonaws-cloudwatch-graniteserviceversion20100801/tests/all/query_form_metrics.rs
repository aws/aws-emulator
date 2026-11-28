// SPDX-License-Identifier: CC0-1.0

//! A publish into the AWS-RESERVED namespace is refused in EVERY form a datum may take, and
//! nothing a client reads afterwards shows it.
//!
//! The rule under test: **`PutMetricData` refuses a `Namespace` in the AWS-reserved namespace
//! space whatever form the request carries its observations in, and refuses it before
//! anything is stored** — so the series is absent from the metric catalogue, no datapoint can
//! be aggregated out of it, and an alarm that watches that series is not driven by the
//! refused publish: its state is the one its creation left it in, and it records no state
//! transition and therefore fires no action.
//!
//! `MetricDatum` offers four ways to carry observations and `PutMetricData` two lists to
//! carry data in, so the rows differ along exactly that: `Value`, `Values`, `Values` with
//! `Counts`, `StatisticValues`, a datum carrying no observation at all, and the same datum
//! moved from `MetricData` into `EntityMetricData`. The refusal is a property of the
//! NAMESPACE, so every row is refused the same way.

use serde_json::{json, Value};

use crate::harness::{build_router, call, error_code, ok, wait_for_evaluation};

/// Wall-clock now as epoch seconds, the clock a query's time range is measured against.
fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}

/// The reserved namespace the rows below publish into: the one the recorded case named.
const RESERVED: &str = "AWS/ES";

/// The metric every row publishes.
const METRIC: &str = "SearchableDocuments";

/// The dimension set every row publishes, the one an OpenSearch domain's metrics carry.
fn dimensions() -> Value {
    json!([
        {"Name": "ClientId", "Value": "000000000000"},
        {"Name": "DomainName", "Value": "forged-domain"},
    ])
}

/// One `MetricDatum` of the series above, carrying `observations`.
fn datum(observations: Value) -> Value {
    let mut datum = json!({"MetricName": METRIC, "Dimensions": dimensions()});
    for (member, value) in observations.as_object().cloned().unwrap_or_default() {
        datum[member] = value;
    }
    datum
}

#[tokio::test]
async fn every_publish_form_into_the_reserved_namespace_is_refused_and_stores_nothing() {
    // (case, the whole PutMetricData input the row sends)
    let rows: Vec<(&str, Value)> = vec![
        (
            "a plain Value",
            json!({"Namespace": RESERVED, "MetricData": [datum(json!({"Value": 4200.0}))]}),
        ),
        (
            "a Values array",
            json!({
                "Namespace": RESERVED,
                "MetricData": [datum(json!({"Values": [1.0, 2.0, 4200.0]}))],
            }),
        ),
        (
            "a Values array weighted by Counts",
            json!({
                "Namespace": RESERVED,
                "MetricData": [datum(json!({"Values": [4200.0], "Counts": [7.0]}))],
            }),
        ),
        (
            "a StatisticValues set",
            json!({
                "Namespace": RESERVED,
                "MetricData": [datum(json!({"StatisticValues": {
                    "SampleCount": 2.0, "Sum": 8400.0, "Minimum": 4200.0, "Maximum": 4200.0,
                }}))],
            }),
        ),
        (
            "a datum carrying no observation at all, which would still create the series",
            json!({"Namespace": RESERVED, "MetricData": [datum(json!({}))]}),
        ),
        (
            "the same datum carried as entity metric data",
            json!({
                "Namespace": RESERVED,
                "EntityMetricData": [{
                    "Entity": {"KeyAttributes": {"Type": "Service", "Name": "forged"}},
                    "MetricData": [datum(json!({"Value": 4200.0}))],
                }],
            }),
        ),
        (
            "several data points of several forms in one request",
            json!({
                "Namespace": RESERVED,
                "MetricData": [
                    datum(json!({"Value": 4200.0})),
                    datum(json!({"Values": [4200.0], "Counts": [2.0]})),
                ],
            }),
        ),
    ];

    for (case, request) in &rows {
        // A fresh emulator per row, so the row's own publish is the only thing that could
        // have stored anything.
        let router = build_router();

        let (status, body) = call(&router, "PutMetricData", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the publish to be refused with HTTP 400, got {status} \
             with body {body}"
        );
        // The modelled shape name is what the awsJson error envelope carries; the
        // `awsQueryCompatible` layer is what renders it to an SDK as the
        // `InvalidParameterValue` code a CloudWatch client branches on.
        assert_eq!(
            error_code(&body),
            "InvalidParameterValueException",
            "case {case:?}: expected the invalid-parameter-value refusal, got {body}"
        );

        // Nothing was stored: the catalogue knows no such series …
        let listed = ok(&router, "ListMetrics", &json!({"Namespace": RESERVED})).await;
        assert_eq!(
            listed["Metrics"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: a refused publish must leave the catalogue empty, got {listed}"
        );
        // … and no datapoint can be aggregated out of it.
        let statistics = ok(
            &router,
            "GetMetricStatistics",
            &json!({
                "Namespace": RESERVED,
                "MetricName": METRIC,
                "Dimensions": dimensions(),
                "StartTime": now_secs() - 3_600.0,
                "EndTime": now_secs() + 3_600.0,
                "Period": 60,
                "Statistics": ["Maximum"],
            }),
        )
        .await;
        assert_eq!(
            statistics["Datapoints"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: a refused publish must store no datapoint, got {statistics}"
        );
    }
}

#[tokio::test]
async fn a_refused_publish_does_not_drive_the_alarm_that_watches_the_reserved_series() {
    let router = build_router();
    // The alarm the recorded case configures on the reserved series, with an action to fire.
    ok(
        &router,
        "PutMetricAlarm",
        &json!({
            "AlarmName": "forged-series-high",
            "Namespace": RESERVED,
            "MetricName": METRIC,
            "Dimensions": dimensions(),
            "Statistic": "Maximum",
            "Period": 60,
            "EvaluationPeriods": 1,
            "Threshold": 10.0,
            "ComparisonOperator": "GreaterThanOrEqualToThreshold",
            "TreatMissingData": "ignore",
            "AlarmActions": ["arn:aws:sns:us-east-1:000000000000:alarm-notifications"],
        }),
    )
    .await;

    let (status, body) = call(
        &router,
        "PutMetricData",
        &json!({
            "Namespace": RESERVED,
            "MetricData": [datum(json!({"Value": 4200.0}))],
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "the forged publish must be refused, got {status} with body {body}"
    );

    // The alarm that watches the series was not driven by it: it still reports the state its
    // creation left it in, and its history records the creation only — no state transition,
    // so no action was fired. The wait is past the span an alarm's first evaluation is owed,
    // so the evaluation HAS run by the time the alarm is read.
    wait_for_evaluation();
    let described = ok(
        &router,
        "DescribeAlarms",
        &json!({"AlarmNames": ["forged-series-high"]}),
    )
    .await;
    assert_eq!(
        described["MetricAlarms"][0]
            .get("StateValue")
            .and_then(Value::as_str),
        Some("INSUFFICIENT_DATA"),
        "a refused publish must not drive the alarm, got {described}"
    );
    let history = ok(
        &router,
        "DescribeAlarmHistory",
        &json!({"AlarmName": "forged-series-high"}),
    )
    .await;
    let items = history["AlarmHistoryItems"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let kinds: Vec<&str> = items
        .iter()
        .map(|item| {
            item.get("HistoryItemType")
                .and_then(Value::as_str)
                .unwrap_or_default()
        })
        .collect();
    assert_eq!(
        kinds,
        vec!["ConfigurationUpdate"],
        "only the alarm's creation may be recorded, got {history}"
    );
}
