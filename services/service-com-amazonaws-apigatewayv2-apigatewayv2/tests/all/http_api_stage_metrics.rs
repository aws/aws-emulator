// SPDX-License-Identifier: CC0-1.0

//! What a deployed HTTP api stage's own metrics DECIDE: the CloudWatch alarm declared over
//! the series its traffic is published into.
//!
//! The rule under test, one level above the recorded case (an alarm on an api's `5xx` series
//! whose `AWS_PROXY` integration answers `500`, one copy with its actions enabled and one
//! with them disabled):
//!
//! > The datapoints a stage publishes about the requests it answered are the ones an alarm
//! > over those series is evaluated FROM, so storing one is what makes that alarm's
//! > evaluation due: an alarm whose series the stage's traffic breaches transitions to
//! > `ALARM` from that traffic alone, with no further call of any kind, and records the
//! > transition in its history — whatever its `ActionsEnabled` says, since that member
//! > governs the actions a transition fires and not the evaluation that finds it. An alarm
//! > whose series the traffic does NOT breach is graded by the same datapoints and reaches
//! > `OK`, which is what tells "evaluated and not breaching" from "never evaluated".
//!
//! Only a cross-service suite can pin this: the api, its stage and the requests they answer
//! are this service's, the answer a route is served with belongs to a function AWS Lambda
//! holds and runs, and the series and the alarm over them are CloudWatch's — all three on ONE
//! engine, as `core/router/src/lib.rs` mounts them. Every assertion reads the outcome back
//! through CloudWatch's own readers (`GetMetricStatistics`, `DescribeAlarms`,
//! `DescribeAlarmHistory`), so an alarm that was not really evaluated cannot pass.
//!
//! The rows differ along what the rule generalises over: WHICH series the alarm watches (the
//! server-error counter, the client-error counter, the request counter), WHAT drives the
//! traffic into it (an integration that answers a server error, a path no route of the api
//! matches, plain served requests), whether the alarm's actions are enabled, and whether the
//! traffic breaches at all.
//!
//! What a transition then DELIVERS — the notification an enabled alarm publishes to its action
//! and the suppression its actions-disabled twin owes — belongs to the alarm rather than to
//! the api, and is pinned over those same two arms by its owner:
//! `.../service-com-amazonaws-cloudwatch-graniteserviceversion20100801/tests/all/`
//! `actions_disabled_evaluation_delivery.rs` and `alarm_action_dispatch.rs`. What each request
//! contributes to which series is pinned by `http_api_request_metrics`; this suite is about
//! what those datapoints then DECIDE.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::http_api_backend_dispatch::{given_function, lambda_integration};
use crate::websocket_handshake_data_plane::send;

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// The namespace API Gateway publishes what its apis did in, and the dimensions a stage's own
/// traffic is counted under.
const NAMESPACE: &str = "AWS/ApiGateway";
const API_ID_DIMENSION: &str = "ApiId";
const STAGE_DIMENSION: &str = "Stage";

/// The series a row's alarm may watch: the request counter and the two error counters (an HTTP
/// api's own lower-case names).
const COUNT: &str = "Count";
const CLIENT_ERRORS: &str = "4xx";
const SERVER_ERRORS: &str = "5xx";

/// The stage every case deploys its routes to.
const STAGE: &str = "live";

/// The two routes the api serves: one the function answers `200` on, one it answers `500` on.
const OK_ROUTE: &str = "GET /ok";
const BOOM_ROUTE: &str = "GET /boom";

/// The handler behind both routes: the status it answers is decided by the path it was reached
/// at, so one function serves both the success and the server-error arm.
const HANDLER_SOURCE: &str = "import json\n\
                              def lambda_handler(event, context):\n\
                              \x20   path = event.get('rawPath') or ''\n\
                              \x20   status = 500 if 'boom' in path else 200\n\
                              \x20   return {'statusCode': status,\n\
                              \x20           'headers': {'content-type': 'application/json'},\n\
                              \x20           'body': json.dumps({'path': path})}\n";

