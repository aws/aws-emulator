// SPDX-License-Identifier: CC0-1.0

//! The FORM of the identifier a write mints for an anomaly detection model, and how it differs
//! from the creation identity an alarm read reports.
//!
//! One rule, asserted over a table whose rows differ along WHICH write mints the identifier and
//! along what the model it names watches: the identifier a write mints for a model ("The unique
//! identifier of the anomaly detector that you created or updated.",
//! `PutAnomalyDetectorOutput$AnomalyDetectorId`, length 1..128, pattern
//! `^[A-Za-z0-9_./:%()+-]+$`) is ONE OPAQUE TOKEN: a single run of characters inside that
//! pattern with none of its punctuation in it, so there is no segment separator for a client to
//! split it on, and no clock reading for a client to recover the instant of the write from.
//!
//! The other half of the rule is that the token is the model's identity, so this suite asserts
//! it where a client meets it: the write reports it, a read reports the same bytes back, a
//! second write of the same definition reports the one the model was created with, the model is
//! found by it and deleted by it, and no other model of the store carries it.
//!
//! The bytes of any one identifier are the service's own per-run value — never a particular
//! token — while the creation identity a metric alarm read reports names the WRITE that created
//! the alarm and keeps the composite form that identity has (`<uuid>/<epoch-millis>`,
//! [`creation_identity_millis`]), which is why the two are minted apart.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{build_router, call, creation_identity_millis, now_secs, ok};

/// The namespace the metrics of this suite live in.
const NAMESPACE: &str = "MyApp/DetectorIdForm";

/// The punctuation the modelled `AnomalyDetectorId` pattern (`^[A-Za-z0-9_./:%()+-]+$`) admits
/// beside the alphanumerics — every character a client could read the identifier as a composite
/// of segments on, and so every character ONE opaque token carries none of.
const PATTERN_PUNCTUATION: &str = "_./:%()+-";

/// The widths an epoch clock reading of this era takes in decimal: ten digits for a reading in
/// seconds, thirteen for one in milliseconds.
const CLOCK_WIDTHS: [usize; 2] = [10, 13];

/// Fail `case` unless `id` is one opaque token: inside the modelled `AnomalyDetectorId` (length
/// 1..128, pattern `^[A-Za-z0-9_./:%()+-]+$`), carrying none of that pattern's punctuation, and
/// naming no instant of the span `from_secs..=to_secs` the write that minted it happened in.
///
/// The one reader of that form, so this suite and every suite that asserts a model's identifier
/// agree on what the form is.
pub fn assert_one_opaque_token(id: &str, case: &str, from_secs: f64, to_secs: f64) {
    assert!(
        !id.is_empty() && id.len() <= 128,
        "case {case:?}: an identifier stays inside the modelled length 1..128, got {} \
         characters in {id:?}",
        id.len()
    );
    assert!(
        id.chars().all(|glyph| glyph.is_ascii_alphanumeric()),
        "case {case:?}: an identifier is one token, so it carries none of the modelled \
         pattern's punctuation ({PATTERN_PUNCTUATION}), got {id:?}"
    );
    assert!(
        creation_identity_millis(id).is_none(),
        "case {case:?}: an identifier is not the composite creation identity an alarm read \
         reports, got {id:?}"
    );
    let named = named_instant(id, from_secs, to_secs);
    assert!(
        named.is_none(),
        "case {case:?}: an identifier names no clock reading, got {named:?} in {id:?} for a \
         write in {from_secs}..={to_secs}"
    );
}

/// The clock reading `id` embeds for an instant of the span `from_secs..=to_secs`, as the
/// decimal window that reads it, or `None` when the identifier embeds none.
///
/// Every window of `id` that is all decimal digits and as wide as a clock reading
/// ([`CLOCK_WIDTHS`]) is read as an instant — in seconds for the narrower width, in
/// milliseconds for the wider one — so an identifier that carried the instant of its write in
/// either unit is caught whatever surrounds it.
fn named_instant(id: &str, from_secs: f64, to_secs: f64) -> Option<String> {
    let bytes = id.as_bytes();
    CLOCK_WIDTHS.iter().find_map(|width| {
        bytes
            .windows(*width)
            .filter(|window| window.iter().all(u8::is_ascii_digit))
            .find(|window| {
                let Ok(text) = std::str::from_utf8(window) else {
                    return false;
                };
                let Ok(reading) = text.parse::<f64>() else {
                    return false;
                };
                let secs = if *width == 10 {
                    reading
                } else {
                    reading / 1_000.0
                };
                (from_secs..=to_secs).contains(&secs)
            })
            .and_then(|window| std::str::from_utf8(window).ok())
            .map(str::to_string)
    })
}

