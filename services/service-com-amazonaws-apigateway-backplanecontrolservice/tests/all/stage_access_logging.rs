// SPDX-License-Identifier: CC0-1.0

//! What a deployed REST stage LOGS about the requests it serves: the access-log record its
//! own `accessLogSettings` ask for, and the execution log its method settings turn on.
//!
//! The rule under test, one level above the recorded case:
//!
//! > With the ACCOUNT's CloudWatch role installed (`UpdateAccount` `/cloudwatchRoleArn`), a
//! > stage carrying `accessLogSettings` — a `destinationArn` naming a log group and a
//! > `$context` format — writes ONE record per request it served into THAT group, rendered
//! > by THAT format: one request is one record, a request the api refused is a request the
//! > stage answered and so is logged too, and each stage writes only into the destination it
//! > names. A stage whose method settings carry `loggingLevel` writes the execution log of
//! > the request beside it, in the group API Gateway publishes for that stage
//! > (`API-Gateway-Execution-Logs_{api-id}/{stage}`), with the payloads of the call in it
//! > only where `dataTrace` is on. With no account CloudWatch role there is nothing for API
//! > Gateway to write as, so the stage serves its traffic and delivers nothing at all.
//!
//! Only a cross-service suite can pin this: the api, the stage and its logging configuration
//! are API Gateway's, the integration's run is AWS Lambda's, and the groups the records land
//! in are CloudWatch Logs' — all three on ONE engine, as `core/router/src/lib.rs` mounts
//! them. Every assertion reads the outcome back through CloudWatch Logs' own reader
//! (`FilterLogEvents`), so a record that did not reach that store cannot pass.
//!
//! The rows differ along what the rule generalises over: the FORMAT (a JSON one and a
//! Common-Log-Format one), the STAGE a request reaches (so the per-destination isolation is
//! a row rather than an aside), the OUTCOME (a request the integration served and one the
//! api refused), the REPETITION of one request, and — for the execution log — the
//! `loggingLevel`/`dataTrace` pair in force.

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for and every ARN names.
const REGION: &str = "us-east-1";

/// The two stages the api is deployed to, and the resource path of its one method.
const PROD: &str = "prod";
const DEV: &str = "dev";
const RESOURCE_PATH: &str = "echo";

/// The access-log destination of each stage: its OWN group, which is what makes the
/// per-stage isolation observable.
const PROD_GROUP: &str = "/apigw/access/prod";
const DEV_GROUP: &str = "/apigw/access/dev";

/// The format each stage renders its records by: a JSON one and a Common Log Format one.
const JSON_FORMAT: &str = concat!(
    r#"{"requestId":"$context.requestId","httpMethod":"$context.httpMethod","#,
    r#""resourcePath":"$context.resourcePath","status":"$context.status","#,
    r#""stage":"$context.stage","user":"$context.identity.user"}"#
);
const CLF_FORMAT: &str = concat!(
    r#"$context.identity.sourceIp - - [$context.requestTime] ""#,
    r#"$context.httpMethod $context.resourcePath" $context.status "#,
    r#"$context.responseLength $context.requestId"#
);

/// The account-level role API Gateway writes the account's logs as
/// (`Account$cloudwatchRoleArn`). No IAM store is mounted here, so it is a value the patch
/// carries rather than a record this suite relies on.
const CLOUDWATCH_ROLE: &str = "arn:aws:iam::000000000000:role/apigw-cloudwatch";

/// The function the method is integrated with, and the role the created function names.
const FUNCTION: &str = "access-logged";
const ROLE: &str = "arn:aws:iam::000000000000:role/lambda-exec";
const HANDLER: &str = "lambda_function.lambda_handler";

/// How long this suite waits for another service's clock-owed transition (a created function
/// becoming `Active`), and how often it looks.
const WAIT: Duration = Duration::from_secs(20);
const LOOK: Duration = Duration::from_millis(100);

/// The handler behind the method: it answers the proxy response envelope with a body naming
/// what it was handed.
const HANDLER_SOURCE: &str = r#"
import json


def lambda_handler(event, context):
    which = (event.get("queryStringParameters") or {}).get("id") or "none"
    return {
        "statusCode": 200,
        "headers": {"Content-Type": "application/json"},
        "body": json.dumps({"seen": which}),
    }