/// How long a row waits for the state its traffic implies, and how often it looks again.
///
/// An alarm's evaluation is owed a span after the datapoints that make it due arrive, and its
/// evaluated range ends one cadence behind the instant it runs, so the transition is not
/// immediate. The deadline is far above either span, so a row that never reaches its state
/// fails on what it last read instead of hanging.
const SETTLES_WITHIN: Duration = Duration::from_secs(60);
/// See [`SETTLES_WITHIN`].
const LOOK_AGAIN: Duration = Duration::from_millis(100);

/// The window every metric read covers, in seconds, and the period it is read at: generous on
/// both sides, so no row depends on where in a minute the suite runs.
const WINDOW_SECONDS: f64 = 900.0;
const AHEAD_SECONDS: f64 = 120.0;
const PERIOD_SECONDS: i64 = 60;

/// The three services this rule spans, over ONE engine, as `core/router/src/lib.rs` mounts
/// them.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_apigatewayv2_apigatewayv2::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    (router, engine)
}

/// One control-plane request against THIS service, signed for [`REGION`].
async fn control(
    emulator: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = match &body {
        Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("the body serializes")),
        None => bytes::Bytes::new(),
    };
    let host = format!("apigateway.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{uri}"))
        .header("host", &host)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTAPIGWV2/20261021/{REGION}/apigateway/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(payload)
        .expect("the request builds");
    answered(emulator, request).await
}

/// One CloudWatch call, over the wire that service speaks (`awsJson1_0`, `POST /` with
/// `X-Amz-Target`).
async fn cloudwatch(emulator: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header(
            "x-amz-target",
            format!("GraniteServiceVersion20100801.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTAPIGWV2/20261021/{REGION}/monitoring/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the body serializes"),
        ))
        .expect("the request builds");
    answered(emulator, request).await
}

