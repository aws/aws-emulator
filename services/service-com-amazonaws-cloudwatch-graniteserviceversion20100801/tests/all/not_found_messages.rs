// SPDX-License-Identifier: CC0-1.0

//! What a CloudWatch not-found refusal says, as opposed to which code it carries.
//!
//! The rule under test: the text a refusal carries belongs to the error class the service
//! answers that call with, so an emulator reproduces exactly the text the service words for
//! that class — and where the class is answered by its CODE alone, the refusal carries no
//! text at all rather than an explanatory sentence the service never sends, with the member
//! that would carry it ABSENT rather than present and empty.
//!
//! Both halves are tabled. The first table's rows differ along what the rule generalises
//! over — which operation refuses (`SetAlarmState`, `GetMetricStream` and the three
//! operations that take a `ResourceARN`), how the named resource misses (never created,
//! created and then deleted, differing from a stored name only in case or by a suffix), which
//! resource KIND the ARN names, and, for `SetAlarmState`, which state the request asks for —
//! so a recorded miss is one row among many. The second table pins the refusals the service
//! DOES word, so "carry no text" stays a property of those classes rather than of every
//! refusal.

use crate::harness::{build_router, call, error_code, error_message, ok};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The account the ARNs below carry; the resource KIND and NAME are what a store keys on.
const ACCOUNT: &str = "000000000000";

/// The Region the harness signs its requests for, and so the one the resources it creates
/// carry in their ARNs.
const REGION: &str = "us-east-1";

/// The three operations that take a `ResourceARN`, so one unheld ARN is asserted on all of
/// them.
const TAGGING_OPERATIONS: [&str; 3] = ["TagResource", "ListTagsForResource", "UntagResource"];

/// The request each `ResourceARN` operation needs, naming `arn`.
fn tagging_request(operation: &str, arn: &str) -> Value {
    match operation {
        "TagResource" => json!({"ResourceARN": arn, "Tags": [{"Key": "Env", "Value": "prod"}]}),
        "UntagResource" => json!({"ResourceARN": arn, "TagKeys": ["Env"]}),
        _ => json!({"ResourceARN": arn}),
    }
}

/// A `PutMetricStream` request with only the members the model makes required.
fn put_stream(name: &str) -> Value {
    json!({
        "Name": name,
        "FirehoseArn": format!("arn:aws:firehose:{REGION}:{ACCOUNT}:deliverystream/{name}"),
        "RoleArn": format!("arn:aws:iam::{ACCOUNT}:role/{name}"),
        "OutputFormat": "json",
    })
}

/// A `PutMetricAlarm` request with only the members the model makes required.
fn put_alarm(name: &str) -> Value {
    json!({
        "AlarmName": name,
        "Namespace": "MyApp",
        "MetricName": "Latency",
        "Statistic": "Average",
        "Period": 60,
        "EvaluationPeriods": 1,
        "Threshold": 1.0,
        "ComparisonOperator": "GreaterThanThreshold",
    })
}

/// One alarm and one metric stream the store DOES hold, plus one of each that was created and
/// then deleted, so a refusal below is about the resource the request names rather than about
/// an empty store.
async fn given_resources(router: &MultiServiceRouter) {
    ok(router, "PutMetricAlarm", &put_alarm("present-alarm")).await;
    ok(router, "PutMetricAlarm", &put_alarm("deleted-alarm")).await;
    ok(
        router,
        "DeleteAlarms",
        &json!({"AlarmNames": ["deleted-alarm"]}),
    )
    .await;

    ok(router, "PutMetricStream", &put_stream("present-stream")).await;
    ok(router, "PutMetricStream", &put_stream("deleted-stream")).await;
    // Deleting twice, because the second delete is the no-op that leaves the following
    // GetMetricStream to answer for the stream being gone.
    for _ in 0..2 {
        ok(
            router,
            "DeleteMetricStream",
            &json!({"Name": "deleted-stream"}),
        )
        .await;
    }
}

