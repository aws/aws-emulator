// SPDX-License-Identifier: CC0-1.0

//! What the identifier a write mints for an anomaly detection model IS, beside the single-token
//! form the sibling suite `anomaly_detector_id_form` asserts: a value of the SERVICE's own,
//! minted once per model, carrying nothing of the request, and naming exactly one model of the
//! store wherever a later call takes an identifier.
//!
//! The rule, and what fixes it: "The unique identifier of the anomaly detector."
//! (`AnomalyDetector$AnomalyDetectorId`, `PutAnomalyDetectorOutput$AnomalyDetectorId`, length
//! 1..128, pattern `^[A-Za-z0-9_./:%()+-]+$` —
//! `smithy_query(query="shape", shape="com.amazonaws.cloudwatch#AnomalyDetectorId")`) is ALL the
//! model says about the value, and 128 characters cannot hold the identity of the series the
//! model watches: a namespace and a metric name of 255 characters each
//! (`SingleMetricAnomalyDetector$Namespace`, `$MetricName`), and up to thirty dimensions whose
//! name runs to 255 characters and whose value to 1024 (`Dimension$Name`, `$Value`). So the
//! identifier is MINTED for the model rather than rendered from the request, and what a client
//! may rely on is its form, its stability for one model, and that it resolves to that model and
//! to no other — which is what every row here asserts.
//!
//! Rows differ along what a model watches and along what its definition is SPELLED with: text
//! longer than any identifier, text outside the characters an identifier admits, an extended
//! statistic, a dimensioned series, and the metric-math form. The form of one identifier, the
//! writes that mint one and the readers of a store's identifiers are the sibling suite's, so
//! both suites ask the same questions of the same answers.

use serde_json::{json, Value};

use crate::anomaly_detector_id_form::{
    assert_one_opaque_token, described_by_ids, described_ids, metric_math, put_detector,
    single_metric_in,
};
use crate::harness::{build_router, call};

/// The namespace the metrics of this suite live in, where a row does not spell its own.
const NAMESPACE: &str = "MyApp/MintedDetectorId";

/// The longest text a metric's namespace or name may carry: "length 1..255"
/// (`SingleMetricAnomalyDetector$Namespace`, `$MetricName`) — twice what the longest identifier
/// holds, which is why an identifier cannot be a rendering of the series it names.
const MAX_METRIC_TEXT: usize = 255;

/// One definition a row writes, and the text of its own that row's identifier must carry none of.
struct Row {
    /// What the row is, named in every assertion it makes.
    case: &'static str,
    /// The `PutAnomalyDetector` request body.
    request: Value,
    /// Text the request spells the model's identity with, each long enough that an identifier
    /// carrying it could not be a coincidence.
    spelled: Vec<String>,
}

/// The definitions every row of this suite is written over.
fn rows() -> Vec<Row> {
    // Text as long as a metric's namespace and name may be — longer than any identifier, so an
    // identifier that rendered it could not satisfy the modelled length at all.
    let long_namespace = format!("Longest/{}", "N".repeat(MAX_METRIC_TEXT - "Longest/".len()));
    let long_metric = "LongestMetricName".repeat(MAX_METRIC_TEXT / "LongestMetricName".len());
    // Text outside the characters an identifier admits: a space, a comma, an equals sign and a
    // hash are all fine in a metric's own identity ("Dimension values must contain only ASCII
    // characters", `Dimension$Value`) and none of them is in `^[A-Za-z0-9_./:%()+-]+$`.
    let punctuated_namespace = "MyApp, Reporting = #live";
    let punctuated_metric = "Request Latency #p50";
    vec![
        Row {
            case: "a dimensioned series under a plain statistic",
            request: single_metric_in(
                NAMESPACE,
                "CheckoutLatency",
                Some(json!([
                    {"Name": "ServiceName", "Value": "checkout-frontend"},
                    {"Name": "Deployment", "Value": "green = canary"},
                ])),
                "Average",
            ),
            spelled: vec![
                "CheckoutLatency".to_string(),
                "ServiceName".to_string(),
                "checkout-frontend".to_string(),
                "Deployment".to_string(),
            ],
        },
        Row {
            case: "an extended statistic, whose own text is inside the identifier pattern",
            request: single_metric_in(NAMESPACE, "TrimmedQueueDepth", None, "TM(10%:90%)"),
            spelled: vec!["TrimmedQueueDepth".to_string()],
        },
        Row {
            case: "a namespace and a metric name at their modelled maximum length",
            request: single_metric_in(&long_namespace, &long_metric, None, "p99.9"),
            // The whole text and a window of it: text longer than the identifier cannot be
            // carried whole, so what the row looks for is a RUN of it — which an identifier
            // that rendered any part of that identity would carry.
            spelled: vec![
                long_namespace.clone(),
                long_metric.clone(),
                long_namespace[..16].to_string(),
                long_metric[..16].to_string(),
            ],
        },
        Row {
            case: "an identity spelled with characters no identifier admits",
            request: single_metric_in(
                punctuated_namespace,
                punctuated_metric,
                Some(json!([{"Name": "Pool Name", "Value": "orders, west"}])),
                "Maximum",
            ),
            spelled: vec![
                "Reporting".to_string(),
                "Latency".to_string(),
                "orders".to_string(),
            ],
        },
        Row {
            case: "the deprecated flat metric members",
            request: json!({
                "Namespace": NAMESPACE,
                "MetricName": "FlatSpellingErrors",
                "Stat": "Sum",
            }),
            spelled: vec!["FlatSpellingErrors".to_string()],
        },
        Row {
            case: "the metric-math form",
            request: metric_math("MathThroughput"),
            spelled: vec!["MathThroughput".to_string()],
        },
    ]
}

