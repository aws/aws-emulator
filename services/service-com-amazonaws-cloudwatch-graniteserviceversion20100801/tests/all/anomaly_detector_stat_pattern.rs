// SPDX-License-Identifier: CC0-1.0

//! The STATISTIC an anomaly detection model is trained on, and the refusal a statistic the
//! model does not admit earns.
//!
//! The rule under test: **a refusal of a value against a modelled `@pattern` quotes the
//! expression the WHOLE value must satisfy, deciding the two ends independently and adding
//! nothing around what the anchors enclosed** — an end the model ANCHORS carries its `^` / `$`
//! away with it, because the anchor is how a Smithy `@pattern` (a FIND) is made total at that
//! end rather than part of the grammar a caller has to satisfy, while an end the model leaves
//! OPEN admits any run of characters and is spelled with one. So the expression between a
//! pair of anchors is quoted as the model writes it, a top-level alternation included: a
//! client that lifts the expression out of the message and re-validates locally gets the one
//! the service publishes.
//!
//! `AnomalyDetectorMetricStat` is the recorded case of an expression that alternates at its
//! top level under a pair of anchors, which the rule leaves bare; the suite beside this one
//! (`published_pattern_requirement`) carries the ends the other recorded members decide.
//!
//! The rows differ along what the rule generalises over: the value submitted (four statistics
//! the pattern admits nowhere, each of a different shape of near-miss), which member carried
//! it — the deprecated flat `Stat` and the `Stat` of the single-metric structure, so the depth
//! the value sat at varies too — and, for the statistics the pattern DOES admit, the form of
//! the admitted statistic (a named one, a percentile, an interquartile mean and a trimmed-mean
//! function), each read back off the model the write created.

use serde_json::{json, Value};

use crate::harness::{build_router, call, error_code, error_message, ok};

/// The clause a constraint refusal states a `@pattern` requirement in, as the framework words
/// it.
const PATTERN_CLAUSE: &str = "Member must satisfy regular expression pattern: ";

/// The error SHAPE the body of a request-validation refusal names.
const VALIDATION_SHAPE: &str = "ValidationException";

/// The `@pattern` of `AnomalyDetectorMetricStat`, the shape both `Stat` members target, as the
/// model declares it: anchored at both ends, alternating at its top level
/// (`smithy_query(query="shape",shape="AnomalyDetectorMetricStat")`).
const STAT_PATTERN: &str = r"^(SampleCount|Average|Sum|Minimum|Maximum|IQM|(p|tc|tm|ts|wm)(\d{1,2}(\.\d{0,10})?|100)|[ou]\d+(\.\d*)?)(_E|_L|_H)?|(TM|TC|TS|WM)\(((((\d{1,2})(\.\d{0,10})?|100(\.0{0,10})?)%)?:((\d{1,2})(\.\d{0,10})?|100(\.0{0,10})?)%|((\d{1,2})(\.\d{0,10})?|100(\.0{0,10})?)%:(((\d{1,2})(\.\d{0,10})?|100(\.0{0,10})?)%)?)\)|(TM|TC|TS|WM|PR)\(((\d+(\.\d{0,10})?|(\d+(\.\d{0,10})?[Ee][+-]?\d+)):((\d+(\.\d{0,10})?|(\d+(\.\d{0,10})?[Ee][+-]?\d+)))?|((\d+(\.\d{0,10})?|(\d+(\.\d{0,10})?[Ee][+-]?\d+)))?:(\d+(\.\d{0,10})?|(\d+(\.\d{0,10})?[Ee][+-]?\d+)))\)$";

/// The namespace every metric of this suite lives in.
const NAMESPACE: &str = "MyApp/DetectorStat";

/// The expression a refusal quotes for the declared pattern `declared`: the expression the
/// WHOLE value must satisfy, with each end decided on its own and nothing added around what
/// the anchors enclosed ([the module docs](self)).
fn quoted_expression(declared: &str) -> String {
    let (body, anchored_at_start) = match declared.strip_prefix('^') {
        Some(rest) => (rest, true),
        None => (declared, false),
    };
    let (body, anchored_at_end) = match body.strip_suffix('$') {
        Some(inner) => (inner, true),
        None => (body, false),
    };
    let open = |anchored: bool| if anchored { "" } else { ".*" };
    format!("{}{body}{}", open(anchored_at_start), open(anchored_at_end))
}

/// One refused row: the statistic submitted, the request that carried it and the member path
/// the clause names.
struct Refused {
    /// What this row varies, named in every assertion it makes.
    case: &'static str,
    /// The statistic the request carried.
    submitted: &'static str,
    /// The request itself.
    request: Value,
    /// The member path the clause names, as the validating walk spells one.
    member: &'static str,
}