#[tokio::test]
async fn a_not_found_answered_by_its_code_alone_carries_no_message() {
    let router = build_router();
    given_resources(&router).await;

    // (case, operation, request, expected error code).
    //
    // Both error shapes spell "no text" the same way — the member ABSENT:
    // `ResourceNotFound$message` is constrained to 1..255 characters, so an empty value is not
    // one it can hold at all, and the `ResourceNotFoundException` this service answers by its
    // code alone leaves its `Message` out rather than sending an empty one, so a client testing
    // for a human-readable reason is not given an empty one.
    let mut cases: Vec<(String, &str, Value, &str)> = vec![];

    // `SetAlarmState` on a name no alarm of either kind carries. Rows differ along how the
    // name misses and along the state the request asks for, which is the other member the
    // call is made of.
    for (case, name) in [
        ("an alarm name nothing was created under", "absent-alarm"),
        ("an alarm created and then deleted", "deleted-alarm"),
        ("a stored alarm name in another case", "PRESENT-ALARM"),
        ("a stored alarm name with a suffix", "present-alarm-2"),
    ] {
        for state in ["OK", "ALARM", "INSUFFICIENT_DATA"] {
            cases.push((
                format!("SetAlarmState {state} on {case}"),
                "SetAlarmState",
                json!({"AlarmName": name, "StateValue": state, "StateReason": "probe"}),
                "ResourceNotFound",
            ));
        }
    }

    // `GetMetricStream` on a name no stream carries, by the same spread of misses.
    for (case, name) in [
        ("a stream name nothing was created under", "absent-stream"),
        ("a stream created and then deleted", "deleted-stream"),
        ("a stored stream name in another case", "PRESENT-STREAM"),
        ("a stored stream name with a suffix", "present-stream-2"),
    ] {
        cases.push((
            format!("GetMetricStream on {case}"),
            "GetMetricStream",
            json!({"Name": name}),
            "ResourceNotFoundException",
        ));
    }

    // The three `ResourceARN` operations on a well-formed ARN of each taggable kind that this
    // store holds no resource for. The dashboard ARN carries no Region, as a dashboard's ARN
    // does not.
    for (case, arn) in [
        (
            "an alarm ARN nothing holds",
            format!("arn:aws:cloudwatch:{REGION}:{ACCOUNT}:alarm:no-such-alarm"),
        ),
        (
            "a metric stream ARN nothing holds",
            format!("arn:aws:cloudwatch:{REGION}:{ACCOUNT}:metric-stream/no-such-stream"),
        ),
        (
            "an insight rule ARN nothing holds",
            format!("arn:aws:cloudwatch:{REGION}:{ACCOUNT}:insight-rule/no-such-rule"),
        ),
        (
            "a dashboard ARN nothing holds",
            format!("arn:aws:cloudwatch::{ACCOUNT}:dashboard/no-such-dashboard"),
        ),
    ] {
        for operation in TAGGING_OPERATIONS {
            cases.push((
                format!("{operation} on {case}"),
                operation,
                tagging_request(operation, &arn),
                "ResourceNotFoundException",
            ));
        }
    }

    for (case, operation, request, code) in cases {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, code),
            "case {case:?}: expected HTTP 404 {code}, got {status} with {body}"
        );
        // What a client reads is empty whichever shape the refusal is …
        let message = error_message(&body);
        assert!(
            message.is_empty(),
            "case {case:?}: this refusal is answered by its code alone, so it must carry no \
             message, got {message:?} in {body}"
        );
        // … and the member that would carry it is absent rather than present and empty.
        assert!(
            body.get("message").is_none() && body.get("Message").is_none(),
            "case {case:?}: a refusal answered by its code alone states no reason, so the \
             message member must be absent rather than empty, got {body}"
        );
    }

    // The resources the store does hold are untouched by the misses: a refused read neither
    // created nor removed anything.
    let stream = ok(&router, "GetMetricStream", &json!({"Name": "present-stream"})).await;
    assert_eq!(
        stream.get("Name").and_then(Value::as_str),
        Some("present-stream"),
        "case \"after the misses\": the stored stream must still read back, got {stream}"
    );
    let described = ok(
        &router,
        "DescribeAlarms",
        &json!({"AlarmNames": ["present-alarm", "deleted-alarm", "absent-alarm"]}),
    )
    .await;
    let names: Vec<&str> = described
        .get("MetricAlarms")
        .and_then(Value::as_array)
        .map(|alarms| {
            alarms
                .iter()
                .filter_map(|alarm| alarm.get("AlarmName").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        names,
        vec!["present-alarm"],
        "case \"after the misses\": only the alarm never deleted may be reported, got {described}"
    );
}

#[tokio::test]
async fn the_not_founds_the_service_words_keep_their_own_text() {
    let router = build_router();
    ok(
        &router,
        "PutDashboard",
        &json!({"DashboardName": "present-board", "DashboardBody": "{}"}),
    )
    .await;

    // (case, operation, request, expected code, expected message)
    let mut cases: Vec<(String, &str, Value, &str, String)> = vec![];

    // A dashboard not-found names the dashboard the request asked for, in the service's own
    // sentence; the rows differ along how the requested name misses.
    for name in ["absent-board", "PRESENT-BOARD", "present-board-2"] {
        cases.push((
            format!("GetDashboard on {name:?}"),
            "GetDashboard",
            json!({"DashboardName": name}),
            "DashboardNotFoundError",
            format!("Dashboard {name} does not exist"),
        ));
    }

    // An anomaly-detector not-found is one fixed sentence that echoes nothing: the caller may
    // have named a metric rather than an identifier. The rows differ along which of the
    // accepted spellings names the detector.
    for (case, request) in [
        (
            "the flat single-metric members",
            json!({"Namespace": "MyApp", "MetricName": "Latency", "Stat": "Average"}),
        ),
        (
            "a SingleMetricAnomalyDetector",
            json!({"SingleMetricAnomalyDetector": {
                "Namespace": "MyApp",
                "MetricName": "Errors",
                "Stat": "Sum",
            }}),
        ),
    ] {
        cases.push((
            format!("DeleteAnomalyDetector naming {case}"),
            "DeleteAnomalyDetector",
            request,
            "ResourceNotFoundException",
            "Anomaly detector not found".to_string(),
        ));
    }

    for (case, operation, request, code, message) in cases {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, code),
            "case {case:?}: expected HTTP 404 {code}, got {status} with {body}"
        );
        let got = error_message(&body);
        assert_eq!(
            got, message,
            "case {case:?}: expected the service's own sentence {message:?}, got {got:?} in {body}"
        );
    }
}
