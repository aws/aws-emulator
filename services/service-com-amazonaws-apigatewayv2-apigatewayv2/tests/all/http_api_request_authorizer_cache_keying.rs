// SPDX-License-Identifier: CC0-1.0

//! What a route gated by a `REQUEST` (Lambda) authorizer SERVES, and how often the
//! authorizer's own function is run while it serves — driven end to end over the
//! `execute-api` data plane, with the authorizer's function and the route's backend run by
//! AWS Lambda on the same engine and the run counts read back through CloudWatch.
//!
//! The rule asserted, one level above the recorded case (one header identity source, a
//! result TTL of 300 seconds, two requests of one value and one of another):
//!
//! > **A route of a deployed HTTP api stage that is gated by a `REQUEST` authorizer is
//! > answered by the route's own integration for every request the authorizer allows, and
//! > the authorizer's function is invoked once per DISTINCT LIST of identity-source values
//! > while the authorizer's result TTL lasts.** "For HTTP APIs, identity sources are also
//! > used as the cache key when caching is enabled" (`Authorizer$IdentitySource`), and "The
//! > time to live (TTL) for cached authorizer results, in seconds. If it equals 0,
//! > authorization caching is disabled. If it is greater than 0, API Gateway caches
//! > authorizer responses" (`Authorizer$AuthorizerResultTtlInSeconds`, whose range is
//! > `0..3600`).
//!
//! So two counts follow from one sequence of requests: the authorizer's function ran once
//! per distinct identity-source value list (and once per REQUEST when the TTL disables
//! caching), while the backend ran once per request — every allowed request reaches the
//! integration, whether its decision was freshly invoked or re-used from the cache.
//!
//! Both counts are read where a client reads them: the `Invocations` series AWS Lambda
//! publishes per function, through CloudWatch's own `GetMetricStatistics`. Nothing here is a
//! value this emulator fills in — every assertion is on a function that RAN, which is why
//! this service, AWS Lambda and CloudWatch are mounted on one engine as
//! `core/router/src/lib.rs` mounts them.
//!
//! The rows differ along what the rule generalises over: how many identity sources the
//! authorizer names (one, two), the request part each names (a header, a query-string
//! parameter), where the boundary between two presented values falls (a pair that would
//! collide with another pair if the values were joined with nothing between them), how many
//! requests are made and in which order their values repeat, and the TTL the authorizer
//! declares (none, the value that disables caching, a middling one, and the upper bound of
//! the member's range).
//!
//! The steps BEFORE the cache are not restated here: `http_api_request_authorizer.rs` owns
//! the identity-source precondition, and `http_api_backend_dispatch.rs` owns the reading of
//! the function's decision, the TTL window and `ResetAuthorizersCache`.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call, REGION};
use crate::http_api_backend_dispatch::{given_function, lambda_integration, ECHO_EVENT};
use crate::websocket_handshake_data_plane::{send, send_with_body, Answer};

/// The route every row gates, and the path a client invokes it at. The stage is the one
/// that serves the api's root, so the path carries no stage segment.
const ROUTE_KEY: &str = "GET /guarded";
/// See [`ROUTE_KEY`].
const ROUTE_PATH: &str = "/guarded";
/// See [`ROUTE_KEY`].
const STAGE_NAME: &str = "$default";

/// The authorizer's own function, and the function the route's integration names.
const GATE_FUNCTION: &str = "authorizer-function";
/// See [`GATE_FUNCTION`].
const BACKEND_FUNCTION: &str = "guarded-backend";

/// An authorizer that authorizes every call it is asked about, in the simple format a `2.0`
/// authorizer that enabled simple responses answers in.
const ALLOW_ALL_GATE: &str = "def lambda_handler(event, context):\n    \
     return {'isAuthorized': True}\n";

/// The payload format the authorizer is configured for, and the one its answer is read
/// under.
const PAYLOAD_FORMAT: &str = "2.0";

