// SPDX-License-Identifier: CC0-1.0

//! An invoke of a deployed HTTP api stage is answered by the BACKEND the matched route
//! fronts, run in process — the last step of the `execute-api` data plane.
//!
//! The rule every table below is one slice of: **a route of a deployed stage is answered by
//! performing its integration and reading that backend's own answer — a Lambda proxy
//! integration by invoking the function it names with the event its `PayloadFormatVersion`
//! defines and mapping the response that version defines — so the status, headers and body a
//! client receives are the function's, while an integration that produced no response is the
//! gateway's `500`, one that did not answer within its own `TimeoutInMillis` is its `503`,
//! and an `HTTP_PROXY` upstream — a URI the caller can reach, which this emulator forwards no
//! request to — is the `307` that sends the caller there.**
//!
//! Nothing here is a value this emulator fills in: every assertion below is on something a
//! function RAN and returned, which is why AWS Lambda is mounted beside this service on one
//! engine — an `AWS_PROXY` route's answer belongs to a function that lives in that service.
//!
//! The suites that already pin the steps BEFORE the integration (the claim, the three
//! addressings, stage resolution, the deployment gate, route matching, the authorization
//! gate, the CORS decoration and the payload quota) are not restated here:
//! `http_api_servable_stage.rs`, `http_api_route_precedence.rs`,
//! `http_api_cors_data_plane.rs`, `http_api_jwt_data_plane.rs`,
//! `http_api_payload_quota.rs`, `http_api_signed_invoke.rs` and
//! `http_api_request_authorizer.rs` own them.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use smithy_server_runtime::MultiServiceRouter;

use crate::websocket_handshake_data_plane::{message, send, send_with_body};

/// The Region every request below is signed for, and the Region of the functions the
/// integrations name.
const REGION: &str = "us-east-1";

/// The handler every deployment package below exposes.
const HANDLER: &str = "lambda_function.lambda_handler";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The status and sentence an HTTP api answers for an integration that produced no
/// response.
const INTEGRATION_FAULT: u16 = 500;
const INTEGRATION_FAULT_MESSAGE: &str = "Internal Server Error";

/// The redirect an `HTTP_PROXY` upstream is answered with: the caller is sent to the
/// backend rather than this emulator forwarding the request to it.
const TEMPORARY_REDIRECT: u16 = 307;

/// The status and sentence it answers for an integration that ran and did not answer within
/// its `TimeoutInMillis`.
const TIMED_OUT: u16 = 503;
const TIMED_OUT_MESSAGE: &str = "Service Unavailable";

/// The status a request the serving stage's route settings throttle is refused with, and
/// the sentence it carries.
const THROTTLED: u16 = 429;
const THROTTLED_MESSAGE: &str = "Too Many Requests";

/// The status a request an authorizer REFUSED is answered with, and the one a request whose
/// identity sources are not all present gets without the function being called.
const FORBIDDEN: u16 = 403;
const UNAUTHORIZED: u16 = 401;

/// This service and AWS Lambda on ONE engine, with request authentication off.
pub(crate) fn emulator_with_lambda() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_apigatewayv2_apigatewayv2::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
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
    signed_call(
        emulator,
        &format!("apigateway.{REGION}.amazonaws.com"),
        "apigateway",
        method,
        uri,
        body,
    )
    .await
}

/// One request against the AWS Lambda emulator; the credential scope is what routes it
/// there rather than to this service.
async fn lambda(
    emulator: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    signed_call(
        emulator,
        &format!("lambda.{REGION}.amazonaws.com"),
        "lambda",
        method,
        uri,
        body,
    )
    .await
}

/// Send one signed request and answer `(status, parsed body)`.
async fn signed_call(
    emulator: &MultiServiceRouter,
    host: &str,
    signing_name: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = match &body {
        Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("the body serializes")),
        None => bytes::Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{uri}"))
        .header("host", host)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTAPIGWV2/20261021/{REGION}/{signing_name}/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(payload)
        .expect("the request builds");
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

/// A .zip deployment package holding `source` as the handler's module, written in memory the
/// way a caller uploads one.
fn package(source: &str) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer
            .start_file("lambda_function.py", options)
            .expect("the entry starts");
        std::io::Write::write_all(&mut writer, source.as_bytes()).expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// Create a function whose code is `source`, bring the activation the clock owes forward, and
/// give it the resource-based permission an API GATEWAY integration needs to invoke it
/// ([`gateway_may_invoke`]) — the `AddPermission` a client makes beside creating the function
/// it fronts a route with, without which the gateway's invocation is not made at all. Answers
/// its ARN.
///
/// The suite that varies that grant — and that drives a function WITHOUT one — is
/// `http_api_lambda_invoke_permission.rs`.
pub(crate) async fn given_function(
    emulator: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    name: &str,
    source: &str,
) -> String {
    let arn = given_ungranted_function(emulator, engine, case, name, source).await;
    gateway_may_invoke(emulator, case, name, None).await;
    arn
}

/// [`given_function`] WITHOUT the grant: the function exists and is active, and its resource
/// policy is whatever the caller adds (or nothing at all).
pub(crate) async fn given_ungranted_function(
    emulator: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    name: &str,
    source: &str,
) -> String {
    let (status, created) = lambda(
        emulator,
        "POST",
        "/2015-03-31/functions",
        Some(json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": 10,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(source)),
            },
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateFunction answered {status} {created}"
    );
    activate(engine);
    created
        .get("FunctionArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: CreateFunction answered no FunctionArn"))
        .to_string()
}