"#;

/// The header API Gateway reports the id of the request it processed in, which an
/// access-log record's `$context.requestId` has to equal.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// One answered request: its status, its body verbatim and the request id it was reported
/// under.
pub(crate) struct Answered {
    pub(crate) status: u16,
    body: Bytes,
    pub(crate) request_id: Option<String>,
}

impl Answered {
    /// The body as text.
    pub(crate) fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    /// The body as JSON, `Null` when it is not JSON at all.
    pub(crate) fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

/// The three services this rule spans, over ONE engine, as `core/router/src/lib.rs` mounts
/// them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The suite exercises the services, not SigV4: the control-plane calls carry a credential
    // scope so the router can name the target service, and the invokes carry no credential at
    // all, as a client of a published invoke URL sends none.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_apigateway_backplanecontrolservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` whose credential scope names `signing_name`, which is what routes
/// the request. The signature itself is never checked here.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// Dispatch one request as it arrives on the wire.
async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Bytes,
) -> Answered {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let response = router
        .dispatch(builder.body(body).expect("a well-formed request"))
        .await;
    let (parts, body) = response.into_parts();
    Answered {
        status: parts.status.as_u16(),
        body,
        request_id: parts
            .headers
            .get(REQUEST_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string),
    }
}

/// One API Gateway control-plane call that has to be accepted with `expected`.
async fn apigw(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    expected: u16,
) -> Value {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let answered = dispatch(
        router,
        method,
        &format!("apigateway.{REGION}.amazonaws.com"),
        path,
        &[
            ("content-type", "application/json"),
            ("authorization", &authorization("apigateway")),
        ],
        Bytes::from(payload),
    )
    .await;
    assert_eq!(
        answered.status,
        expected,
        "setup {case:?}: expected HTTP {expected} from {method} {path}, got {} with {:?}",
        answered.status,
        answered.text()
    );
    answered.json()
}

/// One AWS Lambda call over its `restJson1` wire, answering its status and parsed body.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let answered = dispatch(
        router,
        method,
        &format!("lambda.{REGION}.amazonaws.com"),
        path,
        &[
            ("content-type", "application/json"),
            ("authorization", &authorization("lambda")),
        ],
        Bytes::from(payload),
    )
    .await;
    (answered.status, answered.json())
}

/// One CloudWatch Logs call over its `awsJson1_1` wire, answering its status and parsed body.
async fn logs(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let answered = dispatch(
        router,
        "POST",
        &format!("logs.{REGION}.amazonaws.com"),
        "/",
        &[
            ("content-type", "application/x-amz-json-1.1"),
            ("authorization", &authorization("logs")),
            ("x-amz-target", &format!("Logs_20140328.{operation}")),
        ],
        Bytes::from(body.to_string()),
    )
    .await;
    (answered.status, answered.json())
}

/// One request to the api's own invoke endpoint — the host
/// `{rest-api-id}.execute-api.{region}.amazonaws.com`, which is the endpoint
/// `RestApi$disableExecuteApiEndpoint` documents — carrying no credential at all, as a client
/// of a public method sends none.
pub(crate) async fn invoke(
    router: &MultiServiceRouter,
    api: &str,
    stage: &str,
    verb: &str,
    path: &str,
) -> Answered {
    dispatch(
        router,
        verb,
        &format!("{api}.execute-api.{REGION}.amazonaws.com"),
        &format!("/{stage}{path}"),
        &[("content-type", "application/json")],
        Bytes::new(),
    )
    .await
}

/// One record of a stage's execution log as the group holds it: API Gateway prefixes each
/// line of a request's trace with that request's own id in parentheses, so one group can
/// hold the traces of every request the stage served.
pub(crate) fn traced_line(request_id: &str, line: &str) -> String {
    format!("({request_id}) {line}")
}

/// The ARN of the log group `name`, as a stage's `destinationArn` names one.
fn group_arn(name: &str) -> String {
    format!(
        "arn:aws:logs:{REGION}:{account}:log-group:{name}",
        account = engine::emulator_account_id()
    )
}

