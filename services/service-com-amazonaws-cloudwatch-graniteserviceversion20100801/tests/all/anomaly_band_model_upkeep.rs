// SPDX-License-Identifier: CC0-1.0

//! The anomaly detection MODEL an alarm's band is drawn from, for as long as the alarm is
//! configured that way.
//!
//! An alarm whose threshold is a band names the query computing it ("make this value match the ID
//! of the `ANOMALY_DETECTION_BAND` function", `PutMetricAlarmInput$ThresholdMetricId`) and a band
//! is a model's ("You can use the model to display a band of expected normal values when the
//! metric is graphed.", `PutAnomalyDetector`), so the model is something such an alarm needs for
//! every metric its band is drawn from the whole time it stands — not only when it is written.
//! The rule asserted here: every EVALUATION of such an alarm ensures the model is there, so a
//! model deleted while an alarm still draws a band from it is back by that alarm's next
//! evaluation, and a model no alarm's band is drawn from stays deleted.
//!
//! One table, whose rows differ along what the rule generalises over: how the band reaches the
//! metric it is drawn from (directly over the retrieval query, and through a math expression
//! between the two), which statistic that metric is folded by, and whether any alarm's band is
//! drawn from the model at all — the last being the control the rule must NOT bring back. Every
//! row's model is deleted in one breath and the alarms are then driven by published datapoints,
//! so no row's outcome depends on how fast the host runs: the test looks again until the
//! condition holds or a generous deadline passes.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{build_router, call, ok};

/// The namespace every metric of this suite lives in.
const NAMESPACE: &str = "Custom/BandModelUpkeep";

/// How long the test keeps looking for the models an alarm's evaluation owes before calling them
/// missing.
///
/// Generous on purpose: nothing asserted here is about how fast the host runs, and only a model
/// that never comes back fails a row.
const UPKEEP_WAIT: Duration = Duration::from_secs(30);

/// How often it looks while waiting.
const LOOK_INTERVAL: Duration = Duration::from_millis(100);

/// One row: a metric whose model is deleted, the statistic it is folded by, the alarm (if any)
/// whose band is drawn from it, and whether the rule brings the model back.
struct Row {
    /// What this row varies, named in every assertion it makes.
    case: &'static str,
    /// The metric the model is for.
    metric: &'static str,
    /// The statistic the model folds that metric by.
    stat: &'static str,
    /// The `Metrics` array of the alarm whose band is drawn from it, or `None` for a model no
    /// alarm references.
    queries: Option<Value>,
    /// Whether an alarm's evaluation must create the model again.
    restored: bool,
}

/// The query retrieving `metric` folded by `stat`, as a band's alarm states it.
fn retrieval(id: &str, metric: &str, stat: &str, returns: bool) -> Value {
    json!({
        "Id": id,
        "MetricStat": {
            "Metric": {"Namespace": NAMESPACE, "MetricName": metric},
            "Period": 60,
            "Stat": stat,
        },
        "ReturnData": returns,
    })
}