/// The namespace AWS Lambda publishes its own series in, the series that counts the times a
/// function was invoked, and the dimension it is reported per (the AWS Lambda metric
/// reference).
const LAMBDA_NAMESPACE: &str = "AWS/Lambda";
/// See [`LAMBDA_NAMESPACE`].
const INVOCATIONS_SERIES: &str = "Invocations";
/// See [`LAMBDA_NAMESPACE`].
const FUNCTION_DIMENSION: &str = "FunctionName";

/// The window every metric read covers, in seconds, and the period it is read at: generous
/// on both sides, so no case depends on where in a minute the suite runs or on how fast the
/// host is.
const WINDOW_SECONDS: f64 = 900.0;
/// See [`WINDOW_SECONDS`].
const AHEAD_SECONDS: f64 = 120.0;
/// See [`WINDOW_SECONDS`].
const PERIOD_SECONDS: i64 = 60;

/// This service, AWS Lambda and CloudWatch on ONE engine.
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

/// One request of a row: the identity-source values it presents, as the request parts the
/// authorizer's sources name.
#[derive(Debug, Clone, Copy)]
struct Presentation {
    headers: &'static [(&'static str, &'static str)],
    query: &'static [(&'static str, &'static str)],
}

/// One row: an authorizer configuration, the sequence of requests made against the route it
/// gates, and how many times the authorizer's function must have run for them.
struct Row {
    case: &'static str,
    /// `Authorizer$IdentitySource`, in the `$`-prefixed spelling an HTTP api uses.
    identity_source: &'static [&'static str],
    /// `Authorizer$AuthorizerResultTtlInSeconds`, or `None` for an authorizer that declares
    /// none.
    ttl_seconds: Option<i32>,
    /// The requests made, in order.
    requests: &'static [Presentation],
    /// How many invocations of the authorizer's function those requests owe: one per
    /// distinct identity-source value list while the TTL caches, one per request when it
    /// does not.
    expected_authorizer_runs: f64,
}

/// Two presentations of a single header identity source, one per caller.
const CALLER_ONE: Presentation = Presentation {
    headers: &[("authorization", "caller-one")],
    query: &[],
};
/// See [`CALLER_ONE`].
const CALLER_TWO: Presentation = Presentation {
    headers: &[("authorization", "caller-two")],
    query: &[],
};

/// The rows, each a configuration and a sequence of requests over it.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one header source: one value twice, then another value",
            identity_source: &["$request.header.authorization"],
            ttl_seconds: Some(300),
            requests: &[CALLER_ONE, CALLER_ONE, CALLER_TWO],
            expected_authorizer_runs: 2.0,
        },
        Row {
            case: "one query-string source presenting one value twice",
            identity_source: &["$request.querystring.access_token"],
            ttl_seconds: Some(300),
            requests: &[
                Presentation {
                    headers: &[],
                    query: &[("access_token", "query-one")],
                },
                Presentation {
                    headers: &[],
                    query: &[("access_token", "query-one")],
                },
            ],
            expected_authorizer_runs: 1.0,
        },
        Row {
            case: "two sources, a header and a query-string parameter, the second changing",
            identity_source: &[
                "$request.header.authorization",
                "$request.querystring.access_token",
            ],
            ttl_seconds: Some(300),
            requests: &[
                Presentation {
                    headers: &[("authorization", "caller-one")],
                    query: &[("access_token", "query-one")],
                },
                Presentation {
                    headers: &[("authorization", "caller-one")],
                    query: &[("access_token", "query-one")],
                },
                Presentation {
                    headers: &[("authorization", "caller-one")],
                    query: &[("access_token", "query-two")],
                },
            ],
            expected_authorizer_runs: 2.0,
        },
        Row {
            // Two value lists that run together into the same text: a key that joined the
            // values with nothing between them would decide the second request from the
            // first request's decision.
            case: "two sources whose value lists differ only in where their boundary falls",
            identity_source: &[
                "$request.header.authorization",
                "$request.querystring.access_token",
            ],
            ttl_seconds: Some(3600),
            requests: &[
                Presentation {
                    headers: &[("authorization", "ab")],
                    query: &[("access_token", "c")],
                },
                Presentation {
                    headers: &[("authorization", "a")],
                    query: &[("access_token", "bc")],
                },
            ],
            expected_authorizer_runs: 2.0,
        },
        Row {
            case: "a TTL of zero, which disables caching, presenting one value twice",
            identity_source: &["$request.header.authorization"],
            ttl_seconds: Some(0),
            requests: &[CALLER_ONE, CALLER_ONE],
            expected_authorizer_runs: 2.0,
        },
        Row {
            case: "the upper bound of the TTL range, presenting one value three times",
            identity_source: &["$request.header.authorization"],
            ttl_seconds: Some(3600),
            requests: &[CALLER_ONE, CALLER_ONE, CALLER_ONE],
            expected_authorizer_runs: 1.0,
        },
        Row {
            case: "an authorizer declaring no TTL, which caches for the documented default",
            identity_source: &["$request.header.authorization"],
            ttl_seconds: None,
            requests: &[CALLER_ONE, CALLER_TWO, CALLER_ONE],
            expected_authorizer_runs: 2.0,
        },
    ]
}