/// The messages `FilterLogEvents` reports for `group`, or `None` when the account holds no
/// such group at all — which is what "nothing was delivered and nothing was created" looks
/// like to a client.
pub(crate) async fn delivered(router: &MultiServiceRouter, group: &str) -> Option<Vec<String>> {
    let (status, body) = logs(router, "FilterLogEvents", json!({ "logGroupName": group })).await;
    if status != 200 {
        return None;
    }
    Some(
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
                    .collect()
            })
            .unwrap_or_default(),
    )
}

/// A .zip deployment package holding `source` as the module the handler names, written in
/// memory the way a caller uploads one.
fn package(source: &str) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer
            .start_file("lambda_function.py", options)
            .expect("the entry starts");
        writer
            .write_all(source.as_bytes())
            .expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// The member `name` of a JSON object, as text.
fn member(case: &str, body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("setup {case:?}: no {name:?} in {body}"))
        .to_string()
}

/// Poll `GetFunctionConfiguration` until Lambda first reports the function `Active`.
async fn wait_until_active(router: &MultiServiceRouter) {
    let looks = WAIT.as_millis() / LOOK.as_millis().max(1);
    let mut last = Value::Null;
    for _ in 0..looks {
        let (_, configuration) = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{FUNCTION}/configuration"),
            None,
        )
        .await;
        if configuration.get("State").and_then(Value::as_str) == Some("Active") {
            return;
        }
        last = configuration;
        tokio::time::sleep(LOOK).await;
    }
    panic!("setup CreateFunction: the function never became Active, last read {last}");
}

/// Install the account's CloudWatch role, which is what API Gateway writes the account's
/// logs as.
async fn install_account_role(router: &MultiServiceRouter) {
    apigw(
        router,
        "UpdateAccount",
        "PATCH",
        "/account",
        Some(json!({
            "patchOperations": [
                { "op": "replace", "path": "/cloudwatchRoleArn", "value": CLOUDWATCH_ROLE },
            ],
        })),
        200,
    )
    .await;
}

/// Point the stage's access logging at `group`, rendered by `format`.
async fn configure_access_logging(
    router: &MultiServiceRouter,
    api: &str,
    stage: &str,
    group: &str,
    format: &str,
) {
    apigw(
        router,
        "UpdateStage accessLogSettings",
        "PATCH",
        &format!("/restapis/{api}/stages/{stage}"),
        Some(json!({
            "patchOperations": [
                {
                    "op": "replace",
                    "path": "/accessLogSettings/destinationArn",
                    "value": group_arn(group),
                },
                { "op": "replace", "path": "/accessLogSettings/format", "value": format },
            ],
        })),
        200,
    )
    .await;
}