/// Grant API Gateway the `lambda:InvokeFunction` permission on `function`, optionally
/// narrowed to `source_arn` — the `AddPermission` whose `Principal` is the gateway's service
/// principal, which is what admits an api's invocation of a route's function.
pub(crate) async fn gateway_may_invoke(
    emulator: &MultiServiceRouter,
    case: &str,
    function: &str,
    source_arn: Option<&str>,
) {
    granted(
        emulator,
        case,
        function,
        "lambda:InvokeFunction",
        "apigateway.amazonaws.com",
        source_arn,
    )
    .await;
}

/// One `AddPermission` on `function`: the statement id is derived from what the grant says, so
/// a case may add several and remove one by name ([`statement_id`]).
pub(crate) async fn granted(
    emulator: &MultiServiceRouter,
    case: &str,
    function: &str,
    action: &str,
    principal: &str,
    source_arn: Option<&str>,
) {
    let mut request = json!({
        "StatementId": statement_id(action, principal, source_arn),
        "Action": action,
        "Principal": principal,
    });
    if let Some(source_arn) = source_arn {
        request["SourceArn"] = json!(source_arn);
    }
    let (status, granted) = lambda(
        emulator,
        "POST",
        &format!("/2015-03-31/functions/{function}/policy"),
        Some(request),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: AddPermission answered {status} {granted}"
    );
}

/// Take one statement back off `function`'s resource policy — the `RemovePermission` a client
/// makes when it revokes a grant.
pub(crate) async fn revoked(
    emulator: &MultiServiceRouter,
    case: &str,
    function: &str,
    action: &str,
    principal: &str,
    source_arn: Option<&str>,
) {
    let statement = statement_id(action, principal, source_arn);
    let (status, removed) = lambda(
        emulator,
        "DELETE",
        &format!("/2015-03-31/functions/{function}/policy/{statement}"),
        None,
    )
    .await;
    assert_eq!(
        status, 204,
        "case {case:?}: RemovePermission answered {status} {removed}"
    );
}

/// The `StatementId` one grant is stored under: derived from the grant itself, so the same
/// grant is added and removed by one name and two different grants never collide. Kept inside
/// the length the member admits (`AddPermission$StatementId`, at most 100 characters) by
/// taking the END of the derived name, which is where a source ARN differs.
fn statement_id(action: &str, principal: &str, source_arn: Option<&str>) -> String {
    let scope = source_arn.unwrap_or("any");
    let name: String = format!("{action}-{principal}-{scope}")
        .chars()
        .map(|character| if character.is_ascii_alphanumeric() { character } else { '-' })
        .collect();
    let keep = 100;
    match name.char_indices().nth(name.chars().count().saturating_sub(keep)) {
        Some((at, _)) => name[at..].to_string(),
        None => name,
    }
}

