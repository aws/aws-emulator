// SPDX-License-Identifier: CC0-1.0

//! The CloudWatch series a deployed HTTP api stage publishes about the requests it ANSWERED.
//!
//! The rule under test, one level above the recorded case:
//!
//! > Every request a deployed HTTP api stage answers is counted in API Gateway's own
//! > `AWS/ApiGateway` namespace: one observation of `Count`, and one of each error counter — `4xx` for the
//! > client-error class and `5xx` for the server-error class — which is 1 for a request whose
//! > status is in that class and 0 for one that is not. A request the gateway itself refused
//! > is a request the stage answered, so a throttled route's `429` and a `404` for a path no
//! > route matched are counted exactly like a backend's `200`; the status alone decides the
//! > counter. The series are dimensioned by `ApiId` and by `ApiId` with `Stage`, and a route
//! > whose serving stage sets `DetailedMetricsEnabled` gets the same observations again under
//! > its own `Method` and `Resource` dimensions — a stage that asks for nothing publishes the
//! > stage-level series and no route's.
//!
//! Only a cross-service suite can pin this: the api, its stage and its route settings are this
//! service's, the answer a route is served with belongs to a function AWS Lambda holds and
//! runs, and the metric store the counters land in is CloudWatch's — all three on ONE engine,
//! as `core/router/src/lib.rs` mounts them. Every assertion reads the outcome back through
//! CloudWatch's own readers (`ListMetrics` and `GetMetricStatistics`), so an observation that
//! did not reach that store cannot pass.
//!
//! The rows differ along what the rule generalises over: the STATUS CLASS the stage answered
//! with (a backend's success, a path no route matches, a throttled route, a backend that
//! produced nothing), how MANY requests were made, which STAGE served them (a named one and
//! the one that serves the api's root), the ROUTE KEY's shape, and WHERE the detailed-metrics
//! opt-in is stated (on the route's own settings, on the stage's defaults beside a route that
//! states only a throttle, nowhere).

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::http_api_backend_dispatch::{given_api, given_function, lambda_integration, ECHO_EVENT};
use crate::websocket_handshake_data_plane::{send, Answer};

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// The namespace API Gateway publishes what its HTTP apis answered in.
const NAMESPACE: &str = "AWS/ApiGateway";

/// The counters and the measurements one answered request is observed in.
const COUNT: &str = "Count";
const CLIENT_ERRORS: &str = "4xx";
const SERVER_ERRORS: &str = "5xx";
const LATENCY: &str = "Latency";
const DATA_PROCESSED: &str = "DataProcessed";

/// The dimensions those series are identified by.
const API_ID: &str = "ApiId";
const STAGE: &str = "Stage";
const METHOD: &str = "Method";
const RESOURCE: &str = "Resource";

/// A handler that RAISES, so the route's integration produces no response and the gateway
/// answers the server-error class for it.
const RAISES: &str = "def lambda_handler(event, context):\n    raise RuntimeError('no answer')\n";

/// The window every read below covers, in seconds, and the period it is read at: generous
/// enough on both sides that no case depends on where in a minute the suite runs.
const WINDOW_SECONDS: f64 = 900.0;
const AHEAD_SECONDS: f64 = 120.0;
const PERIOD_SECONDS: i64 = 60;

/// This service, AWS Lambda and CloudWatch on ONE engine, as `core/router/src/lib.rs` mounts
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

