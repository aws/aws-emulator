// SPDX-License-Identifier: CC0-1.0

//! What a deployed HTTP api stage LOGS about the requests it serves: the access-log record
//! its own `AccessLogSettings` ask for.
//!
//! The rule under test, one level above the recorded case:
//!
//! > A stage carrying `AccessLogSettings` — a `DestinationArn` naming a log group and a
//! > `$context` format — writes ONE record per request it served into THAT group, rendered
//! > by THAT format: one request is one record, a request the api refused is a request the
//! > stage answered and so is logged too, and the record's `$context.requestId` is the id
//! > the response reported, which is what lets a client join the answer it holds to the
//! > record of it. A variable the plane has no value for renders as the placeholder a field
//! > with no value carries, so the record is a complete line whatever the format names. A
//! > stage carrying no `AccessLogSettings` delivers nothing.
//!
//! Only a cross-service suite can pin this: the api, the stage and its logging configuration
//! are this service's, the route's answer belongs to a function AWS Lambda holds and runs,
//! and the group the records land in is CloudWatch Logs' — all three on ONE engine, as
//! `core/router/src/lib.rs` mounts them. Every assertion reads the outcome back through
//! CloudWatch Logs' own reader (`FilterLogEvents`), so a record that did not reach that store
//! cannot pass.
//!
//! The rows differ along what the rule generalises over: the FORMAT (a JSON one and a
//! Common-Log-Format one), the OUTCOME (a route the function served and a path no route
//! matches), the VERB and the path, and the REPETITION of one request.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::http_api_backend_dispatch::{given_api, given_function, lambda_integration};
use crate::websocket_handshake_data_plane::send;

/// The Region every request below is signed for and every ARN names.
const REGION: &str = "us-east-1";

/// The function the route is integrated with, and the route it is reached through.
const FUNCTION: &str = "access-logged-http-api";
const ROUTE_KEY: &str = "GET /echo";

/// The destination each case points its stage at.
const GROUP: &str = "/apigwv2/access";

/// The two formats a case renders its records by: a JSON one and a Common Log Format one.
const JSON_FORMAT: &str = concat!(
    r#"{"requestId":"$context.requestId","routeKey":"$context.routeKey","#,
    r#""httpMethod":"$context.httpMethod","status":"$context.status","#,
    r#""stage":"$context.stage","authorizer":"$context.authorizer.claims.sub"}"#
);
const CLF_FORMAT: &str = concat!(
    r#"$context.identity.sourceIp - - [$context.requestTime] ""#,
    r#"$context.httpMethod $context.path" $context.status $context.responseLength "#,
    r#"$context.requestId"#
);

/// The handler behind the route: it answers the structured response the `2.0` format
/// defines.
const HANDLER_SOURCE: &str = "import json\n\
                              \n\
                              \n\
                              def lambda_handler(event, context):\n\
                              \x20   return {\n\
                              \x20       'statusCode': 200,\n\
                              \x20       'headers': {'content-type': 'application/json'},\n\
                              \x20       'body': json.dumps({'seen': event.get('rawPath')}),\n\
                              \x20   }\n";

/// The header API Gateway reports the id of the request it processed in, which an
/// access-log record's `$context.requestId` has to equal.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// The three services this rule spans, over ONE engine, as `core/router/src/lib.rs` mounts
/// them.
///
/// `pub(crate)` because the WEBSOCKET plane's own access-log suite
/// ([`crate::websocket_access_logging`]) spans the same three services and reads its records
/// back through the same readers below, rather than keeping a second copy of this harness.
pub(crate) fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
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
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    (router, engine)
}

/// One CloudWatch Logs call over its `awsJson1_1` wire, answering its status and parsed body.
///
/// `pub(crate)` for the reason [`emulator`] is.
pub(crate) async fn logs(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    let answer = crate::websocket_handshake_data_plane::send_with_body(
        router,
        &format!("logs.{REGION}.amazonaws.com"),
        "POST",
        "/",
        &[
            ("content-type", "application/x-amz-json-1.1"),
            (
                "authorization",
                &format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTAPIGWV2/20261021/{REGION}/logs/\
                     aws4_request, SignedHeaders=host, Signature=0000"
                ),
            ),
            ("x-amz-target", &format!("Logs_20140328.{operation}")),
        ],
        body.to_string().into_bytes(),
    )
    .await;
    (answer.status, answer.body)
}