/// A single-metric write of `metric` in this suite's own namespace, with the dimensions and
/// statistic given.
pub fn single_metric(metric: &str, dimensions: Option<Value>, stat: &str) -> Value {
    single_metric_in(NAMESPACE, metric, dimensions, stat)
}

/// The same write, of a metric in `namespace` — the seam a suite that varies the metric's own
/// text uses, so one construction serves both.
pub fn single_metric_in(
    namespace: &str,
    metric: &str,
    dimensions: Option<Value>,
    stat: &str,
) -> Value {
    let mut detector = json!({"Namespace": namespace, "MetricName": metric, "Stat": stat});
    if let Some(dimensions) = dimensions {
        detector["Dimensions"] = dimensions;
    }
    json!({"SingleMetricAnomalyDetector": detector})
}

/// A metric-math write over `metric`.
pub fn metric_math(metric: &str) -> Value {
    json!({"MetricMathAnomalyDetector": {"MetricDataQueries": [
        {
            "Id": "m1",
            "MetricStat": {
                "Metric": {"Namespace": NAMESPACE, "MetricName": metric},
                "Period": 300,
                "Stat": "Average",
            },
            "ReturnData": false,
        },
        {"Id": "ad1", "Expression": "m1 * 2", "ReturnData": true},
    ]}})
}

/// The identifier `PutAnomalyDetector` reported for `request`, with the span the write happened
/// in — the instants either side of the call — so a row can assert the identifier names neither.
pub async fn put_detector(
    router: &MultiServiceRouter,
    request: &Value,
    case: &str,
) -> (String, f64, f64) {
    let before = now_secs();
    let body = ok(router, "PutAnomalyDetector", request).await;
    let after = now_secs();
    let id = body
        .get("AnomalyDetectorId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected an AnomalyDetectorId, got {body}"))
        .to_string();
    // A second of room either side of the call, so a write whose identifier rounded the instant
    // it happened at to the nearest second is still caught.
    (id, before - 1.0, after + 1.0)
}

