// SPDX-License-Identifier: CC0-1.0

//! What this service refuses at its ENVELOPE, and what it reports PER ITEM instead.
//!
//! The rule under test: SageMaker Metrics does not judge a request against the
//! constraints its model declares. It refuses only an envelope it cannot serve — and each
//! such refusal has its own status, error code, body member and constant sentence — while
//! everything else about a batch resolves inside a 200 response, on the per-item channel
//! the model gives it (`MetricQueryResult.Status`/`Message` for a query,
//! `BatchPutMetricsError.Code`/`MetricIndex` for a data point).
//!
//! Every test drives the real wire path (`MultiServiceRouter::dispatch` with a restJson1
//! request), so the framework's own reading of the request, this service's envelope layer
//! and both handlers are all exercised.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_sagemakermetrics_sagemakermetricsservice::ops::batch_get_metrics::{
    PERIOD_PAIRING_MESSAGE, X_AXIS_TYPE_PAIRING_MESSAGE,
};
use service_com_amazonaws_sagemakermetrics_sagemakermetricsservice::ops::types::{
    METRIC_QUERY_RESULT_STATUS_VALUES, PERIOD_VALUES, PUT_METRICS_ERROR_CODE_VALUES,
    X_AXIS_TYPE_VALUES,
};
use service_com_amazonaws_sagemakermetrics_sagemakermetricsservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// An epoch second the points in these tests are written at.
const T0: i64 = 1_700_000_000;

/// The `Period` and `XAxisType` member that keys a query to the iteration axis — the one
/// value the two enumerated types share (`PERIOD_VALUES`, `X_AXIS_TYPE_VALUES`).
const ITERATION_NUMBER: &str = "IterationNumber";

/// One emulator with this service mounted on a single shared engine, exactly as the
/// router mounts it in production.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are the framework's business and are not what these tests pin.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SageMaker trial-component ARN whose final path segment is `name`.
fn trial_component_arn(name: &str) -> String {
    format!("arn:aws:sagemaker:us-east-1:123456789012:experiment-trial-component/{name}")
}

/// What one call answered: the status, the error code (the restJson1 `__type` and the
/// `x-amzn-errortype` header, which must agree) and the parsed body.
struct Answer {
    status: u16,
    error_type: Option<String>,
    header_error_type: Option<String>,
    body: Value,
}

impl Answer {
    /// A body member as text, or `None` when the body does not carry it. Case matters:
    /// the member a refusal states its sentence in is part of the contract.
    fn member(&self, name: &str) -> Option<&str> {
        self.body.get(name).and_then(Value::as_str)
    }
}