/// The messages `FilterLogEvents` reports for `group`, or `None` when the account holds no
/// such group at all — which is what "nothing was delivered and nothing was created" looks
/// like to a client.
///
/// EVERY page is read, so a reader never misses a line because the group held more events
/// than one answer carries. `None` is only CloudWatch Logs saying it holds no such group
/// ([`names_no_group`]); any other refusal is a failed read and panics rather than being
/// read as "nothing was delivered".
///
/// `pub(crate)` for the reason [`emulator`] is.
pub(crate) async fn delivered_to(
    router: &MultiServiceRouter,
    group: &str,
) -> Option<Vec<String>> {
    let mut messages: Vec<String> = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut request = json!({ "logGroupName": group });
        if let Some(token) = next.as_deref() {
            request["nextToken"] = json!(token);
        }
        let (status, body) = logs(router, "FilterLogEvents", request).await;
        if status != 200 {
            assert!(
                names_no_group(&body),
                "FilterLogEvents on {group:?} must answer 200 or report no such group, got \
                 {status} {body}"
            );
            return None;
        }
        messages.extend(
            body.get("events")
                .and_then(Value::as_array)
                .map(|events| {
                    events
                        .iter()
                        .map(|event| {
                            event
                                .get("message")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string()
                        })
                        .collect::<Vec<String>>()
                })
                .unwrap_or_default(),
        );
        next = body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if next.is_none() {
            return Some(messages);
        }
    }
}

/// Whether a refusal of that read is CloudWatch Logs saying it holds no such log group:
/// `ResourceNotFoundException`, the shape `FilterLogEvents` declares for it, however the
/// envelope qualified the shape it named.
fn names_no_group(body: &Value) -> bool {
    const NO_SUCH_GROUP: &str = "ResourceNotFoundException";
    body.get("__type")
        .and_then(Value::as_str)
        .and_then(|named| named.rsplit('#').next())
        .is_some_and(|shape| shape == NO_SUCH_GROUP)
}

/// [`delivered_to`] for this suite's own destination.
async fn delivered(router: &MultiServiceRouter) -> Option<Vec<String>> {
    delivered_to(router, GROUP).await
}

/// The ARN of the log group `group`, as a stage's `DestinationArn` names one.
///
/// `pub(crate)` for the reason [`emulator`] is.
pub(crate) fn log_group_arn(group: &str) -> String {
    format!(
        "arn:aws:logs:{REGION}:{account}:log-group:{group}",
        account = engine::emulator_account_id()
    )
}

/// The ARN of the destination log group, as a stage's `DestinationArn` names one.
fn group_arn() -> String {
    log_group_arn(GROUP)
}

/// The `$default` stage with auto-deploy on, carrying `AccessLogSettings` that render by
/// `format` — or none at all when `format` is `None`.
fn stage(format: Option<&str>) -> Value {
    let mut stage = json!({ "stageName": "$default", "autoDeploy": true });
    if let Some(format) = format {
        stage["accessLogSettings"] = json!({
            "destinationArn": group_arn(),
            "format": format,
        });
    }
    stage
}

/// One row: a request to the deployed stage and what the record it leaves has to say.
struct Row {
    case: &'static str,
    verb: &'static str,
    /// The path within the api's own endpoint, which for the `$default` stage is the route
    /// path itself.
    path: &'static str,
    /// The status the stage answers with: the function's own, or the api's `404` for a path
    /// no route of it matches.
    status: u16,
    /// The route key the record names, or `None` for a request that matched no route — where
    /// the format's reference renders as the placeholder.
    route_key: Option<&'static str>,
}

/// The rows: a served request, the same request again, another verb on the same path, and a
/// path no route matches.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a served request",
            verb: "GET",
            path: "/echo",
            status: 200,
            route_key: Some(ROUTE_KEY),
        },
        Row {
            case: "the same request again, which is a second record",
            verb: "GET",
            path: "/echo",
            status: 200,
            route_key: Some(ROUTE_KEY),
        },
        Row {
            case: "a verb the route does not name",
            verb: "POST",
            path: "/echo",
            status: 404,
            route_key: None,
        },
        Row {
            case: "a path no route of the api matches",
            verb: "GET",
            path: "/nothing-here",
            status: 404,
            route_key: None,
        },
    ]
}