/// The whole setup: a function, an api with one `GET` method proxying to it, the grant that
/// admits the invocation, and a deployment released to both stages. Answers the api id.
async fn deployed(router: &MultiServiceRouter) -> String {
    let (status, function) = lambda(
        router,
        "POST",
        "/2015-03-31/functions",
        Some(json!({
            "FunctionName": FUNCTION,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": 10,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(HANDLER_SOURCE)),
            },
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup CreateFunction: expected HTTP 201, got {status} with {function}"
    );
    let function_arn = member("CreateFunction", &function, "FunctionArn");

    let api = apigw(
        router,
        "CreateRestApi",
        "POST",
        "/restapis",
        Some(json!({ "name": "access-logged-api" })),
        201,
    )
    .await;
    let api = member("CreateRestApi", &api, "id");
    let resources = apigw(
        router,
        "GetResources",
        "GET",
        &format!("/restapis/{api}/resources"),
        None,
        200,
    )
    .await;
    let root = resources
        .get("item")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .map(|item| member("GetResources", item, "id"))
        .expect("the api's root resource");
    let resource = apigw(
        router,
        "CreateResource",
        "POST",
        &format!("/restapis/{api}/resources/{root}"),
        Some(json!({ "pathPart": RESOURCE_PATH })),
        201,
    )
    .await;
    let resource = member("CreateResource", &resource, "id");
    apigw(
        router,
        "PutMethod",
        "PUT",
        &format!("/restapis/{api}/resources/{resource}/methods/GET"),
        Some(json!({ "authorizationType": "NONE" })),
        201,
    )
    .await;
    apigw(
        router,
        "PutIntegration",
        "PUT",
        &format!("/restapis/{api}/resources/{resource}/methods/GET/integration"),
        Some(json!({
            "type": "AWS_PROXY",
            "httpMethod": "POST",
            "uri": format!(
                "arn:aws:apigateway:{REGION}:lambda:path/2015-03-31/functions/\
                 {function_arn}/invocations"
            ),
        })),
        201,
    )
    .await;
    // "Grants a principal permission to use a function" (`AddPermission`): the grant that
    // admits the invocation API Gateway makes as its own service principal.
    let (status, granted) = lambda(
        router,
        "POST",
        &format!("/2015-03-31/functions/{FUNCTION}/policy"),
        Some(json!({
            "StatementId": "apigateway-invoke",
            "Action": "lambda:InvokeFunction",
            "Principal": "apigateway.amazonaws.com",
            "SourceArn": format!(
                "arn:aws:execute-api:{REGION}:{account}:{api}/*/*/{RESOURCE_PATH}",
                account = engine::emulator_account_id(),
            ),
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup AddPermission: expected HTTP 201, got {status} with {granted}"
    );

    for stage in [PROD, DEV] {
        apigw(
            router,
            "CreateDeployment",
            "POST",
            &format!("/restapis/{api}/deployments"),
            Some(json!({ "stageName": stage })),
            201,
        )
        .await;
    }
    wait_until_active(router).await;
    api
}

/// One row of the access-log table: a request to one stage, and what the record it leaves
/// has to say.
struct Row {
    case: &'static str,
    stage: &'static str,
    /// The path within the stage, query string included.
    path: &'static str,
    /// The status the stage answers with: the integration's own, or the api's refusal for a
    /// path the deployment carries no method for.
    status: u16,
    /// The resource path the record names, which for a request that matched no method is
    /// the path the client addressed.
    resource_path: &'static str,
    /// The group the record lands in: the destination THIS stage names.
    group: &'static str,
}

/// The rows: the two stages with their two formats, a repeat of one request, and a request
/// the api refused.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a served request on the stage with a JSON format",
            stage: PROD,
            path: "/echo?id=first",
            status: 200,
            resource_path: "/echo",
            group: PROD_GROUP,
        },
        Row {
            case: "the same request again, which is a second record",
            stage: PROD,
            path: "/echo?id=second",
            status: 200,
            resource_path: "/echo",
            group: PROD_GROUP,
        },
        Row {
            case: "a served request on the other stage, with its own format",
            stage: DEV,
            path: "/echo?id=third",
            status: 200,
            resource_path: "/echo",
            group: DEV_GROUP,
        },
        Row {
            case: "a request the api refused, which the stage still answered",
            stage: PROD,
            path: "/nothing-here",
            status: 403,
            resource_path: "/nothing-here",
            group: PROD_GROUP,
        },
    ]
}

/// Every request a stage serves leaves one record in the destination THAT stage names,
/// rendered by THAT stage's format.
#[tokio::test]
async fn each_stage_writes_one_record_per_request_into_the_destination_it_names() {
    let router = emulator();
    let api = deployed(&router).await;
    install_account_role(&router).await;
    configure_access_logging(&router, &api, PROD, PROD_GROUP, JSON_FORMAT).await;
    configure_access_logging(&router, &api, DEV, DEV_GROUP, CLF_FORMAT).await;

    let mut counted: Vec<(&str, usize)> = vec![(PROD_GROUP, 0), (DEV_GROUP, 0)];
    for row in rows() {
        let case = row.case;
        let answered = invoke(&router, &api, row.stage, "GET", row.path).await;
        assert_eq!(
            answered.status, row.status,
            "case {case:?}: expected HTTP {} from the stage, got {} with {:?}",
            row.status,
            answered.status,
            answered.text()
        );
        let request_id = answered
            .request_id
            .clone()
            .unwrap_or_else(|| panic!("case {case:?}: the response named no request id"));

        for entry in &mut counted {
            if entry.0 == row.group {
                entry.1 += 1;
            }
        }
        for (group, expected) in &counted {
            // A group nothing has been delivered to yet is one API Gateway has not had to
            // create, which is no records at all.
            let records = delivered(&router, group).await.unwrap_or_default();
            assert_eq!(
                records.len(),
                *expected,
                "case {case:?}: expected {expected} record(s) in {group:?}, got {records:?}"
            );
        }

        let records = delivered(&router, row.group)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: no group {:?} at all", row.group));
        let record = records
            .last()
            .unwrap_or_else(|| panic!("case {case:?}: no record in {:?}", row.group))
            .clone();
        if row.group == PROD_GROUP {
            // The JSON format: every variable it names rendered, and a variable this plane
            // has no value for rendered as the placeholder a field with no value carries.
            let rendered: Value = serde_json::from_str(&record).unwrap_or_else(|error| {
                panic!("case {case:?}: the record {record:?} is not the JSON the format is: {error}")
            });
            assert_eq!(
                rendered,
                json!({
                    "requestId": request_id,
                    "httpMethod": "GET",
                    "resourcePath": row.resource_path,
                    "status": row.status.to_string(),
                    "stage": row.stage,
                    "user": "-",
                }),
                "case {case:?}: expected the stage's format rendered against this call, got \
                 {record:?}"
            );
        } else {
            // The Common Log Format one: the same facts, in that format's own field order.
            let expected = format!(
                "\"GET {path}\" {status} ",
                path = row.resource_path,
                status = row.status
            );
            assert!(
                record.starts_with("127.0.0.1 - - [")
                    && record.contains(&expected)
                    && record.ends_with(&request_id),
                "case {case:?}: expected a Common Log Format line for this call carrying \
                 {expected:?} and ending in {request_id:?}, got {record:?}"
            );
        }
    }
}

/// With no account CloudWatch role there is nothing for API Gateway to write the account's
/// logs as, so a stage configured for logging serves its traffic and delivers nothing.
#[tokio::test]
async fn a_stage_with_no_account_cloudwatch_role_delivers_nothing() {
    let router = emulator();
    let api = deployed(&router).await;
    // The destination exists and the stage points at it; only the ACCOUNT step is skipped.
    let (status, created) = logs(
        &router,
        "CreateLogGroup",
        json!({ "logGroupName": PROD_GROUP }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup CreateLogGroup: expected HTTP 200, got {status} with {created}"
    );
    configure_access_logging(&router, &api, PROD, PROD_GROUP, JSON_FORMAT).await;

    let answered = invoke(&router, &api, PROD, "GET", "/echo?id=unlogged").await;
    assert_eq!(
        (answered.status, answered.json()),
        (200, json!({ "seen": "unlogged" })),
        "case no account role: expected the stage to keep serving, got {} with {:?}",
        answered.status,
        answered.text()
    );
    let records = delivered(&router, PROD_GROUP)
        .await
        .expect("the destination the stage names");
    assert!(
        records.is_empty(),
        "case no account role: expected nothing delivered to {PROD_GROUP:?}, got {records:?}"
    );
    let execution = format!("API-Gateway-Execution-Logs_{api}/{PROD}");
    assert_eq!(
        delivered(&router, &execution).await,
        None,
        "case no account role: expected no execution log group {execution:?} to exist at all"
    );
}

/// One row of the execution-log table: the setting in force, and what the stage then writes.
struct Level {
    case: &'static str,
    /// The `loggingLevel` patched onto every method of the stage, or `None` to leave the
    /// stage with no method setting at all.
    level: Option<&'static str>,
    /// The `dataTrace` patched beside it.
    data_trace: bool,
    /// Whether the request leaves a trace in the stage's execution log group.
    traced: bool,
    /// Whether that trace carries the payloads of the call.
    payloads: bool,
}

/// The rows: no setting, errors only, the whole trace, and the whole trace with data.
fn levels() -> Vec<Level> {
    vec![
        Level {
            case: "no method setting at all",
            level: None,
            data_trace: false,
            traced: false,
            payloads: false,
        },
        Level {
            case: "errors only, for a request the stage served",
            level: Some("ERROR"),
            data_trace: false,
            traced: false,
            payloads: false,
        },
        Level {
            case: "the whole trace, without data tracing",
            level: Some("INFO"),
            data_trace: false,
            traced: true,
            payloads: false,
        },
        Level {
            case: "the whole trace, with data tracing",
            level: Some("INFO"),
            data_trace: true,
            traced: true,
            payloads: true,
        },
    ]
}

/// The `loggingLevel`/`dataTrace` in force decides whether a request leaves a trace in the
/// stage's execution log group, and whether that trace carries the payloads of the call.
#[tokio::test]
async fn the_method_settings_in_force_gate_the_execution_log() {
    let router = emulator();
    let api = deployed(&router).await;
    install_account_role(&router).await;
    let group = format!("API-Gateway-Execution-Logs_{api}/{PROD}");

    let mut traced = 0usize;
    for level in levels() {
        let case = level.case;
        if let Some(value) = level.level {
            apigw(
                &router,
                "UpdateStage methodSettings",
                "PATCH",
                &format!("/restapis/{api}/stages/{PROD}"),
                Some(json!({
                    "patchOperations": [
                        { "op": "replace", "path": "/*/*/logging/loglevel", "value": value },
                        {
                            "op": "replace",
                            "path": "/*/*/logging/dataTrace",
                            "value": level.data_trace.to_string(),
                        },
                    ],
                })),
                200,
            )
            .await;
        }

        let answered = invoke(&router, &api, PROD, "GET", "/echo?id=traced").await;
        assert_eq!(
            (answered.status, answered.json()),
            (200, json!({ "seen": "traced" })),
            "case {case:?}: expected the stage to serve the request, got {} with {:?}",
            answered.status,
            answered.text()
        );
        let request_id = answered
            .request_id
            .clone()
            .unwrap_or_else(|| panic!("case {case:?}: the response named no request id"));

        let records = delivered(&router, &group).await.unwrap_or_default();
        let opened = traced_line(
            &request_id,
            &format!("Execution log for request {request_id}"),
        );
        if !level.traced {
            assert!(
                !records.contains(&opened),
                "case {case:?}: expected no trace of this request in {group:?}, got {records:?}"
            );
            continue;
        }
        traced += 1;
        // THIS request's trace: the lines from its opening line to the one that closes it,
        // which is what the settings in force are asserted over — the group also holds the
        // traces of the requests before it.
        let opened_at = records
            .iter()
            .position(|record| *record == opened)
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected the trace to open with {opened:?} in {group:?}, \
                     got {records:?}"
                )
            });
        let trace: Vec<String> = {
            let rest = &records[opened_at..];
            let end = rest
                .iter()
                .position(|record| {
                    record.starts_with(&traced_line(
                        &request_id,
                        "Method completed with status: ",
                    ))
                })
                .map_or(rest.len(), |at| at + 1);
            rest[..end].to_vec()
        };
        for expected in [
            traced_line(&request_id, "HTTP Method: GET, Resource Path: /echo"),
            traced_line(&request_id, "Integration type: AWS_PROXY"),
            traced_line(&request_id, "Method completed with status: 200"),
        ] {
            assert!(
                trace.contains(&expected),
                "case {case:?}: expected the line {expected:?} in this request's trace, got \
                 {trace:?}"
            );
        }
        // The payload lines: the bytes of the call where data tracing is on, and their
        // sizes where it is off, which is the control that setting exists for.
        let served = r#"{"seen": "traced"}"#;
        let with_data = traced_line(
            &request_id,
            &format!("Method response body after transformations: {served}"),
        );
        let without_data = traced_line(
            &request_id,
            &format!(
                "Method response body after transformations: {} byte(s)",
                served.len()
            ),
        );
        let (present, absent) = if level.payloads {
            (with_data, without_data)
        } else {
            (without_data, with_data)
        };
        assert!(
            trace.contains(&present) && !trace.contains(&absent),
            "case {case:?}: expected {present:?} and not {absent:?} in this request's trace, \
             got {trace:?}"
        );
        // One request is one trace: the group holds as many opening lines as requests that
        // were traced. Anchored at the record's own start — every record opens with the
        // `(<request id>) ` naming the request it is about — so a record that merely embeds
        // the phrase is not counted as a trace of its own.
        let opened_count = records
            .iter()
            .filter(|record| {
                record
                    .split_once(") ")
                    .is_some_and(|(id, line)| {
                        id.starts_with('(') && line.starts_with("Execution log for request ")
                    })
            })
            .count();
        assert_eq!(
            opened_count, traced,
            "case {case:?}: expected one trace per traced request ({traced}), got \
             {opened_count} in {records:?}"
        );
    }
}