/// The whole api one row drives: the backend function, the authorizer's function, the
/// integration, the `REQUEST` authorizer, the gated route and the stage that serves the
/// api's root. Answers the host the api's own `ApiEndpoint` names.
async fn given_gated_api(
    emulator: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    row: &Row,
) -> String {
    let case = row.case;
    let backend = given_function(emulator, engine, case, BACKEND_FUNCTION, ECHO_EVENT).await;
    let gate = given_function(emulator, engine, case, GATE_FUNCTION, ALLOW_ALL_GATE).await;

    let (status, api) = call(
        emulator,
        "POST",
        "/v2/apis",
        Some(json!({ "name": "request-authorized-api", "protocolType": "HTTP" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateApi answered {status} {api}"
    );
    let api_id = as_str(&api, "apiId");
    let endpoint = as_str(&api, "apiEndpoint");
    let host = endpoint
        .split_once("://")
        .map_or(endpoint.as_str(), |(_scheme, host)| host)
        .to_string();

    let (status, integration) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/integrations"),
        Some(lambda_integration(&backend, PAYLOAD_FORMAT)),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateIntegration answered {status} {integration}"
    );
    let integration_id = as_str(&integration, "integrationId");

    // "For REQUEST authorizers, this must be a well-formed Lambda function URI, for example,
    // arn:aws:apigateway:{region}:lambda:path/2015-03-31/functions/{function-arn}/invocations"
    // (`Authorizer$AuthorizerUri`).
    let mut authorizer_request = json!({
        "name": "identity-keyed-gate",
        "authorizerType": "REQUEST",
        "authorizerUri": format!(
            "arn:aws:apigateway:{REGION}:lambda:path/2015-03-31/functions/{gate}/invocations"
        ),
        "authorizerPayloadFormatVersion": PAYLOAD_FORMAT,
        "enableSimpleResponses": true,
        "identitySource": row.identity_source,
    });
    if let Some(ttl) = row.ttl_seconds {
        authorizer_request["authorizerResultTtlInSeconds"] = json!(ttl);
    }
    let (status, authorizer) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/authorizers"),
        Some(authorizer_request),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateAuthorizer answered {status} {authorizer}"
    );
    let authorizer_id = as_str(&authorizer, "authorizerId");

    // `CUSTOM` is the `AuthorizationType` "for using a Lambda authorizer"
    // (`Route$AuthorizationType`), associated with the authorizer by `AuthorizerId`.
    let (status, route) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/routes"),
        Some(json!({
            "routeKey": ROUTE_KEY,
            "target": format!("integrations/{integration_id}"),
            "authorizationType": "CUSTOM",
            "authorizerId": authorizer_id
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateRoute answered {status} {route}"
    );

    let (status, stage) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(json!({ "stageName": STAGE_NAME, "autoDeploy": true })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateStage answered {status} {stage}"
    );
    host
}

/// The query string one presentation is invoked with, or nothing at all for a presentation
/// that names no query-string parameter.
fn query_string(parameters: &[(&str, &str)]) -> String {
    if parameters.is_empty() {
        return String::new();
    }
    let rendered: Vec<String> = parameters
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    format!("?{}", rendered.join("&"))
}

/// One CloudWatch call over its `awsJson1_0` wire.
async fn monitoring(emulator: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let answer: Answer = send_with_body(
        emulator,
        &format!("monitoring.{REGION}.amazonaws.com"),
        "POST",
        "/",
        &[
            ("content-type", "application/x-amz-json-1.0"),
            (
                "authorization",
                &format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTAPIGWV2/20261021/{REGION}/monitoring/\
                     aws4_request, SignedHeaders=host, Signature=0000"
                ),
            ),
            (
                "x-amz-target",
                &format!("GraniteServiceVersion20100801.{operation}"),
            ),
        ],
        body.to_string().into_bytes(),
    )
    .await;
    (answer.status, answer.body)
}

/// How many times `function` was invoked, as a client learns it: the `Sum` of the
/// `Invocations` datapoints AWS Lambda published for that function in the window.
///
/// Summed over the datapoints rather than read off one, because a period boundary crossed
/// while a case runs splits its observations over two of them.
async fn invocations(emulator: &MultiServiceRouter, function: &str) -> f64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs_f64())
        .unwrap_or_default();
    let (status, body) = monitoring(
        emulator,
        "GetMetricStatistics",
        json!({
            "Namespace": LAMBDA_NAMESPACE,
            "MetricName": INVOCATIONS_SERIES,
            "Dimensions": [{ "Name": FUNCTION_DIMENSION, "Value": function }],
            "StartTime": now - WINDOW_SECONDS,
            "EndTime": now + AHEAD_SECONDS,
            "Period": PERIOD_SECONDS,
            "Statistics": ["Sum"],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "GetMetricStatistics for {function:?} answered {status} {body}"
    );
    body.get("Datapoints")
        .and_then(Value::as_array)
        .map(|points| {
            points
                .iter()
                .map(|point| point.get("Sum").and_then(Value::as_f64).unwrap_or_default())
                .sum()
        })
        .unwrap_or_default()
}

/// RULE: every request a `REQUEST` authorizer allows is answered by the gated route's own
/// integration, and the authorizer's function is invoked once per distinct list of
/// identity-source values while its result TTL caches the decision.
#[tokio::test]
async fn every_allowed_request_is_served_and_the_authorizer_runs_once_per_identity() {
    for row in rows() {
        let case = row.case;
        let (emulator, engine) = emulator();
        let host = given_gated_api(&emulator, &engine, &row).await;

        for (index, presentation) in row.requests.iter().enumerate() {
            let path = format!("{ROUTE_PATH}{}", query_string(presentation.query));
            let answer = send(&emulator, &host, "GET", &path, presentation.headers).await;
            assert_eq!(
                answer.status,
                200,
                "case {case:?} / request {index}: expected the gated route to be served by \
                 its backend, got {}",
                answer.summary()
            );
            // The backend echoes the event it was given, so the body says the INTEGRATION
            // answered this request rather than the gateway answering for it.
            let event: Value = serde_json::from_str(&answer.text).unwrap_or(Value::Null);
            assert_eq!(
                event.get("routeKey").and_then(Value::as_str),
                Some(ROUTE_KEY),
                "case {case:?} / request {index}: expected the backend's own event for \
                 {ROUTE_KEY:?}, got {}",
                answer.text
            );
        }

        let authorizer_runs = invocations(&emulator, GATE_FUNCTION).await;
        assert_eq!(
            authorizer_runs, row.expected_authorizer_runs,
            "case {case:?}: expected the authorizer's function to be invoked {} time(s) for \
             {} request(s), got {authorizer_runs}",
            row.expected_authorizer_runs,
            row.requests.len()
        );
        // Every allowed request reaches the integration, whether its decision was freshly
        // invoked or re-used: the cache holds the authorizer's answer and not the route's.
        let backend_runs = invocations(&emulator, BACKEND_FUNCTION).await;
        let expected_backend_runs = row.requests.len() as f64;
        assert_eq!(
            backend_runs, expected_backend_runs,
            "case {case:?}: expected the backend to be invoked once per request \
             ({expected_backend_runs}), got {backend_runs}"
        );
    }
}