/// Every request a stage serves leaves one record, rendered by the stage's own format, in
/// the destination it names — and the record names the request the response named.
#[tokio::test]
async fn a_stage_writes_one_record_per_request_rendered_by_its_own_format() {
    let (router, engine) = emulator();
    let function = given_function(
        &router,
        &engine,
        "the access-logged api",
        FUNCTION,
        HANDLER_SOURCE,
    )
    .await;
    let (_api, host) = given_api(
        &router,
        "the access-logged api",
        lambda_integration(&function, "2.0"),
        ROUTE_KEY,
        stage(Some(JSON_FORMAT)),
    )
    .await;

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let answered = send(&router, &host, row.verb, row.path, &[]).await;
        assert_eq!(
            answered.status, row.status,
            "case {case:?}: expected HTTP {} from the stage, got {}",
            row.status,
            answered.summary()
        );
        let request_id = answered
            .header(REQUEST_ID_HEADER)
            .unwrap_or_else(|| panic!("case {case:?}: the response named no request id"));

        let records = delivered(&router)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: no group {GROUP:?} at all"));
        assert_eq!(
            records.len(),
            index + 1,
            "case {case:?}: expected one record per request served ({}), got {records:?}",
            index + 1
        );
        let record = records
            .last()
            .unwrap_or_else(|| panic!("case {case:?}: no record in {GROUP:?}"));
        let rendered: Value = serde_json::from_str(record).unwrap_or_else(|error| {
            panic!("case {case:?}: the record {record:?} is not the JSON the format is: {error}")
        });
        assert_eq!(
            rendered,
            json!({
                "requestId": request_id,
                "routeKey": row.route_key.unwrap_or("-"),
                "httpMethod": row.verb,
                "status": row.status.to_string(),
                "stage": "$default",
                "authorizer": "-",
            }),
            "case {case:?}: expected the stage's format rendered against this call, got \
             {record:?}"
        );
    }
}

/// The same request, logged by a stage whose format is a Common Log Format line: the same
/// facts in that format's own field order.
#[tokio::test]
async fn a_common_log_format_stage_renders_the_same_call_in_that_format() {
    let (router, engine) = emulator();
    let function = given_function(
        &router,
        &engine,
        "the clf api",
        FUNCTION,
        HANDLER_SOURCE,
    )
    .await;
    let (_api, host) = given_api(
        &router,
        "the clf api",
        lambda_integration(&function, "2.0"),
        ROUTE_KEY,
        stage(Some(CLF_FORMAT)),
    )
    .await;

    let answered = send(&router, &host, "GET", "/echo", &[]).await;
    assert_eq!(
        answered.status, 200,
        "case the clf api: expected the function's own 200, got {}",
        answered.summary()
    );
    let request_id = answered
        .header(REQUEST_ID_HEADER)
        .expect("the response names the request it answered");
    let served = answered.bytes.len();
    let records = delivered(&router)
        .await
        .expect("the destination the stage names");
    let record = records
        .last()
        .unwrap_or_else(|| panic!("case the clf api: no record in {GROUP:?}, got {records:?}"));
    let expected = format!("\"GET /echo\" 200 {served} {request_id}");
    assert!(
        record.starts_with("127.0.0.1 - - [") && record.ends_with(&expected),
        "case the clf api: expected a Common Log Format line ending in {expected:?}, got \
         {record:?}"
    );
}

/// A stage carrying no `AccessLogSettings` serves its traffic and delivers nothing.
#[tokio::test]
async fn a_stage_with_no_access_log_settings_delivers_nothing() {
    let (router, engine) = emulator();
    let function = given_function(
        &router,
        &engine,
        "the unlogged api",
        FUNCTION,
        HANDLER_SOURCE,
    )
    .await;
    let (_api, host) = given_api(
        &router,
        "the unlogged api",
        lambda_integration(&function, "2.0"),
        ROUTE_KEY,
        stage(None),
    )
    .await;

    let answered = send(&router, &host, "GET", "/echo", &[]).await;
    assert_eq!(
        answered.status, 200,
        "case no access log settings: expected the stage to keep serving, got {}",
        answered.summary()
    );
    assert_eq!(
        delivered(&router).await,
        None,
        "case no access log settings: expected no destination group {GROUP:?} to exist at all"
    );
}