/// Dispatch one restJson1 request whose body is `body` exactly as written.
async fn call(router: &MultiServiceRouter, method: &str, path: &str, body: &str) -> Answer {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://metrics.sagemaker.us-east-1.amazonaws.com{path}"))
        .header("host", "metrics.sagemaker.us-east-1.amazonaws.com")
        .header("content-type", "application/json")
        // The SigV4 credential scope is how the router picks the service; the signature
        // itself is not verified here (authentication is off above).
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIDEXAMPLE/20261109/us-east-1/sagemaker/aws4_request, \
             SignedHeaders=host, Signature=not-verified",
        )
        .body(bytes::Bytes::from(body.to_owned()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header_error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|e| {
            panic!(
                "{method} {path}: response body is not JSON ({e}): {:?}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    Answer {
        status,
        error_type: parsed
            .get("__type")
            .and_then(Value::as_str)
            .map(str::to_string),
        header_error_type,
        body: parsed,
    }
}

/// Dispatch one request whose body is a JSON value.
async fn send(router: &MultiServiceRouter, method: &str, path: &str, body: &Value) -> Answer {
    call(
        router,
        method,
        path,
        &serde_json::to_string(body).expect("serialize the request body"),
    )
    .await
}

/// One well-formed data point.
fn point(metric: &str, step: i32, value: f64) -> Value {
    json!({
        "MetricName": metric,
        "Timestamp": T0 + i64::from(step),
        "Step": step,
        "Value": value,
    })
}

/// One well-formed query over `component`'s `metric` series.
fn query(metric: &str, component: &str, period: &str, axis: &str) -> Value {
    json!({
        "MetricName": metric,
        "ResourceArn": trial_component_arn(component),
        "MetricStat": "Last",
        "Period": period,
        "XAxisType": axis,
    })
}

/// What one envelope refusal must answer with: the status, the body member its sentence
/// travels in, and the sentence — or no member at all.
#[derive(Debug, Clone, Copy)]
struct Refusal {
    status: u16,
    detail: Option<(&'static str, &'static str)>,
}

/// RULE: a call that carries no value for a member its operation needs is refused at the
/// envelope, with the status, the body member and the constant sentence that member earns —
/// and absent, `null`, an empty list and blank text all state the same nothing.
#[tokio::test]
async fn a_call_that_carries_no_value_for_a_member_is_refused_at_the_envelope() {
    let router = emulator();

    // The three members, each with the refusal it earns.
    let no_queries = Refusal {
        status: 500,
        detail: None,
    };
    let invalid_resource = Refusal {
        status: 400,
        detail: Some(("message", "Invalid resource.")),
    };
    let unhandled = Refusal {
        status: 500,
        detail: Some(("message", "Unhandled internal failure")),
    };

    // case, method, path, body as sent, the refusal it earns. Each member is stated as
    // absent, as null, and as an empty value of its own type, so the answer is keyed to
    // the member rather than to one spelling of "nothing".
    let cases: Vec<(&str, &str, &str, String, Refusal)> = vec![
        (
            "get: no queries member",
            "POST",
            "/BatchGetMetrics",
            json!({}).to_string(),
            no_queries,
        ),
        (
            "get: queries null",
            "POST",
            "/BatchGetMetrics",
            json!({"MetricQueries": Value::Null}).to_string(),
            no_queries,
        ),
        (
            "get: no queries at all",
            "POST",
            "/BatchGetMetrics",
            json!({"MetricQueries": []}).to_string(),
            no_queries,
        ),
        (
            "get: no body at all",
            "POST",
            "/BatchGetMetrics",
            String::new(),
            no_queries,
        ),
        (
            "put: no trial component member",
            "PUT",
            "/BatchPutMetrics",
            json!({"MetricData": [point("loss", 0, 1.0)]}).to_string(),
            invalid_resource,
        ),
        (
            "put: trial component null",
            "PUT",
            "/BatchPutMetrics",
            json!({
                "TrialComponentName": Value::Null,
                "MetricData": [point("loss", 0, 1.0)],
            })
            .to_string(),
            invalid_resource,
        ),
        (
            "put: trial component blank",
            "PUT",
            "/BatchPutMetrics",
            json!({
                "TrialComponentName": "   ",
                "MetricData": [point("loss", 0, 1.0)],
            })
            .to_string(),
            invalid_resource,
        ),
        (
            "put: no metric data member",
            "PUT",
            "/BatchPutMetrics",
            json!({"TrialComponentName": "tc-envelope"}).to_string(),
            unhandled,
        ),
        (
            "put: metric data null",
            "PUT",
            "/BatchPutMetrics",
            json!({"TrialComponentName": "tc-envelope", "MetricData": Value::Null}).to_string(),
            unhandled,
        ),
        (
            "put: no data points at all",
            "PUT",
            "/BatchPutMetrics",
            json!({"TrialComponentName": "tc-envelope", "MetricData": []}).to_string(),
            unhandled,
        ),
    ];

    for (case, method, path, body, expected) in cases {
        let answer = call(&router, method, path, &body).await;
        assert_eq!(
            answer.status, expected.status,
            "case {case:?}: expected status {}, got {} ({:?})",
            expected.status, answer.status, answer.body
        );
        match expected.detail {
            Some((member, sentence)) => assert_eq!(
                answer.member(member),
                Some(sentence),
                "case {case:?}: expected {member} {sentence:?}, got {:?}",
                answer.body
            ),
            None => {
                for member in ["message", "Message"] {
                    assert_eq!(
                        answer.member(member),
                        None,
                        "case {case:?}: this refusal states no detail, got {:?}",
                        answer.body
                    );
                }
            }
        }
        // Whatever the refusal, it never leaks the framework's constraint-walk sentence.
        for member in ["message", "Message"] {
            assert!(
                answer
                    .member(member)
                    .is_none_or(|text| !text.contains("validation error")),
                "case {case:?}: the per-member constraint envelope must not reach the \
                 caller, got {:?}",
                answer.body
            );
        }
    }
}

/// RULE: a value whose JSON type its modelled shape cannot be read from never became a
/// value, so it is refused as a DESERIALIZATION failure: HTTP 400 with the sentence in the
/// body member spelled `Message`, naming the CLASS of JSON that arrived, and an error type
/// that is not the constraint one.
#[tokio::test]
async fn a_member_of_the_wrong_json_type_is_refused_as_a_deserialization_failure() {
    let router = emulator();

    // case, method, path, body as sent, the class of value the sentence names.
    let cases: Vec<(&str, &str, &str, String, &str)> = vec![
        (
            "a string where a list is modelled",
            "PUT",
            "/BatchPutMetrics",
            json!({"TrialComponentName": "tc-types", "MetricData": "STRING_VALUE"}).to_string(),
            "STRING_VALUE",
        ),
        (
            "a number where a list is modelled",
            "POST",
            "/BatchGetMetrics",
            json!({"MetricQueries": 7}).to_string(),
            "NUMBER_VALUE",
        ),
        (
            "an object where a list is modelled",
            "POST",
            "/BatchGetMetrics",
            json!({"MetricQueries": {"MetricName": "loss"}}).to_string(),
            "OBJECT_VALUE",
        ),
        (
            "a list where a name is modelled",
            "PUT",
            "/BatchPutMetrics",
            json!({
                "TrialComponentName": ["tc-types"],
                "MetricData": [point("loss", 0, 1.0)],
            })
            .to_string(),
            "ARRAY_VALUE",
        ),
        (
            "a flag where a name is modelled",
            "PUT",
            "/BatchPutMetrics",
            json!({
                "TrialComponentName": true,
                "MetricData": [point("loss", 0, 1.0)],
            })
            .to_string(),
            "BOOLEAN_VALUE",
        ),
        (
            // Nested inside a batch item: a measurement sent as text.
            "a string where a point's value is modelled",
            "PUT",
            "/BatchPutMetrics",
            json!({
                "TrialComponentName": "tc-types",
                "MetricData": [{
                    "MetricName": "loss",
                    "Timestamp": T0,
                    "Step": 0,
                    "Value": "not-a-number",
                }],
            })
            .to_string(),
            "STRING_VALUE",
        ),
        (
            "a flag where a query's name is modelled",
            "POST",
            "/BatchGetMetrics",
            json!({"MetricQueries": [{"MetricName": false}]}).to_string(),
            "BOOLEAN_VALUE",
        ),
        (
            "a batch item that is not an object",
            "PUT",
            "/BatchPutMetrics",
            json!({"TrialComponentName": "tc-types", "MetricData": [7]}).to_string(),
            "NUMBER_VALUE",
        ),
    ];

    for (case, method, path, body, class) in cases {
        let answer = call(&router, method, path, &body).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected status 400, got {} ({:?})",
            answer.status, answer.body
        );
        assert_eq!(
            answer.member("Message"),
            Some(format!("{class} is not an expected Json type").as_str()),
            "case {case:?}: expected the sentence under Message, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.member("message"),
            None,
            "case {case:?}: this class states nothing under the lowercase member, got {:?}",
            answer.body
        );
        assert_ne!(
            answer.error_type.as_deref(),
            Some("ValidationException"),
            "case {case:?}: a value that never became a value is not a constraint \
             violation, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.header_error_type, answer.error_type,
            "case {case:?}: the header and the body must name one error type, got {:?} / {:?}",
            answer.header_error_type, answer.error_type
        );
    }
}

/// RULE: `Period` and `XAxisType` are iteration-keyed together or neither is; a query that
/// pairs them otherwise is that query's own `ValidationError`, with the sentence naming
/// whichever of the two IS iteration-keyed, and carries no value lists at all — while its
/// siblings in the same 200 response are answered normally.
///
/// The table is every `Period` x `XAxisType` the model lists (`PERIOD_VALUES` x
/// `X_AXIS_TYPE_VALUES`), so both directions of the mismatch and every agreeing pairing are
/// covered rather than one recorded row.
#[tokio::test]
async fn a_period_and_axis_that_disagree_about_iterations_fail_that_query_alone() {
    let router = emulator();
    let component = "tc-pairing";
    let put = send(
        &router,
        "PUT",
        "/BatchPutMetrics",
        &json!({
            "TrialComponentName": component,
            "MetricData": [point("paired", 0, 6.0)],
        }),
    )
    .await;
    assert_eq!(
        put.status, 200,
        "the baseline point must be ingested, got {} ({:?})",
        put.status, put.body
    );

    for period in PERIOD_VALUES {
        for axis in X_AXIS_TYPE_VALUES {
            let case = format!("{period}/{axis}");
            // A well-formed sibling at index 0 and the pairing under test at index 1, so a
            // failure is shown to be positional.
            let answer = send(
                &router,
                "POST",
                "/BatchGetMetrics",
                &json!({"MetricQueries": [
                    query("paired", component, ITERATION_NUMBER, ITERATION_NUMBER),
                    query("paired", component, period, axis),
                ]}),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: a query's fate is reported inside a 200, got {} ({:?})",
                answer.status, answer.body
            );
            let results = answer.body["MetricQueryResults"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected MetricQueryResults, got {:?}", answer.body)
                });
            assert_eq!(
                results.len(),
                2,
                "case {case:?}: one result per query, got {results:?}"
            );
            assert_eq!(
                results[0]["Status"].as_str(),
                Some("Complete"),
                "case {case:?}: the sibling query is answered normally, got {:?}",
                results[0]
            );
            let result = &results[1];
            let status = result["Status"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a result carries a Status: {result:?}"));
            assert!(
                METRIC_QUERY_RESULT_STATUS_VALUES.contains(&status),
                "case {case:?}: status {status:?} is not a modelled MetricQueryResultStatus"
            );

            let period_is_iteration = *period == ITERATION_NUMBER;
            let axis_is_iteration = *axis == ITERATION_NUMBER;
            let expected_sentence = match (period_is_iteration, axis_is_iteration) {
                // Each sentence names the member that has to change for the two to agree.
                (true, false) => Some(X_AXIS_TYPE_PAIRING_MESSAGE),
                (false, true) => Some(PERIOD_PAIRING_MESSAGE),
                _ => None,
            };
            match expected_sentence {
                Some(sentence) => {
                    assert_eq!(
                        status, "ValidationError",
                        "case {case:?}: expected Status ValidationError, got {result:?}"
                    );
                    assert_eq!(
                        result["Message"].as_str(),
                        Some(sentence),
                        "case {case:?}: expected Message {sentence:?}, got {result:?}"
                    );
                    for member in ["XAxisValues", "MetricValues"] {
                        assert_eq!(
                            result.get(member),
                            None,
                            "case {case:?}: a failing result carries no {member}, got {result:?}"
                        );
                    }
                }
                None => {
                    assert_eq!(
                        status, "Complete",
                        "case {case:?}: an agreeing pairing is evaluated, got {result:?}"
                    );
                    for member in ["XAxisValues", "MetricValues"] {
                        assert!(
                            result[member].is_array(),
                            "case {case:?}: an answered query carries {member}, got {result:?}"
                        );
                    }
                }
            }
        }
    }
}

/// RULE: `Errors` is on every answer of `BatchPutMetrics`, as the empty list when every
/// point was ingested — its presence is not conditional on a failure.
#[tokio::test]
async fn every_put_reports_errors_even_when_nothing_failed() {
    let router = emulator();

    // case, the batch as sent — one point, several points, and a point that leaves out
    // everything the request need not state.
    let batches: Vec<(&str, Value)> = vec![
        ("one point", json!([point("loss", 0, 1.0)])),
        (
            "several points",
            json!([point("loss", 1, 2.0), point("loss", 2, 3.0)]),
        ),
        (
            "a point with no step",
            json!([{"MetricName": "loss", "Timestamp": T0 + 9, "Value": 4.0}]),
        ),
        (
            "a point that states only a name and a value",
            json!([{"MetricName": "loss", "Value": 5.0}]),
        ),
        (
            "a point whose value is infinite",
            json!([{"MetricName": "loss", "Timestamp": T0 + 11, "Value": "Infinity"}]),
        ),
    ];

    for (case, data) in batches {
        let answer = send(
            &router,
            "PUT",
            "/BatchPutMetrics",
            &json!({"TrialComponentName": "tc-errors", "MetricData": data}),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected status 200, got {} ({:?})",
            answer.status, answer.body
        );
        let errors = answer.body["Errors"].as_array().unwrap_or_else(|| {
            panic!("case {case:?}: Errors is on every answer, got {:?}", answer.body)
        });
        assert!(
            errors.is_empty(),
            "case {case:?}: every point was ingested, got {errors:?}"
        );
    }
}

/// RULE: a point that never became a measurement is reported at ITS index with the code
/// this service states for that class, while every sibling point in the same batch is
/// ingested and readable back — and a point that IS a measurement (an infinite value is a
/// number) is stored.
///
/// The forms a request can carry a non-finite value in are the quoted ones this protocol's
/// codec reads a float from (`NaN`, `Infinity`, `-Infinity`); a bare non-finite literal is
/// not JSON at all.
#[tokio::test]
async fn a_point_that_cannot_be_stored_is_reported_at_its_index() {
    let router = emulator();
    let component = "tc-unstorable";

    // case, the name and the value (as JSON) the second point states, whether it is
    // reported at its index. A non-finite value travels quoted, which is how a request
    // carries one.
    let cases: [(&str, &str, &str, bool); 5] = [
        ("not a number", "series", "\"NaN\"", true),
        ("unbounded above", "series", "\"Infinity\"", false),
        ("unbounded below", "series", "\"-Infinity\"", false),
        ("no name at all", "", "2.5", true),
        ("a name with nothing in it", "   ", "2.5", true),
    ];

    for (index, (case, name, value, reported)) in cases.into_iter().enumerate() {
        let metric = format!("series-{index}");
        let offending_name = if name == "series" {
            metric.clone()
        } else {
            name.to_string()
        };
        let body = format!(
            r#"{{"TrialComponentName":"{component}","MetricData":[
                 {{"MetricName":"{metric}","Timestamp":{T0},"Step":0,"Value":1.5}},
                 {{"MetricName":"{offending_name}","Timestamp":{T0},"Step":1,"Value":{value}}}
               ]}}"#
        );
        let answer = call(&router, "PUT", "/BatchPutMetrics", &body).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a point's fate is reported inside a 200, got {} ({:?})",
            answer.status, answer.body
        );
        let errors = answer.body["Errors"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected Errors, got {:?}", answer.body));
        if !reported {
            assert!(
                errors.is_empty(),
                "case {case:?}: a point that IS a measurement is stored, got {errors:?}"
            );
            continue;
        }
        assert_eq!(
            errors.len(),
            1,
            "case {case:?}: only the offending point is reported, got {errors:?}"
        );
        assert_eq!(
            errors[0]["MetricIndex"].as_i64(),
            Some(1),
            "case {case:?}: the entry names the offending point's index, got {errors:?}"
        );
        let code = errors[0]["Code"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: an entry carries a Code: {errors:?}"));
        assert!(
            PUT_METRICS_ERROR_CODE_VALUES.contains(&code),
            "case {case:?}: code {code:?} is not a modelled PutMetricsErrorCode"
        );
        assert_eq!(
            code, "INTERNAL_ERROR",
            "case {case:?}: expected INTERNAL_ERROR, got {code:?}"
        );

        // The sibling point was ingested and reads back; the refused one left nothing.
        let read = send(
            &router,
            "POST",
            "/BatchGetMetrics",
            &json!({"MetricQueries": [
                query(&metric, component, ITERATION_NUMBER, ITERATION_NUMBER),
            ]}),
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the read-back must be served, got {} ({:?})",
            read.status, read.body
        );
        let result = &read.body["MetricQueryResults"][0];
        assert_eq!(
            result["XAxisValues"].as_array().map(Vec::len),
            Some(1),
            "case {case:?}: only the sibling point is stored, got {result:?}"
        );
        assert_eq!(
            result["MetricValues"][0].as_f64(),
            Some(1.5),
            "case {case:?}: the sibling point's value reads back, got {result:?}"
        );
    }
}