/// Every identifier the store reports, over both modelled model types: "If empty, defaults to
/// SINGLE_METRIC." (`DescribeAnomalyDetectorsInput$AnomalyDetectorTypes`).
pub async fn described_ids(router: &MultiServiceRouter) -> Vec<String> {
    let body = ok(
        router,
        "DescribeAnomalyDetectors",
        &json!({"AnomalyDetectorTypes": ["SINGLE_METRIC", "METRIC_MATH"]}),
    )
    .await;
    body.get("AnomalyDetectors")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|detector| {
            detector
                .get("AnomalyDetectorId")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect()
}

/// The identifiers a read that names `ids` reports, which is how a row asserts the models are
/// found by the tokens the writes minted for them.
pub async fn described_by_ids(router: &MultiServiceRouter, ids: &[String]) -> Vec<String> {
    let body = ok(
        router,
        "DescribeAnomalyDetectors",
        &json!({"AnomalyDetectorIds": ids}),
    )
    .await;
    body.get("AnomalyDetectors")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|detector| {
            detector
                .get("AnomalyDetectorId")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect()
}

#[tokio::test]
async fn a_write_mints_one_opaque_token_for_the_model_it_creates() {
    // Rows differ along the spelling that names the model and along what it watches — the
    // structure with dimensions, the structure without, the deprecated flat members, and the
    // metric-math structure — so the form is asserted over every write that mints a model's
    // identifier rather than over one recorded request.
    let cases: Vec<(&str, Value)> = vec![
        (
            "the single-metric structure with dimensions",
            single_metric(
                "Latency",
                Some(json!([{"Name": "Service", "Value": "checkout"}])),
                "Average",
            ),
        ),
        (
            "the single-metric structure with no dimensions and a percentile statistic",
            single_metric("Queued", None, "p99"),
        ),
        (
            "the deprecated flat metric members",
            json!({"Namespace": NAMESPACE, "MetricName": "Errors", "Stat": "Sum"}),
        ),
        ("the metric-math structure", metric_math("Throughput")),
    ];

    let mut minted: Vec<String> = Vec::new();
    for (case, request) in cases {
        let router = build_router();
        let (id, from_secs, to_secs) = put_detector(&router, &request, case).await;
        assert_one_opaque_token(&id, case, from_secs, to_secs);

        // The identity is the model's OWN: no model of any store this suite wrote carries it.
        assert!(
            !minted.contains(&id),
            "case {case:?}: every model carries an identifier of its own, got the repeated \
             {id:?} among {minted:?}"
        );
        minted.push(id.clone());

        // A READ reports the token the write minted, unchanged — the very bytes a client can
        // re-describe or delete the model with.
        let described = described_ids(&router).await;
        assert_eq!(
            described,
            vec![id.clone()],
            "case {case:?}: a read reports the identifier the write minted, got {described:?}"
        );
        let found = described_by_ids(&router, std::slice::from_ref(&id)).await;
        assert_eq!(
            found,
            vec![id.clone()],
            "case {case:?}: the model is found by the identifier the write minted, got {found:?}"
        );

        // A second write of the same definition UPDATES that model, so it reports the identifier
        // the model was created with rather than minting a second one.
        let (again, _, _) = put_detector(&router, &request, case).await;
        assert_eq!(
            again, id,
            "case {case:?}: a write that updates a model reports the identifier it was created \
             with"
        );

        // And the model is deleted by that token, after which a model written again for the
        // same definition carries a new one — of the same form.
        let (status, body) = call(
            &router,
            "DeleteAnomalyDetector",
            &json!({"AnomalyDetectorId": id.clone()}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the model is deleted by the identifier the write minted, got {body}"
        );
        assert!(
            described_ids(&router).await.is_empty(),
            "case {case:?}: the deleted model is gone"
        );
        let (recreated, from_secs, to_secs) = put_detector(&router, &request, case).await;
        assert_one_opaque_token(&recreated, case, from_secs, to_secs);
        assert_ne!(
            recreated, id,
            "case {case:?}: a model created again after a delete carries a new identifier"
        );
    }
}

#[tokio::test]
async fn the_model_an_alarm_band_creates_carries_one_opaque_token_too() {
    // The other write that mints a model's identifier: the alarm whose threshold is drawn from a
    // band ("In an alarm based on an anomaly detection model, this is the ID of the
    // ANOMALY_DETECTION_BAND function used as the threshold for the alarm",
    // `PutMetricAlarmInput$ThresholdMetricId`). Rows differ along the band's width and along the
    // statistic and dimensions of the series it is drawn from.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a band of an explicit width over a dimensioned series",
            json!([
                {
                    "Id": "m1",
                    "ReturnData": true,
                    "MetricStat": {
                        "Metric": {
                            "Namespace": NAMESPACE,
                            "MetricName": "Latency",
                            "Dimensions": [{"Name": "Service", "Value": "checkout"}],
                        },
                        "Period": 60,
                        "Stat": "Average",
                    },
                },
                {"Id": "ad1", "Expression": "ANOMALY_DETECTION_BAND(m1, 2)", "ReturnData": true},
            ]),
        ),
        (
            "a band of the default width over a series with no dimensions",
            json!([
                {
                    "Id": "m1",
                    "ReturnData": true,
                    "MetricStat": {
                        "Metric": {
                            "Namespace": NAMESPACE,
                            "MetricName": "Errors",
                            "Dimensions": [],
                        },
                        "Period": 60,
                        "Stat": "Sum",
                    },
                },
                {"Id": "ad1", "Expression": "ANOMALY_DETECTION_BAND(m1)", "ReturnData": true},
            ]),
        ),
    ];

    for (case, queries) in rows {
        let router = build_router();
        let before = now_secs();
        ok(
            &router,
            "PutMetricAlarm",
            &json!({
                "AlarmName": "band-owner",
                "Metrics": queries,
                "ThresholdMetricId": "ad1",
                "ComparisonOperator": "LessThanLowerOrGreaterThanUpperThreshold",
                "EvaluationPeriods": 1,
                "TreatMissingData": "missing",
                "ActionsEnabled": false,
            }),
        )
        .await;
        let after = now_secs();

        let described = described_ids(&router).await;
        assert_eq!(
            described.len(),
            1,
            "case {case:?}: the alarm write creates the model its band is drawn from, got \
             {described:?}"
        );
        assert_one_opaque_token(&described[0], case, before - 1.0, after + 1.0);
    }
}