/// Replace one function's code, so a later invocation runs the new handler.
async fn update_function_code(
    emulator: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    name: &str,
    source: &str,
) {
    let (status, updated) = lambda(
        emulator,
        "PUT",
        &format!("/2015-03-31/functions/{name}/code"),
        Some(json!({
            "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(source)),
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: UpdateFunctionCode answered {status} {updated}"
    );
    activate(engine);
}

/// Bring every function's activation forward: the transition is owed by the CLOCK, so
/// nothing here sleeps on the emulator's behalf.
fn activate(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for function in store.functions.values_mut() {
        function.active_due = past;
    }
}

/// An api, one integration, one route over it and one stage with auto-deploy on. Answers
/// `(ApiId, the host its ApiEndpoint names)`.
pub(crate) async fn given_api(
    emulator: &MultiServiceRouter,
    case: &str,
    integration: Value,
    route_key: &str,
    stage: Value,
) -> (String, String) {
    let (status, api) = control(
        emulator,
        "POST",
        "/v2/apis",
        Some(json!({ "name": format!("api-{}", route_key.replace(['/', ' ', '{', '}', '+'], "-")), "protocolType": "HTTP" })),
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
    let (status, created) = control(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/integrations"),
        Some(integration),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateIntegration answered {status} {created}"
    );
    let integration_id = created
        .get("integrationId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let (status, route) = control(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/routes"),
        Some(json!({
            "routeKey": route_key,
            "target": format!("integrations/{integration_id}")
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateRoute answered {status} {route}"
    );
    let (status, created) = control(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(stage),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateStage answered {status} {created}"
    );
    (api_id, host)
}

/// The `$default` stage with auto-deploy on: the stage that serves the api's root.
fn root_stage() -> Value {
    json!({ "stageName": "$default", "autoDeploy": true })
}

/// A Lambda proxy integration over `function_arn` in `payload_format`.
pub(crate) fn lambda_integration(function_arn: &str, payload_format: &str) -> Value {
    json!({
        "integrationType": "AWS_PROXY",
        "integrationUri": function_arn,
        "payloadFormatVersion": payload_format
    })
}

/// A handler that answers the structured response the format defines.
const STRUCTURED: &str = "import json\n\
     def lambda_handler(event, context):\n\
     \x20   return {'statusCode': 201, 'headers': {'x-made-by': 'the-function'},\n\
     \x20            'body': json.dumps({'ran': True})}\n";

/// A handler that answers a document naming no status at all.
const INFERRED: &str = "def lambda_handler(event, context):\n    return {'ok': True}\n";

/// A handler that ECHOES the event it was given as its response body, which is how the
/// event's own members are asserted.
pub(crate) const ECHO_EVENT: &str = "import json\n\
     def lambda_handler(event, context):\n\
     \x20   return {'statusCode': 200, 'headers': {'content-type': 'application/json'},\n\
     \x20            'body': json.dumps(event)}\n";

#[tokio::test]
async fn a_lambda_proxy_route_answers_with_what_the_function_returned() {
    // Rows differ along the payload format version and the SHAPE of what the handler
    // returned: the structured response both versions define, the document `2.0` infers a
    // response from, a handler that raised, and a handler whose answer the version cannot
    // be read as.
    let cases: [(&str, &str, &str, u16, Option<&str>); 6] = [
        (
            "the structured response under 2.0",
            "2.0",
            STRUCTURED,
            201,
            Some(r#"{"ran": true}"#),
        ),
        (
            "the structured response under 1.0",
            "1.0",
            STRUCTURED,
            201,
            Some(r#"{"ran": true}"#),
        ),
        (
            "a document naming no status, which 2.0 infers a 200 for",
            "2.0",
            INFERRED,
            200,
            Some(r#"{"ok":true}"#),
        ),
        (
            "the same document under 1.0, which infers nothing",
            "1.0",
            INFERRED,
            INTEGRATION_FAULT,
            None,
        ),
        (
            "a handler that raised",
            "2.0",
            "def lambda_handler(event, context):\n    raise ValueError('boom')\n",
            INTEGRATION_FAULT,
            None,
        ),
        (
            "a handler that answered a status no response can carry",
            "2.0",
            "def lambda_handler(event, context):\n    return {'statusCode': 99}\n",
            INTEGRATION_FAULT,
            None,
        ),
    ];
    for (case, payload_format, source, expected_status, expected_body) in cases {
        let (emulator, engine) = emulator_with_lambda();
        let function =
            given_function(&emulator, &engine, case, "answering-function", source).await;
        let (_, host) = given_api(
            &emulator,
            case,
            lambda_integration(&function, payload_format),
            "GET /orders",
            root_stage(),
        )
        .await;
        let answer = send(&emulator, &host, "GET", "/orders", &[]).await;
        assert_eq!(
            answer.status,
            expected_status,
            "case {case:?}: expected {expected_status} from the route, got {}",
            answer.summary()
        );
        match expected_body {
            Some(body) => {
                assert_eq!(
                    answer.text, body,
                    "case {case:?}: expected the function's own body {body:?}, got {}",
                    answer.summary()
                );
            }
            None => assert_eq!(
                message(&answer),
                INTEGRATION_FAULT_MESSAGE,
                "case {case:?}: expected the integration-fault sentence, got {}",
                answer.summary()
            ),
        }
    }
    // And the response HEADERS the function set reach the client, as does a cookie it set
    // through the member `2.0` carries them in.
    let (emulator, engine) = emulator_with_lambda();
    let case = "the headers and cookies a 2.0 response sets";
    let function = given_function(
        &emulator,
        &engine,
        case,
        "cookie-function",
        "def lambda_handler(event, context):\n    \
         return {'statusCode': 200, 'headers': {'x-made-by': 'the-function'},\n    \
         'cookies': ['a=1; Path=/', 'b=2'], 'body': ''}\n",
    )
    .await;
    let (_, host) = given_api(
        &emulator,
        case,
        lambda_integration(&function, "2.0"),
        "GET /orders",
        root_stage(),
    )
    .await;
    let answer = send(&emulator, &host, "GET", "/orders", &[]).await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: expected the function's 200, got {}",
        answer.summary()
    );
    assert_eq!(
        answer.header("x-made-by").as_deref(),
        Some("the-function"),
        "case {case:?}: expected the header the function set, got {}",
        answer.summary()
    );
    assert_eq!(
        answer.all_headers("set-cookie"),
        vec!["a=1; Path=/".to_string(), "b=2".to_string()],
        "case {case:?}: expected one set-cookie field per cookie, got {:?}",
        answer.all_headers("set-cookie")
    );
}

#[tokio::test]
async fn the_event_the_function_receives_states_the_request_as_the_payload_format_defines_it() {
    // Rows differ along the payload format version and the stage the request is addressed
    // to: the path the event reports is the one WITHIN the api, so a named stage's event
    // carries the stage segment and the root stage's does not.
    let cases: [(&str, &str, &str, &str, &str); 4] = [
        ("2.0 on the root stage", "2.0", "$default", "/orders/7", "/orders/7"),
        ("2.0 on a named stage", "2.0", "prod", "/prod/orders/7", "/prod/orders/7"),
        ("1.0 on the root stage", "1.0", "$default", "/orders/7", "/orders/7"),
        ("1.0 on a named stage", "1.0", "prod", "/prod/orders/7", "/prod/orders/7"),
    ];
    for (case, payload_format, stage_name, request_path, expected_path) in cases {
        let (emulator, engine) = emulator_with_lambda();
        let function = given_function(&emulator, &engine, case, "echo-function", ECHO_EVENT).await;
        let (api_id, host) = given_api(
            &emulator,
            case,
            lambda_integration(&function, payload_format),
            "GET /orders/{id}",
            json!({
                "stageName": stage_name,
                "autoDeploy": true,
                "stageVariables": { "backend": "blue" }
            }),
        )
        .await;
        let answer = send(
            &emulator,
            &host,
            "GET",
            &format!("{request_path}?a=1&a=2&b=3"),
            &[("cookie", "one=1; two=2"), ("x-caller", "suite")],
        )
        .await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: expected the function's 200, got {}",
            answer.summary()
        );
        let event: Value = serde_json::from_str(&answer.text).unwrap_or(Value::Null);
        // Each row of this inner table is one member of the documented event and the value
        // this request owes it.
        let members: Vec<(&str, &str, Value)> = if payload_format == "2.0" {
            vec![
                ("the format version", "/version", json!("2.0")),
                ("the matched key", "/routeKey", json!("GET /orders/{id}")),
                ("the path within the api", "/rawPath", json!(expected_path)),
                ("the query string as it arrived", "/rawQueryString", json!("a=1&a=2&b=3")),
                ("the cookies lifted out of the header", "/cookies", json!(["one=1", "two=2"])),
                ("a header the client sent", "/headers/x-caller", json!("suite")),
                ("the repeated query parameter", "/queryStringParameters/a", json!("1,2")),
                ("the path parameter the key names", "/pathParameters/id", json!("7")),
                ("the stage's variables", "/stageVariables/backend", json!("blue")),
                ("the method", "/requestContext/http/method", json!("GET")),
                ("the stage", "/requestContext/stage", json!(stage_name)),
                ("the api", "/requestContext/apiId", json!(api_id)),
                ("the path, again, in the request context", "/requestContext/http/path", json!(expected_path)),
            ]
        } else {
            vec![
                ("the format version", "/version", json!("1.0")),
                ("the key's path as the resource", "/resource", json!("/orders/{id}")),
                ("the path within the api", "/path", json!(expected_path)),
                ("the method on the event itself", "/httpMethod", json!("GET")),
                ("a header the client sent", "/headers/x-caller", json!("suite")),
                (
                    "the multi-valued query map",
                    "/multiValueQueryStringParameters/a",
                    json!(["1", "2"]),
                ),
                ("the path parameter the key names", "/pathParameters/id", json!("7")),
                ("the stage's variables", "/stageVariables/backend", json!("blue")),
                ("the stage", "/requestContext/stage", json!(stage_name)),
            ]
        };
        for (member, pointer, expected) in members {
            let actual = event.pointer(pointer).cloned().unwrap_or(Value::Null);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {member} at {pointer} to be {expected}, got \
                 {actual} in {}",
                answer.text
            );
        }
    }
}

#[tokio::test]
async fn a_payload_that_is_not_text_reaches_the_function_and_the_client_as_its_bytes() {
    // The handler echoes the body it was given back under the same flag, so what the client
    // receives is what it sent: the rule holds in both directions, for a payload that can
    // travel as text and for one that cannot.
    let source = "def lambda_handler(event, context):\n    \
         return {'statusCode': 200, 'isBase64Encoded': event.get('isBase64Encoded', False),\n    \
         'headers': {'content-type': 'application/octet-stream'},\n    \
         'body': event.get('body', '')}\n";
    // Rows differ along what the payload IS: text, bytes no text can hold, and nothing.
    let cases: [(&str, Vec<u8>); 3] = [
        ("a text payload", b"hello".to_vec()),
        (
            "a payload that is not valid UTF-8",
            vec![0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0xff, 0x00],
        ),
        ("no payload at all", Vec::new()),
    ];
    for (case, payload) in cases {
        let (emulator, engine) = emulator_with_lambda();
        let function = given_function(&emulator, &engine, case, "echo-bytes", source).await;
        let (_, host) = given_api(
            &emulator,
            case,
            lambda_integration(&function, "2.0"),
            "POST /upload",
            root_stage(),
        )
        .await;
        let answer = send_with_body(
            &emulator,
            &host,
            "POST",
            "/upload",
            &[("content-type", "application/octet-stream")],
            payload.clone(),
        )
        .await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: expected the function's 200, got {}",
            answer.summary()
        );
        assert_eq!(
            answer.bytes.to_vec(),
            payload,
            "case {case:?}: expected the bytes that were sent, got {:?}",
            answer.bytes
        );
    }
}

#[tokio::test]
async fn an_integration_that_does_not_answer_in_time_is_503_while_its_twin_serves() {
    // Two routes of ONE api over two integrations of the same function: one whose
    // `TimeoutInMillis` the run cannot meet, and one whose it can. The twin is what shows
    // the refusal belongs to the integration's own deadline and not to the api.
    let case = "a route whose integration overruns its timeout, beside one that does not";
    let (emulator, engine) = emulator_with_lambda();
    let function = given_function(
        &emulator,
        &engine,
        case,
        "slow-function",
        "import time\n\
         def lambda_handler(event, context):\n\
         \x20   time.sleep(1)\n\
         \x20   return {'statusCode': 200, 'body': 'late'}\n",
    )
    .await;
    let (api_id, host) = given_api(
        &emulator,
        case,
        json!({
            "integrationType": "AWS_PROXY",
            "integrationUri": function,
            "payloadFormatVersion": "2.0",
            "timeoutInMillis": 50
        }),
        "GET /slow",
        root_stage(),
    )
    .await;
    // The twin: the same function behind a route whose integration allows the whole run.
    let (status, patient) = control(
        &emulator,
        "POST",
        &format!("/v2/apis/{api_id}/integrations"),
        Some(json!({
            "integrationType": "AWS_PROXY",
            "integrationUri": function,
            "payloadFormatVersion": "2.0",
            "timeoutInMillis": 29_000
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateIntegration for the twin answered {status} {patient}"
    );
    let (status, route) = control(
        &emulator,
        "POST",
        &format!("/v2/apis/{api_id}/routes"),
        Some(json!({
            "routeKey": "GET /patient",
            "target": format!("integrations/{}", patient.get("integrationId").and_then(Value::as_str).unwrap_or_default())
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateRoute for the twin answered {status} {route}"
    );

    let refused = send(&emulator, &host, "GET", "/slow", &[]).await;
    assert_eq!(
        refused.status,
        TIMED_OUT,
        "case {case:?}: expected {TIMED_OUT} for the route whose integration overran its \
         timeout, got {}",
        refused.summary()
    );
    assert_eq!(
        message(&refused),
        TIMED_OUT_MESSAGE,
        "case {case:?}: expected the timeout sentence, got {}",
        refused.summary()
    );
    let served = send(&emulator, &host, "GET", "/patient", &[]).await;
    assert_eq!(
        served.status,
        200,
        "case {case:?}: expected the twin route to be served, got {}",
        served.summary()
    );
    assert_eq!(
        served.text, "late",
        "case {case:?}: expected the function's own body, got {}",
        served.summary()
    );
}

/// A `REQUEST` authorizer over the api `api_id`, answering its `AuthorizerId`.
async fn given_authorizer(
    emulator: &MultiServiceRouter,
    case: &str,
    api_id: &str,
    function_arn: &str,
    payload_format: &str,
    simple_responses: bool,
    ttl_seconds: i32,
) -> String {
    let (status, created) = control(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/authorizers"),
        Some(json!({
            "name": "gate",
            "authorizerType": "REQUEST",
            "authorizerUri": format!(
                "arn:aws:apigateway:{REGION}:lambda:path/2015-03-31/functions/{function_arn}/invocations"
            ),
            "authorizerPayloadFormatVersion": payload_format,
            "enableSimpleResponses": simple_responses,
            "authorizerResultTtlInSeconds": ttl_seconds,
            "identitySource": ["$request.header.authorization"]
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateAuthorizer answered {status} {created}"
    );
    created
        .get("authorizerId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Put the route `GET /orders` of `api_id` behind the authorizer, and release it.
async fn given_authorized_route(
    emulator: &MultiServiceRouter,
    case: &str,
    api_id: &str,
    integration_id: &str,
    authorizer_id: &str,
) {
    let (status, route) = control(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/routes"),
        Some(json!({
            "routeKey": "GET /orders",
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
}

/// The whole api a `REQUEST`-authorizer case drives: the integration's function, the
/// authorizer's function, the gated route and the stage. Answers the invoke host.
async fn given_authorized_api(
    emulator: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    authorizer_source: &str,
    payload_format: &str,
    simple_responses: bool,
    ttl_seconds: i32,
) -> String {
    let backend = given_function(emulator, engine, case, "gated-backend", ECHO_EVENT).await;
    let gate = given_function(emulator, engine, case, "gate-function", authorizer_source).await;
    let (status, api) = control(
        emulator,
        "POST",
        "/v2/apis",
        Some(json!({ "name": "gated-api", "protocolType": "HTTP" })),
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
        Some(lambda_integration(&backend, "2.0")),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateIntegration answered {status} {integration}"
    );
    let authorizer = given_authorizer(
        emulator,
        case,
        &api_id,
        &gate,
        payload_format,
        simple_responses,
        ttl_seconds,
    )
    .await;
    given_authorized_route(
        emulator,
        case,
        &api_id,
        integration
            .get("integrationId")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        &authorizer,
    )
    .await;
    let (status, stage) = control(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(root_stage()),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateStage answered {status} {stage}"
    );
    host
}

/// A simple-response authorizer: it authorizes the one token and refuses everything else,
/// and returns a context whose value is an ARRAY.
const SIMPLE_GATE: &str = "def lambda_handler(event, context):\n    \
     allowed = event['headers'].get('authorization') == 'let-me-in'\n    \
     return {'isAuthorized': allowed, 'context': {'tier': ['gold', 'silver']}}\n";

/// A policy-document authorizer: it answers the IAM policy an authorizer returns by
/// default, allowing the route it was asked about or denying it.
const POLICY_GATE: &str = "def lambda_handler(event, context):\n    \
     effect = 'Allow' if event['headers'].get('authorization') == 'let-me-in' else 'Deny'\n    \
     arn = event.get('methodArn') or event.get('routeArn')\n    \
     return {'principalId': 'user-7', 'policyDocument': {'Version': '2012-10-17',\n    \
     'Statement': [{'Action': 'execute-api:Invoke', 'Effect': effect,\n    \
     'Resource': arn}]}, 'context': {'tier': 'gold'}}\n";

#[tokio::test]
async fn a_request_authorizers_own_decision_gates_the_route() {
    // Rows differ along the configuration the decision is read under and what the function
    // answered: the simple response a `2.0` authorizer that enabled it returns, the IAM
    // policy every other authorizer returns, and a simple response under `1.0`, which
    // defines none — so the function stated no decision this authorizer can read.
    let cases: [(&str, &str, &str, bool, &str, u16); 7] = [
        ("a simple response that authorizes", SIMPLE_GATE, "2.0", true, "let-me-in", 200),
        ("a simple response that refuses", SIMPLE_GATE, "2.0", true, "nope", FORBIDDEN),
        ("a policy that allows the route", POLICY_GATE, "1.0", false, "let-me-in", 200),
        ("a policy that denies the route", POLICY_GATE, "1.0", false, "nope", FORBIDDEN),
        (
            "a policy read by a 2.0 authorizer that did not enable simple responses",
            POLICY_GATE,
            "2.0",
            false,
            "let-me-in",
            200,
        ),
        (
            "a simple response under payload format 1.0, which defines none",
            SIMPLE_GATE,
            "1.0",
            true,
            "let-me-in",
            INTEGRATION_FAULT,
        ),
        (
            "an authorizer whose function raised",
            "def lambda_handler(event, context):\n    raise ValueError('boom')\n",
            "2.0",
            true,
            "let-me-in",
            INTEGRATION_FAULT,
        ),
    ];
    for (case, source, payload_format, simple, token, expected) in cases {
        let (emulator, engine) = emulator_with_lambda();
        let host = given_authorized_api(
            &emulator, &engine, case, source, payload_format, simple, 300,
        )
        .await;
        let answer = send(
            &emulator,
            &host,
            "GET",
            "/orders",
            &[("authorization", token)],
        )
        .await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected {expected} from the gated route, got {}",
            answer.summary()
        );
    }

    // A request presenting none of the authorizer's identity sources never reaches the
    // function: "it returns a 401 Unauthorized response without calling the Lambda
    // function".
    let case = "a request presenting no identity source at all";
    let (emulator, engine) = emulator_with_lambda();
    let host =
        given_authorized_api(&emulator, &engine, case, SIMPLE_GATE, "2.0", true, 300).await;
    let answer = send(&emulator, &host, "GET", "/orders", &[]).await;
    assert_eq!(
        answer.status, UNAUTHORIZED,
        "case {case:?}: expected {UNAUTHORIZED} for a request missing the identity source, \
         got {}",
        answer.summary()
    );
}

#[tokio::test]
async fn the_context_the_authorizer_returned_reaches_the_integration_unchanged() {
    // The integration's handler echoes its event, so what the authorizer returned is read
    // off the event the function was given: a value that is an ARRAY arrives as one.
    let case = "a simple-response authorizer returning an array context value";
    let (emulator, engine) = emulator_with_lambda();
    let host = given_authorized_api(
        &emulator, &engine, case, SIMPLE_GATE, "2.0", true, 300,
    )
    .await;
    let answer = send(
        &emulator,
        &host,
        "GET",
        "/orders",
        &[("authorization", "let-me-in")],
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: expected the gated route to be served, got {}",
        answer.summary()
    );
    let event: Value = serde_json::from_str(&answer.text).unwrap_or(Value::Null);
    assert_eq!(
        event.pointer("/requestContext/authorizer/lambda/tier"),
        Some(&json!(["gold", "silver"])),
        "case {case:?}: expected the authorizer's own context under \
         requestContext.authorizer.lambda, got {}",
        answer.text
    );
}

#[tokio::test]
async fn a_decision_is_cached_for_its_ttl_and_a_reset_takes_it_away() {
    // The function is REPLACED between the two requests, so the second request's answer
    // says which decision served it: the cached one (the first function's) or a fresh
    // invocation (the second function's). Rows differ along the TTL the authorizer
    // declared — the documented caching window, and the value that disables caching.
    let cases: [(&str, i32, u16); 2] = [
        ("an authorizer caching its decisions", 300, 200),
        ("an authorizer whose TTL disables caching", 0, FORBIDDEN),
    ];
    for (case, ttl, after_replacement) in cases {
        let (emulator, engine) = emulator_with_lambda();
        let host = given_authorized_api(
            &emulator, &engine, case, SIMPLE_GATE, "2.0", true, ttl,
        )
        .await;
        let first = send(
            &emulator,
            &host,
            "GET",
            "/orders",
            &[("authorization", "let-me-in")],
        )
        .await;
        assert_eq!(
            first.status,
            200,
            "case {case:?}: expected the first request to be authorized, got {}",
            first.summary()
        );
        // The same identity, now against a function that refuses everything.
        update_function_code(
            &emulator,
            &engine,
            case,
            "gate-function",
            "def lambda_handler(event, context):\n    return {'isAuthorized': False}\n",
        )
        .await;
        let second = send(
            &emulator,
            &host,
            "GET",
            "/orders",
            &[("authorization", "let-me-in")],
        )
        .await;
        assert_eq!(
            second.status, after_replacement,
            "case {case:?}: expected {after_replacement} once the function was replaced, \
             got {}",
            second.summary()
        );
        // A DIFFERENT identity is a different cache key, so it is decided by the function
        // as it stands now — which refuses it — whatever was cached for the first.
        let other = send(
            &emulator,
            &host,
            "GET",
            "/orders",
            &[("authorization", "another-token")],
        )
        .await;
        assert_eq!(
            other.status,
            FORBIDDEN,
            "case {case:?}: expected a different identity to be decided on its own, got {}",
            other.summary()
        );
    }

    // And `ResetAuthorizersCache` takes away what the stage has cached, so the next request
    // is decided by the function again.
    let case = "a reset of the stage's authorizer cache";
    let (emulator, engine) = emulator_with_lambda();
    let host =
        given_authorized_api(&emulator, &engine, case, SIMPLE_GATE, "2.0", true, 3600).await;
    let api_id = host.split('.').next().unwrap_or_default().to_string();
    let authorized = send(
        &emulator,
        &host,
        "GET",
        "/orders",
        &[("authorization", "let-me-in")],
    )
    .await;
    assert_eq!(
        authorized.status,
        200,
        "case {case:?}: expected the first request to be authorized, got {}",
        authorized.summary()
    );
    update_function_code(
        &emulator,
        &engine,
        case,
        "gate-function",
        "def lambda_handler(event, context):\n    return {'isAuthorized': False}\n",
    )
    .await;
    let (status, reset) = control(
        &emulator,
        "DELETE",
        &format!("/v2/apis/{api_id}/stages/$default/cache/authorizers"),
        None,
    )
    .await;
    // The status API Gateway answers a flush with, not the model's `code: 204`
    // (`crate::state::FLUSHED_CACHE_OK_STATUS`).
    assert_eq!(
        status, 200,
        "case {case:?}: ResetAuthorizersCache answered {status} {reset}"
    );
    let after = send(
        &emulator,
        &host,
        "GET",
        "/orders",
        &[("authorization", "let-me-in")],
    )
    .await;
    assert_eq!(
        after.status,
        FORBIDDEN,
        "case {case:?}: expected the reset to send the next request to the function again, \
         got {}",
        after.summary()
    );
}

#[tokio::test]
async fn a_route_the_serving_stage_throttles_is_429_while_its_twin_serves() {
    // The stage states route settings for ONE of its two routes, so the same api, the same
    // function and the same stage answer differently for the two: the throttled route's
    // bucket holds one request and refills at no rate, while the twin's holds whatever the
    // stage states for it, which is nothing. The rate is zero so the assertion is the
    // bucket's CAPACITY and not the clock: what the rate refills over time is pinned, with a
    // clock the case controls, by `state::route_throttle`'s own table.
    let case = "a route the stage throttles beside one it states no settings for";
    let (emulator, engine) = emulator_with_lambda();
    let function =
        given_function(&emulator, &engine, case, "metered-function", STRUCTURED).await;
    let (api_id, host) = given_api(
        &emulator,
        case,
        lambda_integration(&function, "2.0"),
        "GET /metered",
        json!({
            "stageName": "$default",
            "autoDeploy": true,
            "routeSettings": {
                "GET /metered": { "throttlingRateLimit": 0.0, "throttlingBurstLimit": 1 }
            }
        }),
    )
    .await;
    let (status, twin) = control(
        &emulator,
        "POST",
        &format!("/v2/apis/{api_id}/routes"),
        Some(json!({
            "routeKey": "GET /twin",
            "target": format!("integrations/{}", first_integration_id(&emulator, &api_id).await)
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateRoute for the twin answered {status} {twin}"
    );

    // The burst admits one request of the metered route; every later one is refused by the
    // gateway while the burst is spent, and the route the stage meters nothing on is served.
    let first = send(&emulator, &host, "GET", "/metered", &[]).await;
    assert_eq!(
        first.status,
        201,
        "case {case:?}: expected the burst to admit the first request, got {}",
        first.summary()
    );
    for attempt in ["the request past the burst", "a later request still past it"] {
        let throttled = send(&emulator, &host, "GET", "/metered", &[]).await;
        assert_eq!(
            throttled.status,
            THROTTLED,
            "case {case:?}: expected {THROTTLED} for {attempt}, got {}",
            throttled.summary()
        );
        assert_eq!(
            message(&throttled),
            THROTTLED_MESSAGE,
            "case {case:?}: expected the throttling sentence for {attempt}, got {}",
            throttled.summary()
        );
    }
    let twin = send(&emulator, &host, "GET", "/twin", &[]).await;
    assert_eq!(
        twin.status,
        201,
        "case {case:?}: expected the twin route to be served, got {}",
        twin.summary()
    );
}

/// The `IntegrationId` of the api's one integration, for a second route pointed at it.
async fn first_integration_id(emulator: &MultiServiceRouter, api_id: &str) -> String {
    let (status, listed) = control(
        emulator,
        "GET",
        &format!("/v2/apis/{api_id}/integrations"),
        None,
    )
    .await;
    assert_eq!(status, 200, "GetIntegrations answered {status} {listed}");
    listed
        .pointer("/items/0/integrationId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn an_http_proxy_route_sends_the_caller_to_the_endpoint_it_names() {
    // An `HTTP_PROXY` upstream is a URI the CALLER can reach, and this emulator forwards no
    // request to a destination its own configuration names, so the caller is sent there:
    // `307`, with the upstream the integration resolved in `Location`. The rows differ along
    // where the upstream comes from — the integration's literal URI, and one a stage variable
    // selects — and the row that resolves to no backend at all.
    let (emulator, _engine) = emulator_with_lambda();
    let cases: [(&str, &str, &str, Value, Option<&str>); 3] = [
        (
            "a literal upstream",
            "GET /literal",
            "https://upstream.example.test/items",
            json!({ "stageName": "$default", "autoDeploy": true }),
            Some("https://upstream.example.test/items"),
        ),
        (
            "an upstream a stage variable selects",
            "GET /selected",
            "https://${stageVariables.backendUrl}/items",
            json!({
                "stageName": "$default",
                "autoDeploy": true,
                "stageVariables": { "backendUrl": "selected.example.test" }
            }),
            Some("https://selected.example.test/items"),
        ),
        (
            "an upstream naming a variable the stage does not carry",
            "GET /unresolved",
            "https://${stageVariables.missing}/items",
            json!({ "stageName": "$default", "autoDeploy": true }),
            None,
        ),
    ];
    for (case, route_key, integration_uri, stage, expected) in cases {
        let (_, host) = given_api(
            &emulator,
            case,
            json!({
                "integrationType": "HTTP_PROXY",
                "integrationUri": integration_uri,
                "integrationMethod": "ANY",
                "payloadFormatVersion": "1.0"
            }),
            route_key,
            stage,
        )
        .await;
        let path = route_key.split_once(' ').map(|(_, path)| path).unwrap_or("/");
        let answer = send(&emulator, &host, "GET", path, &[]).await;
        match expected {
            Some(location) => {
                assert_eq!(
                    answer.status,
                    TEMPORARY_REDIRECT,
                    "case {case:?}: expected the caller to be sent to the upstream, got {}",
                    answer.summary()
                );
                assert_eq!(
                    answer.location.as_deref(),
                    Some(location),
                    "case {case:?}: expected {location:?} in Location, got {}",
                    answer.summary()
                );
            }
            None => {
                assert_eq!(
                    answer.status,
                    INTEGRATION_FAULT,
                    "case {case:?}: an upstream that resolves to no backend is the \
                     integration-fault class, got {}",
                    answer.summary()
                );
                assert_eq!(
                    answer.location, None,
                    "case {case:?}: nothing may be sent to an unresolved backend, got {}",
                    answer.summary()
                );
            }
        }
    }
}