/// RULE: the bounds this emulator sets for its own resources are the only ones left once the
/// modelled `@length` of a batch is off the request boundary — one call serves at most
/// `MAX_ITEMS_PER_REQUEST` items, and the store holds no name longer than `MAX_NAME_BYTES`.
/// Both are well above what either model declares, so no request the model admits meets
/// them; past them the work is refused rather than done.
#[tokio::test]
async fn the_bounds_this_emulator_sets_for_itself_are_enforced() {
    use service_com_amazonaws_sagemakermetrics_sagemakermetricsservice::state::{
        MAX_ITEMS_PER_REQUEST, MAX_NAME_BYTES,
    };

    let router = emulator();
    let component = "tc-bounds";

    // case, how many items, the status the call is answered at.
    let batches: [(&str, usize, u16); 4] = [
        ("one item", 1, 200),
        ("items at the bound", MAX_ITEMS_PER_REQUEST, 200),
        ("one item past the bound", MAX_ITEMS_PER_REQUEST + 1, 500),
        ("far past the bound", MAX_ITEMS_PER_REQUEST * 3, 500),
    ];
    for (case, items, expected) in batches {
        let data: Vec<Value> = (0..items)
            .map(|step| point("bounded", i32::try_from(step).expect("a small step"), 1.0))
            .collect();
        let answer = send(
            &router,
            "PUT",
            "/BatchPutMetrics",
            &json!({"TrialComponentName": component, "MetricData": data}),
        )
        .await;
        assert_eq!(
            answer.status, expected,
            "case {case:?} ({items} point(s)): expected status {expected}, got {} ({:?})",
            answer.status, answer.body
        );
        let queries: Vec<Value> = (0..items)
            .map(|_| query("bounded", component, ITERATION_NUMBER, ITERATION_NUMBER))
            .collect();
        let answer = send(
            &router,
            "POST",
            "/BatchGetMetrics",
            &json!({"MetricQueries": queries}),
        )
        .await;
        assert_eq!(
            answer.status, expected,
            "case {case:?} ({items} query/queries): expected status {expected}, got {} ({:?})",
            answer.status, answer.body
        );
    }

    // A name longer than the store holds is refused for its own index, and stores nothing;
    // the sibling point beside it is ingested.
    let long_name = "n".repeat(MAX_NAME_BYTES + 1);
    let answer = send(
        &router,
        "PUT",
        "/BatchPutMetrics",
        &json!({
            "TrialComponentName": component,
            "MetricData": [point("short", 0, 1.0), {
                "MetricName": long_name,
                "Timestamp": T0,
                "Step": 1,
                "Value": 2.0,
            }],
        }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "a point's fate is reported inside a 200, got {} ({:?})",
        answer.status, answer.body
    );
    let errors = answer.body["Errors"]
        .as_array()
        .unwrap_or_else(|| panic!("expected Errors, got {:?}", answer.body));
    assert_eq!(
        errors.len(),
        1,
        "only the offending point is reported, got {errors:?}"
    );
    assert_eq!(
        (errors[0]["Code"].as_str(), errors[0]["MetricIndex"].as_i64()),
        (Some("METRIC_LIMIT_EXCEEDED"), Some(1)),
        "expected METRIC_LIMIT_EXCEEDED at the offending index, got {errors:?}"
    );
    let read = send(
        &router,
        "POST",
        "/BatchGetMetrics",
        &json!({"MetricQueries": [
            query(&long_name, component, ITERATION_NUMBER, ITERATION_NUMBER),
            query("short", component, ITERATION_NUMBER, ITERATION_NUMBER),
        ]}),
    )
    .await;
    assert_eq!(
        read.body["MetricQueryResults"][0]["MetricValues"]
            .as_array()
            .map(Vec::len),
        Some(0),
        "the refused point stored nothing, got {:?}",
        read.body["MetricQueryResults"][0]
    );
    assert_eq!(
        read.body["MetricQueryResults"][1]["MetricValues"][0].as_f64(),
        Some(1.0),
        "the sibling point reads back, got {:?}",
        read.body["MetricQueryResults"][1]
    );
}

/// RULE: an item that states less than the model requires is an item with those members
/// UNSET — a query that names no series selects nothing, a point is stored from what it did
/// say — rather than a refusal of the whole call. A point that states no MEASUREMENT is the
/// one exception, reported at its own index like any other value that is not a number.
#[tokio::test]
async fn an_item_that_states_less_than_the_model_requires_is_still_served() {
    let router = emulator();
    let component = "tc-partial-items";

    // A point that states only a name and a value: the instant it did not state is the
    // epoch, so it is stored and reads back there.
    let put = send(
        &router,
        "PUT",
        "/BatchPutMetrics",
        &json!({
            "TrialComponentName": component,
            "MetricData": [
                {"MetricName": "sparse", "Value": 2.5},
                // ...while a point that states no value at all never became a measurement.
                {"MetricName": "valueless", "Timestamp": T0},
            ],
        }),
    )
    .await;
    assert_eq!(
        put.status, 200,
        "a point that states less is still ingested, got {} ({:?})",
        put.status, put.body
    );
    assert_eq!(
        (
            put.body["Errors"][0]["Code"].as_str(),
            put.body["Errors"][0]["MetricIndex"].as_i64()
        ),
        (Some("INTERNAL_ERROR"), Some(1)),
        "the point that states no measurement is reported at its index, got {:?}",
        put.body
    );
    assert_eq!(
        put.body["Errors"].as_array().map(Vec::len),
        Some(1),
        "only the point that states no measurement is reported, got {:?}",
        put.body
    );
    let read = send(
        &router,
        "POST",
        "/BatchGetMetrics",
        &json!({"MetricQueries": [
            query("sparse", component, "OneHour", "Timestamp"),
        ]}),
    )
    .await;
    let result = &read.body["MetricQueryResults"][0];
    assert_eq!(
        result["MetricValues"][0].as_f64(),
        Some(2.5),
        "the ingested point reads back, got {result:?}"
    );
    assert_eq!(
        result["XAxisValues"][0].as_i64(),
        Some(0),
        "the instant it did not state is the epoch, got {result:?}"
    );

    // case, the query as sent — each leaving out one of the members that say what to read.
    let partial: Vec<(&str, Value)> = vec![
        (
            "no metric name",
            json!({
                "ResourceArn": trial_component_arn(component),
                "MetricStat": "Last",
                "Period": ITERATION_NUMBER,
                "XAxisType": ITERATION_NUMBER,
            }),
        ),
        (
            "no statistic",
            json!({
                "MetricName": "sparse",
                "ResourceArn": trial_component_arn(component),
                "Period": ITERATION_NUMBER,
                "XAxisType": ITERATION_NUMBER,
            }),
        ),
        (
            "no period or axis",
            json!({
                "MetricName": "sparse",
                "ResourceArn": trial_component_arn(component),
                "MetricStat": "Last",
            }),
        ),
        ("nothing at all", json!({})),
        (
            "every member null",
            json!({
                "MetricName": Value::Null,
                "ResourceArn": Value::Null,
                "MetricStat": Value::Null,
                "Period": Value::Null,
                "XAxisType": Value::Null,
            }),
        ),
    ];

    for (case, sent) in partial {
        let answer = send(
            &router,
            "POST",
            "/BatchGetMetrics",
            &json!({"MetricQueries": [sent]}),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a query that states less is served, got {} ({:?})",
            answer.status, answer.body
        );
        let result = &answer.body["MetricQueryResults"][0];
        assert_eq!(
            result["Status"].as_str(),
            Some("Complete"),
            "case {case:?}: it selects nothing, which is Complete, got {result:?}"
        );
        for member in ["XAxisValues", "MetricValues"] {
            assert_eq!(
                result[member].as_array().map(Vec::len),
                Some(0),
                "case {case:?}: it selected nothing, got {result:?}"
            );
        }
    }
}

/// The width of the `OneMinute` period, in seconds: a bucket is keyed by its own start,
/// `floor(epoch_second / width) * width`, so the instant a point was stored at is what a
/// read on the `Timestamp` axis reports it under.
const ONE_MINUTE: i64 = 60;

/// The measurement each point of the instant-text table carries.
const MEASUREMENT: f64 = 1.5;

/// RULE: a string bound to a timestamp member became an instant only when it PARSES in the
/// format bound to that member — this protocol's epoch second for a member the model binds
/// no `@timestampFormat` to, which is every timestamp this service declares
/// (`smithy_query(query="shape",shape="RawMetricData,Timestamp")` reports no format on the
/// member or on the shape it targets). Text that parses is read, and the point is stored at
/// the instant it names; text that does not never became a value at all, so it earns the
/// deserialization refusal of this class — HTTP 400, the sentence naming the CLASS of JSON
/// that arrived under `Message`, nothing under the lowercase member, and an error type that
/// is not the constraint one — and nothing is ingested.
///
/// The rows span each published spelling of an instant (epoch seconds, ISO-8601 date-time,
/// HTTP date) and the text classes around them, so the rule is pinned to the format bound to
/// the member rather than to one spelling.
#[tokio::test]
async fn a_points_instant_reads_from_text_only_in_the_format_bound_to_the_member() {
    let router = emulator();
    let component = "tc-instant-text";

    // case, the text a point states its instant as, the epoch second that text names —
    // `None` for text the bound format does not parse.
    let rows: [(&str, &str, Option<i64>); 7] = [
        ("epoch seconds", "1700000000", Some(1_700_000_000)),
        ("the epoch itself", "0", Some(0)),
        (
            "a fraction of a second past an epoch second",
            "1700000000.5",
            Some(1_700_000_000),
        ),
        ("an ISO-8601 date-time", "2023-11-14T22:13:20Z", None),
        ("an HTTP date", "Tue, 14 Nov 2023 22:13:20 GMT", None),
        ("text that is no instant at all", "not-a-time", None),
        ("text with nothing in it", "", None),
    ];

    for (index, (case, text, instant)) in rows.into_iter().enumerate() {
        // Its own series per row, so what a row stored is read back on its own.
        let metric = format!("instant-{index}");
        let answer = send(
            &router,
            "PUT",
            "/BatchPutMetrics",
            &json!({
                "TrialComponentName": component,
                "MetricData": [{
                    "MetricName": metric,
                    "Timestamp": text,
                    "Step": 0,
                    "Value": MEASUREMENT,
                }],
            }),
        )
        .await;

        match instant {
            Some(_) => {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: text the bound format parses is read, got {} ({:?})",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.body["Errors"].as_array().map(Vec::len),
                    Some(0),
                    "case {case:?}: the point was ingested, got {:?}",
                    answer.body
                );
            }
            None => {
                assert_eq!(
                    answer.status, 400,
                    "case {case:?}: text no format reads never became a value, got {} ({:?})",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.member("Message"),
                    Some("STRING_VALUE is not an expected Json type"),
                    "case {case:?}: expected the class under Message, got {:?}",
                    answer.body
                );
                assert_eq!(
                    answer.member("message"),
                    None,
                    "case {case:?}: this class states nothing under the lowercase member, \
                     got {:?}",
                    answer.body
                );
                assert_ne!(
                    answer.error_type.as_deref(),
                    Some("ValidationException"),
                    "case {case:?}: a value that never became a value is not a constraint \
                     violation, got {:?}",
                    answer.body
                );
                assert_eq!(
                    answer.header_error_type, answer.error_type,
                    "case {case:?}: the header and the body must name one error type, got \
                     {:?} / {:?}",
                    answer.header_error_type, answer.error_type
                );
            }
        }

        // What the series holds now: the point at the instant the text named, or nothing at
        // all for a call that never became an input. The read is on the `Timestamp` axis, so
        // the bucket it reports is computed from the stored instant rather than from the
        // request.
        let read = send(
            &router,
            "POST",
            "/BatchGetMetrics",
            &json!({"MetricQueries": [query(&metric, component, "OneMinute", "Timestamp")]}),
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the read-back must be served, got {} ({:?})",
            read.status, read.body
        );
        let result = &read.body["MetricQueryResults"][0];
        match instant {
            Some(epoch_second) => {
                let bucket = epoch_second.div_euclid(ONE_MINUTE) * ONE_MINUTE;
                assert_eq!(
                    result["XAxisValues"].as_array().map(Vec::len),
                    Some(1),
                    "case {case:?}: one point is stored, got {result:?}"
                );
                assert_eq!(
                    result["XAxisValues"][0].as_i64(),
                    Some(bucket),
                    "case {case:?}: expected the point at the minute of epoch second \
                     {epoch_second}, got {result:?}"
                );
                assert_eq!(
                    result["MetricValues"][0].as_f64(),
                    Some(MEASUREMENT),
                    "case {case:?}: the measurement reads back, got {result:?}"
                );
            }
            None => {
                for member in ["XAxisValues", "MetricValues"] {
                    assert_eq!(
                        result[member].as_array().map(Vec::len),
                        Some(0),
                        "case {case:?}: a refused call ingested nothing, got {result:?}"
                    );
                }
            }
        }
    }
}