/// Fail `case` unless `id` carries none of the `spelled` text the request names the model's
/// identity with — the property that separates a MINTED identifier from one rendered out of the
/// request, read case-insensitively so an identifier that re-cased the text is caught too.
///
/// Only fragments of six characters or more are looked for: a shorter run can turn up in an
/// opaque value by chance, where a six-character run of the request's own words cannot.
fn assert_carries_none_of(id: &str, spelled: &[String], case: &str) {
    let folded = id.to_ascii_lowercase();
    for fragment in spelled.iter().filter(|text| text.len() >= 6) {
        let wanted = fragment.to_ascii_lowercase();
        assert!(
            !folded.contains(&wanted),
            "case {case:?}: an identifier is minted for the model, so it carries none of the \
             text the request spells its identity with, got {fragment:?} in {id:?}"
        );
    }
}

#[tokio::test]
async fn an_identifier_is_minted_whatever_the_definition_is_spelled_with() {
    for row in rows() {
        let router = build_router();
        let (id, from_secs, to_secs) = put_detector(&router, &row.request, row.case).await;

        // The form the model fixes — the length among it, whatever the length of the identity
        // the model watches — and nothing of the request in it.
        assert_one_opaque_token(&id, row.case, from_secs, to_secs);
        assert_carries_none_of(&id, &row.spelled, row.case);

        // And it is the handle the model is known by: the read reports the same bytes, the
        // identifier filter resolves to that model, and the delete takes it by the same value.
        let described = described_ids(&router).await;
        assert_eq!(
            described,
            vec![id.clone()],
            "case {:?}: a read reports the identifier the write minted, got {described:?}",
            row.case
        );
        let found = described_by_ids(&router, std::slice::from_ref(&id)).await;
        assert_eq!(
            found,
            vec![id.clone()],
            "case {:?}: the identifier the write minted names that model, got {found:?}",
            row.case
        );
        let (status, body) = call(
            &router,
            "DeleteAnomalyDetector",
            &json!({"AnomalyDetectorId": id.clone()}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the model is deleted by the identifier the write minted, got {body}",
            row.case
        );
    }
}

#[tokio::test]
async fn each_identifier_names_exactly_one_model_of_the_store() {
    let router = build_router();
    let rows = rows();

    // One store holding every definition of the table at once, which is what makes "names
    // exactly one model" assertable: the identifier filter has other models to pick wrongly from.
    let mut minted: Vec<String> = Vec::new();
    for row in &rows {
        let (id, _, _) = put_detector(&router, &row.request, row.case).await;
        assert!(
            !minted.contains(&id),
            "case {:?}: every model carries an identifier of its own, got the repeated {id:?} \
             among {minted:?}",
            row.case
        );
        minted.push(id);
    }

    // A read reports exactly the identifiers the writes minted, and reports them stably: the
    // same bytes, for the same models, on a second read.
    let mut expected = minted.clone();
    expected.sort();
    let described = described_ids(&router).await;
    assert_eq!(
        described, expected,
        "every model of the store is reported under the identifier its write minted, got \
         {described:?}"
    );
    assert_eq!(
        described_ids(&router).await,
        expected,
        "an identifier is stable across reads: a second read reports the same bytes"
    );

    // The identifier filter names the models it lists and nothing else, whether it names one of
    // them or several.
    for wanted in [
        vec![minted[0].clone()],
        vec![minted[1].clone(), minted[3].clone()],
        minted.clone(),
    ] {
        let mut expected = wanted.clone();
        expected.sort();
        let found = described_by_ids(&router, &wanted).await;
        assert_eq!(
            found, expected,
            "a read naming {wanted:?} reports those models and no other, got {found:?}"
        );
    }

    // And a delete by one identifier removes that model only: every other identifier of the
    // store still names its own model afterwards.
    let removed = minted.remove(2);
    let (status, body) = call(
        &router,
        "DeleteAnomalyDetector",
        &json!({"AnomalyDetectorId": removed.clone()}),
    )
    .await;
    assert_eq!(
        status, 200,
        "a model is deleted by the identifier its write minted, got {body}"
    );
    let mut expected = minted.clone();
    expected.sort();
    let described = described_ids(&router).await;
    assert_eq!(
        described, expected,
        "a delete by identifier removes that model only, got {described:?} after removing \
         {removed:?}"
    );
    let found = described_by_ids(&router, std::slice::from_ref(&removed)).await;
    assert!(
        found.is_empty(),
        "the identifier of a deleted model names nothing, got {found:?}"
    );
}