/// One CloudWatch call over its `awsJson1_0` wire, answering its status and parsed body.
async fn monitoring(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let answer: Answer = crate::websocket_handshake_data_plane::send_with_body(
        router,
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

/// Now, as the epoch seconds a CloudWatch read states its window in.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs_f64())
        .unwrap_or_default()
}

/// The `Sum` of every datapoint the series `metric`/`dimensions` holds in the window.
///
/// Summed over the datapoints rather than read off one, because a window boundary crossed
/// while a case is running splits its observations over two periods — and the whole of what a
/// counter says is its total.
async fn total(
    router: &MultiServiceRouter,
    metric: &str,
    dimensions: &[(&str, String)],
) -> Option<f64> {
    let now = now_secs();
    let (status, body) = monitoring(
        router,
        "GetMetricStatistics",
        json!({
            "Namespace": NAMESPACE,
            "MetricName": metric,
            "Dimensions": dimensions
                .iter()
                .map(|(name, value)| json!({ "Name": name, "Value": value }))
                .collect::<Vec<Value>>(),
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

/// Every series this namespace holds for `api_id`, as `(metric name, its dimension pairs)`
/// with the pairs ordered by dimension name.
async fn series(router: &MultiServiceRouter, api_id: &str) -> Vec<(String, Vec<(String, String)>)> {
    let (status, body) = monitoring(router, "ListMetrics", json!({ "Namespace": NAMESPACE })).await;
    assert_eq!(status, 200, "ListMetrics answered {status} {body}");
    let mut held: Vec<(String, Vec<(String, String)>)> = body
        .get("Metrics")
        .and_then(Value::as_array)
        .map(|metrics| {
            metrics
                .iter()
                .map(|metric| {
                    let mut dimensions: Vec<(String, String)> = metric
                        .get("Dimensions")
                        .and_then(Value::as_array)
                        .map(|pairs| {
                            pairs
                                .iter()
                                .map(|pair| {
                                    (
                                        pair.get("Name")
                                            .and_then(Value::as_str)
                                            .unwrap_or_default()
                                            .to_string(),
                                        pair.get("Value")
                                            .and_then(Value::as_str)
                                            .unwrap_or_default()
                                            .to_string(),
                                    )
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    dimensions.sort();
                    (
                        metric
                            .get("MetricName")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        dimensions,
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    held.retain(|(_, dimensions)| {
        dimensions
            .iter()
            .any(|(name, value)| name == API_ID && value == api_id)
    });
    held.sort();
    held
}

/// One row: an api whose stage answers a run of requests, and what the series it publishes
/// then hold.
struct Row {
    case: &'static str,
    /// The stage that serves the traffic.
    stage_name: &'static str,
    /// The route the api holds, and the verb and path within the stage each request uses.
    route_key: &'static str,
    verb: &'static str,
    route_path: &'static str,
    /// The handler behind the route: one that answers, or one that raises.
    handler: &'static str,
    /// The throttle the stage states for the route, as `(rate, burst)`, or none at all.
    throttle: Option<(f64, i32)>,
    /// Where the detailed-metrics opt-in is stated: on the route's own settings, on the
    /// stage's defaults, or nowhere.
    detailed: Detailed,
    /// The status each request in order is answered with.
    statuses: &'static [u16],
    /// The `Method`/`Resource` dimensions of the route series the row expects, or `None` for a
    /// row that expects none at all.
    route_series: Option<(&'static str, &'static str)>,
}

/// Where a stage states `DetailedMetricsEnabled`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Detailed {
    /// Nowhere: the stage asks for no route's series.
    Off,
    /// On the settings of the route itself.
    OnTheRoute,
    /// On the stage's `DefaultRouteSettings`, which govern every route it states nothing for.
    OnTheStageDefaults,
}

/// The rows: every status class, both stage forms, both places the opt-in can be stated.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one request a function served, on a named stage",
            stage_name: "live",
            route_key: "GET /served",
            verb: "GET",
            route_path: "/served",
            handler: ECHO_EVENT,
            throttle: None,
            detailed: Detailed::Off,
            statuses: &[200],
            route_series: None,
        },
        Row {
            case: "three requests a function served, on the stage that serves the api's root",
            stage_name: "$default",
            route_key: "GET /served",
            verb: "GET",
            route_path: "/served",
            handler: ECHO_EVENT,
            throttle: None,
            detailed: Detailed::Off,
            statuses: &[200, 200, 200],
            route_series: None,
        },
        Row {
            case: "a path no route of the api matches, which the gateway answers itself",
            stage_name: "live",
            route_key: "GET /served",
            verb: "GET",
            route_path: "/nothing-here",
            handler: ECHO_EVENT,
            throttle: None,
            detailed: Detailed::Off,
            statuses: &[404, 404],
            route_series: None,
        },
        Row {
            case: "a route the stage throttles to nothing, whose refusals are the 4xx class",
            stage_name: "live",
            route_key: "GET /metered",
            verb: "GET",
            route_path: "/metered",
            handler: ECHO_EVENT,
            throttle: Some((0.0, 1)),
            detailed: Detailed::Off,
            statuses: &[200, 429, 429, 429],
            route_series: None,
        },
        Row {
            case: "a route whose function produced no response, which is the 5xx class",
            stage_name: "live",
            route_key: "GET /broken",
            verb: "GET",
            route_path: "/broken",
            handler: RAISES,
            throttle: None,
            detailed: Detailed::Off,
            statuses: &[500],
            route_series: None,
        },
        Row {
            case: "a route whose own settings ask for detailed metrics",
            stage_name: "live",
            route_key: "GET /served",
            verb: "GET",
            route_path: "/served",
            handler: ECHO_EVENT,
            throttle: None,
            detailed: Detailed::OnTheRoute,
            statuses: &[200, 200],
            route_series: Some(("GET", "/served")),
        },
        Row {
            case: "a stage whose default route settings ask for them, on a route key carrying \
                   a path variable",
            stage_name: "live",
            route_key: "POST /orders/{id}",
            verb: "POST",
            route_path: "/orders/7",
            handler: ECHO_EVENT,
            throttle: None,
            detailed: Detailed::OnTheStageDefaults,
            statuses: &[200],
            route_series: Some(("POST", "/orders/{id}")),
        },
        Row {
            case: "a detailed-metrics stage whose route key answers every verb",
            stage_name: "live",
            route_key: "ANY /{proxy+}",
            verb: "DELETE",
            route_path: "/anything/at/all",
            handler: ECHO_EVENT,
            throttle: None,
            detailed: Detailed::OnTheRoute,
            statuses: &[200],
            route_series: Some(("ANY", "/{proxy+}")),
        },
        Row {
            case: "a route whose own settings state only a throttle, under a stage whose \
                   defaults ask for detailed metrics",
            stage_name: "live",
            route_key: "GET /metered",
            verb: "GET",
            route_path: "/metered",
            handler: ECHO_EVENT,
            throttle: Some((0.0, 1)),
            detailed: Detailed::OnTheStageDefaults,
            statuses: &[200, 429],
            route_series: Some(("GET", "/metered")),
        },
    ]
}

/// The stage this row creates: its name, auto-deploy, and the route settings its throttle and
/// its detailed-metrics opt-in are stated in.
fn stage_of(row: &Row) -> Value {
    let mut settings = json!({});
    if let Some((rate, burst)) = row.throttle {
        settings["throttlingRateLimit"] = json!(rate);
        settings["throttlingBurstLimit"] = json!(burst);
    }
    if row.detailed == Detailed::OnTheRoute {
        settings["detailedMetricsEnabled"] = json!(true);
    }
    let mut stage = json!({ "stageName": row.stage_name, "autoDeploy": true });
    if settings.as_object().is_some_and(|held| !held.is_empty()) {
        stage["routeSettings"] = json!({ row.route_key: settings });
    }
    if row.detailed == Detailed::OnTheStageDefaults {
        stage["defaultRouteSettings"] = json!({ "detailedMetricsEnabled": true });
    }
    stage
}

/// The path within the api's endpoint a request of this row is made at: the route path itself
/// on the stage that serves the api's root, and the stage name in front of it otherwise.
fn invoke_path(row: &Row) -> String {
    if row.stage_name == "$default" {
        return row.route_path.to_string();
    }
    format!("/{}{}", row.stage_name, row.route_path)
}

/// Every request a stage answers is counted, its status decides which counter, and the route
/// series exist exactly for a route the stage asked for them for.
#[tokio::test]
async fn every_answered_request_is_counted_in_the_series_its_status_belongs_to() {
    for row in rows() {
        let case = row.case;
        // One emulator per row, so each row's series hold exactly its own traffic.
        let (router, engine) = emulator();
        let function = given_function(&router, &engine, case, "metric-source", row.handler).await;
        let (api_id, host) = given_api(
            &router,
            case,
            lambda_integration(&function, "2.0"),
            row.route_key,
            stage_of(&row),
        )
        .await;

        let path = invoke_path(&row);
        for (attempt, expected) in row.statuses.iter().enumerate() {
            let answered = send(&router, &host, row.verb, &path, &[]).await;
            assert_eq!(
                answered.status, *expected,
                "case {case:?}: expected request {attempt} at {path:?} to be answered \
                 {expected}, got {}",
                answered.summary()
            );
        }

        let answered = row.statuses.len() as f64;
        let client_errors = row
            .statuses
            .iter()
            .filter(|status| (400..500).contains(*status))
            .count() as f64;
        let server_errors = row
            .statuses
            .iter()
            .filter(|status| (500..600).contains(*status))
            .count() as f64;

        // The stage-level series: the api alone, and the api with the stage that served.
        let api_dimensions = vec![(API_ID, api_id.clone())];
        let stage_dimensions = vec![
            (API_ID, api_id.clone()),
            (STAGE, row.stage_name.to_string()),
        ];
        let mut expected_dimension_sets: Vec<Vec<(String, String)>> = vec![
            pairs(&api_dimensions),
            pairs(&stage_dimensions),
        ];
        let mut scopes = vec![
            ("the api's own series", api_dimensions),
            ("the stage's series", stage_dimensions.clone()),
        ];
        if let Some((method, resource)) = row.route_series {
            let route_dimensions = vec![
                (API_ID, api_id.clone()),
                (STAGE, row.stage_name.to_string()),
                (METHOD, method.to_string()),
                (RESOURCE, resource.to_string()),
            ];
            expected_dimension_sets.push(pairs(&route_dimensions));
            scopes.push(("the route's own series", route_dimensions));
        }

        for (scope, dimensions) in &scopes {
            for (metric, expected) in [
                (COUNT, answered),
                (CLIENT_ERRORS, client_errors),
                (SERVER_ERRORS, server_errors),
            ] {
                let actual = total(&router, metric, dimensions).await;
                assert_eq!(
                    actual,
                    Some(expected),
                    "case {case:?}: expected {scope} to hold {expected} in {metric}, got \
                     {actual:?}"
                );
            }
        }

        // WHICH series exist: the stage-level ones always, the route's only where the stage
        // asked for it, and every metric of a request under each.
        let held = series(&router, &api_id).await;
        let mut dimension_sets: Vec<Vec<(String, String)>> =
            held.iter().map(|(_, pairs)| pairs.clone()).collect();
        dimension_sets.sort();
        dimension_sets.dedup();
        expected_dimension_sets.sort();
        assert_eq!(
            dimension_sets, expected_dimension_sets,
            "case {case:?}: expected exactly these dimension sets for the api, got {held:?}"
        );
        let mut metrics: Vec<String> = held.iter().map(|(metric, _)| metric.clone()).collect();
        metrics.sort();
        metrics.dedup();
        let mut expected_metrics = vec![
            COUNT.to_string(),
            CLIENT_ERRORS.to_string(),
            SERVER_ERRORS.to_string(),
            LATENCY.to_string(),
            DATA_PROCESSED.to_string(),
        ];
        expected_metrics.sort();
        assert_eq!(
            metrics, expected_metrics,
            "case {case:?}: expected every metric of an answered request, got {held:?}"
        );
    }
}

/// The dimension pairs of a scope, ordered by name the way a series reports them.
fn pairs(dimensions: &[(&str, String)]) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = dimensions
        .iter()
        .map(|(name, value)| ((*name).to_string(), value.clone()))
        .collect();
    pairs.sort();
    pairs
}