/// The status and parsed body of one dispatched request.
async fn answered(
    emulator: &MultiServiceRouter,
    request: http::Request<bytes::Bytes>,
) -> (u16, Value) {
    let response = emulator.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// An api whose two routes front one Lambda integration, deployed to a [`STAGE`] stage with
/// auto-deploy on. Answers `(ApiId, the host its `ApiEndpoint` names)`.
async fn given_api(emulator: &MultiServiceRouter, case: &str, function: &str) -> (String, String) {
    let (status, api) = control(
        emulator,
        "POST",
        "/v2/apis",
        Some(json!({ "name": "alarmed-api", "protocolType": "HTTP" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateApi answered {status} {api}"
    );
    let api_id = api
        .get("apiId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let host = api
        .get("apiEndpoint")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .split_once("://")
        .map(|(_scheme, rest)| rest.to_string())
        .unwrap_or_default();
    let (status, integration) = control(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/integrations"),
        Some(lambda_integration(function, "2.0")),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateIntegration answered {status} {integration}"
    );
    let integration_id = integration
        .get("integrationId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    for route_key in [OK_ROUTE, BOOM_ROUTE] {
        let (status, route) = control(
            emulator,
            "POST",
            &format!("/v2/apis/{api_id}/routes"),
            Some(json!({
                "routeKey": route_key,
                "target": format!("integrations/{integration_id}"),
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateRoute {route_key:?} answered {status} {route}"
        );
    }
    let (status, created) = control(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(json!({ "stageName": STAGE, "autoDeploy": true })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateStage answered {status} {created}"
    );
    (api_id, host)
}

/// The path within the api's endpoint that reaches `route_path` on the [`STAGE`] stage: "the
/// stage name is typically appended to this URI to form a complete path to a deployed API
/// stage" (`Api$ApiEndpoint`), so a named stage is reached behind its own segment.
fn invoke_path(route_path: &str) -> String {
    format!("/{STAGE}{route_path}")
}

/// The `Sum` over every datapoint of one stage-level series, or `None` when the read failed.
///
/// Summed over the datapoints rather than read off one, because a period boundary crossed
/// while a row is running splits its observations over two of them.
async fn total(emulator: &MultiServiceRouter, metric: &str, api_id: &str) -> Option<f64> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64());
    let (status, body) = cloudwatch(
        emulator,
        "GetMetricStatistics",
        &json!({
            "Namespace": NAMESPACE,
            "MetricName": metric,
            "Dimensions": [
                {"Name": API_ID_DIMENSION, "Value": api_id},
                {"Name": STAGE_DIMENSION, "Value": STAGE},
            ],
            "StartTime": now - WINDOW_SECONDS,
            "EndTime": now + AHEAD_SECONDS,
            "Period": PERIOD_SECONDS,
            "Statistics": ["Sum"],
        }),
    )
    .await;
    if status != 200 {
        return None;
    }
    Some(
        body.get("Datapoints")
            .and_then(Value::as_array)
            .map(|points| {
                points
                    .iter()
                    .map(|point| point.get("Sum").and_then(Value::as_f64).unwrap_or_default())
                    .sum()
            })
            .unwrap_or_default(),
    )
}

/// The `StateValue` of one metric alarm, as `DescribeAlarms` reports it.
async fn alarm_state(emulator: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = cloudwatch(
        emulator,
        "DescribeAlarms",
        &json!({"AlarmNames": [name], "AlarmTypes": ["MetricAlarm"]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeAlarms for {name} answered {status} {body}"
    );
    body["MetricAlarms"]
        .as_array()
        .and_then(|alarms| alarms.first())
        .and_then(|alarm| alarm["StateValue"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// The state `DescribeAlarms` reports for `name`, once it is `wanted` or once the deadline is
/// up — so a row that never reaches it fails on what it last read.
async fn awaited_state(emulator: &MultiServiceRouter, name: &str, wanted: &str) -> String {
    let deadline = Instant::now() + SETTLES_WITHIN;
    loop {
        let state = alarm_state(emulator, name).await;
        if state == wanted || Instant::now() >= deadline {
            return state;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// The states one alarm's `StateUpdate` history records it moved TO, oldest first.
async fn recorded_transitions(emulator: &MultiServiceRouter, name: &str) -> Vec<String> {
    let (status, body) = cloudwatch(
        emulator,
        "DescribeAlarmHistory",
        &json!({
            "AlarmName": name,
            "AlarmTypes": ["MetricAlarm"],
            "HistoryItemType": "StateUpdate",
            "ScanBy": "TimestampAscending",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeAlarmHistory for {name} answered {status} {body}"
    );
    body["AlarmHistoryItems"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|item| item["HistoryData"].as_str().map(str::to_string))
        .filter_map(|data| serde_json::from_str::<Value>(&data).ok())
        .filter_map(|data| {
            data.pointer("/newState/stateValue")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect()
}

/// One row: the series an alarm watches, the traffic the stage is given, and the state that
/// traffic leaves the alarm in.
struct Row {
    case: &'static str,
    /// The series the alarm is declared on.
    metric: &'static str,
    /// The route path the row's requests are made to, and the status the stage answers them
    /// with.
    path: &'static str,
    status: u16,
    /// How many of those requests the row makes.
    requests: usize,
    /// Whether the alarm's actions are enabled.
    actions_enabled: bool,
    /// The `Sum` the row's own series must hold once the traffic is served — what the alarm is
    /// then evaluated from.
    published: f64,
    /// The state the alarm must reach from it.
    state: &'static str,
}

/// The threshold every row's alarm carries, with `GreaterThanOrEqualToThreshold`: one
/// observation of the watched counter inside a period breaches it, so a row's verdict does not
/// depend on how its requests fall either side of a period boundary.
const THRESHOLD: f64 = 1.0;

const ROWS: &[Row] = &[
    Row {
        case: "the recorded case: a 5xx alarm whose actions are enabled, breached by an \
               integration that answers a server error",
        metric: SERVER_ERRORS,
        path: "/boom",
        status: 500,
        requests: 4,
        actions_enabled: true,
        published: 4.0,
        state: "ALARM",
    },
    Row {
        case: "its twin whose actions are disabled, which evaluates the same traffic",
        metric: SERVER_ERRORS,
        path: "/boom",
        status: 500,
        requests: 4,
        actions_enabled: false,
        published: 4.0,
        state: "ALARM",
    },
    Row {
        case: "an alarm on the client-error counter, breached by a path no route matches",
        metric: CLIENT_ERRORS,
        path: "/nothing-here",
        status: 404,
        requests: 3,
        actions_enabled: true,
        published: 3.0,
        state: "ALARM",
    },
    Row {
        case: "an alarm on the request counter, breached by the traffic itself",
        metric: COUNT,
        path: "/ok",
        status: 200,
        requests: 5,
        actions_enabled: true,
        published: 5.0,
        state: "ALARM",
    },
    Row {
        case: "an alarm on a counter the traffic never increments, which is graded rather \
               than left without data",
        metric: SERVER_ERRORS,
        path: "/ok",
        status: 200,
        requests: 2,
        actions_enabled: true,
        published: 0.0,
        state: "OK",
    },
];

#[tokio::test]
async fn the_traffic_a_stage_serves_decides_the_alarm_declared_over_its_series() {
    for row in ROWS {
        let case = row.case;
        // A fresh emulator per row, so the series its alarm grades hold only its own traffic.
        let (router, engine) = emulator();
        let function =
            given_function(&router, &engine, case, "alarmed-http-api", HANDLER_SOURCE).await;
        let (api_id, host) = given_api(&router, case, &function).await;

        let name = format!("{}-alarm", row.metric);
        let (status, body) = cloudwatch(
            &router,
            "PutMetricAlarm",
            &json!({
                "AlarmName": name,
                "Namespace": NAMESPACE,
                "MetricName": row.metric,
                "Dimensions": [
                    {"Name": API_ID_DIMENSION, "Value": api_id},
                    {"Name": STAGE_DIMENSION, "Value": STAGE},
                ],
                "Statistic": "Sum",
                "Period": 60,
                "EvaluationPeriods": 1,
                "DatapointsToAlarm": 1,
                "Threshold": THRESHOLD,
                "ComparisonOperator": "GreaterThanOrEqualToThreshold",
                "TreatMissingData": "notBreaching",
                "ActionsEnabled": row.actions_enabled,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: PutMetricAlarm answered {status} {body}"
        );
        // Nothing has been served yet, so nothing can have breached: the state the row
        // asserts below is one only the traffic can produce.
        let state = alarm_state(&router, &name).await;
        assert_ne!(
            state, "ALARM",
            "case {case:?}: expected an alarm over traffic nothing has served yet not to be in \
             ALARM, got {state:?}"
        );

        for request in 0..row.requests {
            let answered = send(&router, &host, "GET", &invoke_path(row.path), &[]).await;
            assert_eq!(
                answered.status, row.status,
                "case {case:?}: request {request} expected HTTP {} from the stage, got {}",
                row.status,
                answered.summary()
            );
        }
        let published = total(&router, row.metric, &api_id).await;
        assert_eq!(
            published,
            Some(row.published),
            "case {case:?}: expected the stage's {} sum to be {} after the traffic, got \
             {published:?}",
            row.metric,
            row.published
        );

        let reached = awaited_state(&router, &name, row.state).await;
        assert_eq!(
            reached, row.state,
            "case {case:?} (ActionsEnabled={}): expected the alarm to reach {} from the \
             traffic the stage served, got {reached:?}",
            row.actions_enabled, row.state
        );
        let transitions = recorded_transitions(&router, &name).await;
        assert!(
            transitions.iter().any(|state| state == row.state),
            "case {case:?}: expected the move to {} to be recorded in the alarm's history, got \
             {transitions:?}",
            row.state
        );
    }
}