/// The detectors `DescribeAnomalyDetectors` reports for this suite's namespace, keyed by the
/// metric each is for: the metric name against the detector's own id.
async fn listed_models(router: &MultiServiceRouter) -> Vec<(String, String)> {
    let listed = ok(
        router,
        "DescribeAnomalyDetectors",
        &json!({"Namespace": NAMESPACE}),
    )
    .await;
    listed["AnomalyDetectors"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|detector| {
            (
                detector["SingleMetricAnomalyDetector"]["MetricName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                detector["AnomalyDetectorId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// Publish one datapoint for `metric`, which is what drives the alarm evaluations the clock has
/// made due.
async fn publish(router: &MultiServiceRouter, metric: &str) {
    ok(
        router,
        "PutMetricData",
        &json!({
            "Namespace": NAMESPACE,
            "MetricData": [{"MetricName": metric, "Value": 1.0, "Unit": "Count"}],
        }),
    )
    .await;
}

#[tokio::test]
async fn an_alarms_evaluation_keeps_the_model_its_band_is_drawn_from() {
    let router = build_router();
    let rows = [
        Row {
            case: "a model the band is drawn from directly",
            metric: "Direct",
            stat: "Maximum",
            queries: Some(json!([
                retrieval("m1", "Direct", "Maximum", true),
                {"Id": "ad1", "Expression": "ANOMALY_DETECTION_BAND(m1, 2)"},
            ])),
            restored: true,
        },
        Row {
            case: "a model reached through the expression the band is drawn over",
            metric: "Chained",
            stat: "Average",
            queries: Some(json!([
                retrieval("m1", "Chained", "Average", true),
                {"Id": "e1", "Expression": "m1", "ReturnData": false},
                {"Id": "ad1", "Expression": "ANOMALY_DETECTION_BAND(e1, 3)"},
            ])),
            restored: true,
        },
        Row {
            case: "a model no alarm's band is drawn from",
            metric: "Unreferenced",
            stat: "Maximum",
            queries: None,
            restored: false,
        },
    ];

    // Every row's model, created the way a caller creates one, and the id it carries — so the
    // model that comes back can be told from the one that was deleted.
    let mut created: Vec<(&str, String)> = Vec::new();
    for row in &rows {
        let model = ok(
            &router,
            "PutAnomalyDetector",
            &json!({
                "SingleMetricAnomalyDetector": {
                    "Namespace": NAMESPACE,
                    "MetricName": row.metric,
                    "Stat": row.stat,
                },
            }),
        )
        .await;
        let id = model["AnomalyDetectorId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: the model must report an id, got {model}", row.case))
            .to_string();
        created.push((row.metric, id));
    }

    // The alarms whose bands are drawn from those models.
    for (index, row) in rows.iter().enumerate() {
        let Some(queries) = row.queries.as_ref() else {
            continue;
        };
        ok(
            &router,
            "PutMetricAlarm",
            &json!({
                "AlarmName": format!("band-model-upkeep-{index}"),
                "Metrics": queries,
                "ThresholdMetricId": "ad1",
                "ComparisonOperator": "GreaterThanUpperThreshold",
                "EvaluationPeriods": 1,
                "TreatMissingData": "missing",
                "ActionsEnabled": false,
            }),
        )
        .await;
    }

    // Every model deleted in one breath, referenced or not.
    for row in &rows {
        let (status, body) = call(
            &router,
            "DeleteAnomalyDetector",
            &json!({
                "SingleMetricAnomalyDetector": {
                    "Namespace": NAMESPACE,
                    "MetricName": row.metric,
                    "Stat": row.stat,
                },
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the model must be deletable, got {status} with {body}",
            row.case
        );
    }
    let emptied = listed_models(&router).await;
    assert!(
        emptied.is_empty(),
        "every model of the namespace was deleted, so none may be listed, got {emptied:?}"
    );

    // The alarms are driven by the datapoints they grade: the model each referencing alarm needs
    // is created again by that alarm's own evaluation, so the test looks again until the models
    // it expects are listed, publishing into every metric as it goes.
    let wanted: Vec<&str> = rows
        .iter()
        .filter(|row| row.restored)
        .map(|row| row.metric)
        .collect();
    let deadline = Instant::now() + UPKEEP_WAIT;
    let listed = loop {
        for row in &rows {
            publish(&router, row.metric).await;
        }
        let listed = listed_models(&router).await;
        let restored = wanted
            .iter()
            .all(|metric| listed.iter().any(|(named, _)| named == metric));
        if restored || Instant::now() >= deadline {
            break listed;
        }
        ::tokio::time::sleep(LOOK_INTERVAL).await;
    };

    for row in &rows {
        let case = row.case;
        let back = listed.iter().find(|(metric, _)| metric == row.metric);
        assert_eq!(
            back.is_some(),
            row.restored,
            "case {case:?}: expected the model to be listed again: {}, got {listed:?}",
            row.restored
        );
        let Some((_, id)) = back else {
            continue;
        };
        let deleted = created
            .iter()
            .find(|(metric, _)| *metric == row.metric)
            .map(|(_, id)| id.clone())
            .unwrap_or_default();
        assert_ne!(
            *id, deleted,
            "case {case:?}: the model that came back is a new model, so it carries an id of its \
             own, got {id:?} beside the deleted {deleted:?}"
        );
    }
}