/// RULE: a bound on the WORK one call may ask for is decided from the request at the level
/// it is stated — the COUNT of a batch member — and refuses the call before any ITEM of that
/// batch is read, so a batch past `MAX_ITEMS_PER_REQUEST` is refused for its size whatever
/// its items carry, while the per-item deserialization refusal is reached only for a batch
/// this service would serve. The body's own top level is read first, so the bound hides no
/// unreadable member of the envelope itself.
///
/// The rows span both operations, both batch members, the unreadable value at the first, a
/// middle and the last index, each class of unreadable value (a number, text, an item that is
/// no object at all), and the batch size at the bound, one past it and far past it — the
/// recorded case is the single row "one query past the bound, the unreadable query first".
#[tokio::test]
async fn the_item_bound_is_decided_before_any_item_of_the_batch_is_read() {
    use service_com_amazonaws_sagemakermetrics_sagemakermetricsservice::state::MAX_ITEMS_PER_REQUEST;

    let router = emulator();
    let component = "tc-precedence";
    let bound = MAX_ITEMS_PER_REQUEST;

    // The two refusals whose precedence this pins, each with the status, body member and
    // constant sentence it carries.
    let over_sized = Refusal {
        status: 500,
        detail: Some(("message", "Unhandled internal failure")),
    };
    let unreadable = |sentence: &'static str| Refusal {
        status: 400,
        detail: Some(("Message", sentence)),
    };
    // The two sentences the unreadable rows below earn, each naming the CLASS of JSON that
    // arrived where a readable value was modelled.
    let number_arrived = "NUMBER_VALUE is not an expected Json type";
    let text_arrived = "STRING_VALUE is not an expected Json type";
    let served = Refusal {
        status: 200,
        detail: None,
    };

    // A batch of `items` well-formed points, the one at `bad` carrying a measurement that is
    // not a number — a value its member cannot be read from.
    let points = |items: usize, bad: Option<usize>| -> Value {
        let data: Vec<Value> = (0..items)
            .map(|step| {
                let mut datum = point("precedence", i32::try_from(step).expect("a small step"), 1.0);
                if bad == Some(step) {
                    datum["Value"] = json!("not-a-number");
                }
                datum
            })
            .collect();
        Value::Array(data)
    };
    // A batch of `items` well-formed queries, the one at `bad` naming its series with a
    // number.
    let queries = |items: usize, bad: Option<usize>| -> Value {
        let batch: Vec<Value> = (0..items)
            .map(|index| {
                let mut one = query("precedence", component, ITERATION_NUMBER, ITERATION_NUMBER);
                if bad == Some(index) {
                    one["MetricName"] = json!(7);
                }
                one
            })
            .collect();
        Value::Array(batch)
    };
    let get = |batch: Value| json!({"MetricQueries": batch});
    let put = |batch: Value| json!({"TrialComponentName": component, "MetricData": batch});

    // case, method, path, body as sent, the answer it earns.
    let rows: Vec<(&str, &str, &str, Value, Refusal)> = vec![
        (
            "a get at the bound, every query readable",
            "POST",
            "/BatchGetMetrics",
            get(queries(bound, None)),
            served,
        ),
        (
            "a get at the bound with an unreadable query",
            "POST",
            "/BatchGetMetrics",
            get(queries(bound, Some(0))),
            unreadable(number_arrived),
        ),
        (
            "a get one query past the bound, every query readable",
            "POST",
            "/BatchGetMetrics",
            get(queries(bound + 1, None)),
            over_sized,
        ),
        (
            "a get one query past the bound, the unreadable query first",
            "POST",
            "/BatchGetMetrics",
            get(queries(bound + 1, Some(0))),
            over_sized,
        ),
        (
            "a get one query past the bound, the unreadable query last",
            "POST",
            "/BatchGetMetrics",
            get(queries(bound + 1, Some(bound))),
            over_sized,
        ),
        (
            "a get far past the bound, the unreadable query in the middle",
            "POST",
            "/BatchGetMetrics",
            get(queries(bound * 3, Some(bound * 3 / 2))),
            over_sized,
        ),
        (
            "a get past the bound whose batch member is no list at all",
            "POST",
            "/BatchGetMetrics",
            get(json!("STRING_VALUE")),
            unreadable(text_arrived),
        ),
        (
            "a put at the bound with an unreadable point in the middle",
            "PUT",
            "/BatchPutMetrics",
            put(points(bound, Some(bound / 2))),
            unreadable(text_arrived),
        ),
        (
            "a put one point past the bound, the unreadable point last",
            "PUT",
            "/BatchPutMetrics",
            put(points(bound + 1, Some(bound))),
            over_sized,
        ),
        (
            "a put far past the bound, the unreadable point first",
            "PUT",
            "/BatchPutMetrics",
            put(points(bound * 3, Some(0))),
            over_sized,
        ),
        (
            "a put past the bound whose items are no objects at all",
            "PUT",
            "/BatchPutMetrics",
            put(Value::Array(vec![json!(7); bound + 1])),
            over_sized,
        ),
        (
            // The top level is read before the count, so the envelope's own unreadable
            // member still wins over the size of the batch beside it.
            "a put past the bound that also names its component with a number",
            "PUT",
            "/BatchPutMetrics",
            json!({"TrialComponentName": 7, "MetricData": points(bound + 1, None)}),
            unreadable(number_arrived),
        ),
    ];

    for (case, method, path, body, expected) in rows {
        let answer = send(&router, method, path, &body).await;
        assert_eq!(
            answer.status, expected.status,
            "case {case:?}: expected status {}, got {} ({:?})",
            expected.status, answer.status, answer.body
        );
        match expected.detail {
            Some((member, sentence)) => {
                assert_eq!(
                    answer.member(member),
                    Some(sentence),
                    "case {case:?}: expected {member} {sentence:?}, got {:?}",
                    answer.body
                );
                let other = if member == "Message" {
                    "message"
                } else {
                    "Message"
                };
                assert_eq!(
                    answer.member(other),
                    None,
                    "case {case:?}: this refusal states nothing under {other}, got {:?}",
                    answer.body
                );
            }
            None => {
                for member in ["message", "Message"] {
                    assert_eq!(
                        answer.member(member),
                        None,
                        "case {case:?}: a served call states no refusal sentence, got {:?}",
                        answer.body
                    );
                }
            }
        }
    }

    // Every refusal above is an envelope refusal, so no handler ran for it and nothing it
    // carried was ingested: the series holds only what the one well-formed batch wrote.
    let written = send(&router, "PUT", "/BatchPutMetrics", &put(points(bound, None))).await;
    assert_eq!(
        written.status, 200,
        "a batch at the bound is served, got {} ({:?})",
        written.status, written.body
    );
    let read = send(&router, "POST", "/BatchGetMetrics", &get(queries(1, None))).await;
    assert_eq!(
        read.body["MetricQueryResults"][0]["MetricValues"]
            .as_array()
            .map(Vec::len),
        Some(bound),
        "only the served batch stored anything, got {:?}",
        read.body["MetricQueryResults"][0]
    );
}