/// The rows: each statistic carried by the deprecated flat member and by the single-metric
/// structure's own, so the same requirement is stated at two depths of the request.
fn refused_rows() -> Vec<Refused> {
    // Four statistics the pattern admits at no position: a word none of its branches opens
    // with, a declared statistic in the wrong case, a percentile with no number after its
    // letter, and a number before the letter instead of after it.
    let submitted = [
        ("a word no branch of the expression admits", "NotARealStatistic"),
        ("a declared statistic in the wrong case", "average"),
        ("a percentile letter with no number", "p"),
        ("a percentile written back to front", "99p"),
    ];
    let mut rows = Vec::new();
    for (case, stat) in submitted {
        rows.push(Refused {
            case,
            submitted: stat,
            request: json!({
                "Namespace": NAMESPACE,
                "MetricName": "Latency",
                "Stat": stat,
            }),
            member: "stat",
        });
        rows.push(Refused {
            case,
            submitted: stat,
            request: json!({
                "SingleMetricAnomalyDetector": {
                    "Namespace": NAMESPACE,
                    "MetricName": "Latency",
                    "Stat": stat,
                },
            }),
            member: "singleMetricAnomalyDetector.stat",
        });
    }
    rows
}

#[tokio::test]
async fn a_statistic_outside_the_modelled_pattern_is_refused_quoting_what_the_anchors_enclosed() {
    // The published rendering of this pattern, pinned on its own: the model anchors both ends,
    // so the quoted expression is the text between them — a top-level alternation left exactly
    // as the model wrote it, with no group added around it.
    let quoted = quoted_expression(STAT_PATTERN);
    assert!(
        quoted.starts_with("(SampleCount|Average|") && quoted.ends_with(r"\)"),
        "the statistic requirement must be quoted as the text between the model's anchors, got \
         {quoted:?}"
    );

    let router = build_router();
    for Refused {
        case,
        submitted,
        request,
        member,
    } in refused_rows()
    {
        let (status, body) = call(&router, "PutAnomalyDetector", &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, VALIDATION_SHAPE),
            "case {case:?} at {member:?}: expected HTTP 400 {VALIDATION_SHAPE}, got {status} \
             with {body}"
        );
        let expected = format!(
            "1 validation error detected: Value '{submitted}' at '{member}' failed to satisfy \
             constraint: {PATTERN_CLAUSE}{quoted}"
        );
        let stated = error_message(&body);
        assert_eq!(
            stated, expected,
            "case {case:?} at {member:?}: expected the refusal to read {expected:?}, got \
             {stated:?}"
        );
        // Stated separately, because the rendering is the whole point: whatever else the clause
        // carries, the expression it quotes is neither the model's own text nor that text with
        // anything added around it.
        for stated_expression in stated.split(PATTERN_CLAUSE).skip(1) {
            assert!(
                !stated_expression.starts_with('^') && !stated_expression.ends_with('$'),
                "case {case:?} at {member:?}: the quoted expression must carry no anchor of the \
                 model's, got {stated_expression:?}"
            );
            assert!(
                !stated_expression.starts_with("(("),
                "case {case:?} at {member:?}: the quoted expression must add no group around \
                 what the anchors enclosed, got {stated_expression:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_statistic_the_modelled_pattern_admits_is_stored_and_read_back() {
    let router = build_router();
    // Four forms the expression admits, one per branch it offers: a named statistic, a
    // percentile, the interquartile mean, and a trimmed-mean function over a percentile range.
    let admitted = [
        ("a named statistic", "Average"),
        ("a percentile", "p99"),
        ("the interquartile mean", "IQM"),
        ("a trimmed mean over a percentile range", "TM(25%:75%)"),
    ];
    for (case, stat) in admitted {
        let metric_name = format!("Latency-{stat}");
        let created = ok(
            &router,
            "PutAnomalyDetector",
            &json!({
                "SingleMetricAnomalyDetector": {
                    "Namespace": NAMESPACE,
                    "MetricName": metric_name,
                    "Stat": stat,
                },
            }),
        )
        .await;
        let id = created.get("AnomalyDetectorId").and_then(Value::as_str);
        assert!(
            id.is_some_and(|id| !id.is_empty()),
            "case {case:?}: expected the write to report an AnomalyDetectorId, got {created}"
        );

        let described = ok(
            &router,
            "DescribeAnomalyDetectors",
            &json!({"Namespace": NAMESPACE, "MetricName": metric_name}),
        )
        .await;
        let stats: Vec<&str> = described
            .get("AnomalyDetectors")
            .and_then(Value::as_array)
            .map(|detectors| {
                detectors
                    .iter()
                    .filter_map(|detector| {
                        detector
                            .pointer("/SingleMetricAnomalyDetector/Stat")
                            .and_then(Value::as_str)
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            stats,
            vec![stat],
            "case {case:?}: expected the read to report the statistic the write named, got \
             {described}"
        );
    }
}
