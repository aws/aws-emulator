// SPDX-License-Identifier: CC0-1.0

//! `TestInvokeMethod` against an `AWS_PROXY` method whose function's resource policy decides
//! the invocation.
//!
//! The rule under test, one level above the recorded case:
//!
//! > API Gateway invokes a proxy integration's function as its own service principal, so the
//! > FUNCTION's resource-based policy decides whether the invocation is made — and a
//! > SIMULATED invocation is decided by the same policy, against the method's ARN under the
//! > stage AWS reserves for a test invocation (`test-invoke-stage`). A grant that covers
//! > that ARN admits the simulation, which then reports the handler's own response; a
//! > function that admits API Gateway nothing for the method — including one whose grant
//! > covers only a DEPLOYED stage — is not invoked at all, and the simulation reports the
//! > api's `API_CONFIGURATION_ERROR` gateway response (`500` with
//! > `{"message":"Internal server error"}`) with the configuration error named in the
//! > execution log.
//!
//! Only a cross-service suite can pin this: the api, the method and the simulation are API
//! Gateway's, while the function and its policy are AWS Lambda's and the log of a run that
//! HAPPENED is CloudWatch Logs' — all three on ONE engine, as `core/router/src/lib.rs`
//! mounts them. The run's log is what tells an invocation that happened from one that did
//! not, which is the half of the rule a status alone cannot show.
//!
//! The rows differ along what the rule generalises over: the `SourceArn` the grant is
//! narrowed to — none at all, every stage of the api, the reserved test-invoke stage by
//! name, a DEPLOYED stage only, and another api's method — which is the axis the decision
//! turns on.

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for and every ARN names.
pub(crate) const REGION: &str = "us-east-1";

/// The resource path the method is put on, and the stage the api is deployed to — the stage
/// a grant narrowed to a DEPLOYED stage names.
pub(crate) const RESOURCE_PATH: &str = "echo";
pub(crate) const STAGE: &str = "prod";

/// The function the method is integrated with, the group its runs log to, and the role the
/// created function names.
pub(crate) const FUNCTION: &str = "simulated-invoke";
const FUNCTION_GROUP: &str = "/aws/lambda/simulated-invoke";
const ROLE: &str = "arn:aws:iam::000000000000:role/lambda-exec";
const HANDLER: &str = "lambda_function.lambda_handler";

/// How long this suite waits for another service's clock-owed transition (a created function
/// becoming `Active`), and how often it looks.
const WAIT: Duration = Duration::from_secs(20);
const LOOK: Duration = Duration::from_millis(100);

/// The marker the handler answers with, which is how a reported body is told from one the
/// api itself rendered.
pub(crate) const MARKER: &str = "the-handler-ran";

/// The stage segment AWS reserves for a SIMULATED invocation, which the `aws:SourceArn` of the
/// call `TestInvokeMethod` makes carries in place of a deployed stage's name
/// (`crate::service::invoke`'s `TEST_INVOKE_STAGE`, as `src/ops/test_invoke_method.rs` holds
/// it).
pub(crate) const TEST_INVOKE_STAGE: &str = "test-invoke-stage";

/// The record the Lambda runtime frames every run with, which is what tells a function that
/// RAN from one that was never invoked.
const START_RECORD: &str = "START RequestId: ";

/// The body API Gateway answers a method whose integration it could not perform with.
pub(crate) const INTERNAL_SERVER_ERROR: &str = "Internal server error";

/// The phrase API Gateway names this configuration error by in the execution log.
pub(crate) const INVALID_PERMISSIONS: &str = "Invalid permissions on Lambda function";

/// The handler behind the method: it answers the proxy response envelope carrying the
/// marker.
const HANDLER_SOURCE: &str = r#"
import json


def lambda_handler(event, context):
    return {
        "statusCode": 200,
        "headers": {"Content-Type": "application/json"},
        "body": json.dumps({"marker": "the-handler-ran"}),
    }
"#;

/// One answered request: its status and its body verbatim.
pub(crate) struct Answered {
    pub(crate) status: u16,
    pub(crate) body: Bytes,
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
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The suite exercises the services, not SigV4: the calls carry a credential scope so the
    // router can name the target service, and the signature itself is never checked here.
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
/// the request.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// Dispatch one request as it arrives on the wire.
pub(crate) async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    path: &str,
    signing_name: &str,
    extra: &[(&str, &str)],
    body: Bytes,
) -> Answered {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header("content-type", "application/json")
        .header("authorization", authorization(signing_name));
    for (name, value) in extra {
        builder = builder.header(*name, *value);
    }
    let response = router
        .dispatch(builder.body(body).expect("a well-formed request"))
        .await;
    let (parts, body) = response.into_parts();
    Answered {
        status: parts.status.as_u16(),
        body,
    }
}

/// One API Gateway control-plane call, answering its status and parsed body.
async fn apigw(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answered {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    dispatch(
        router,
        method,
        &format!("apigateway.{REGION}.amazonaws.com"),
        path,
        "apigateway",
        &[],
        Bytes::from(payload),
    )
    .await
}

/// One API Gateway control-plane call that has to be accepted with `expected`.
pub(crate) async fn accepted(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    expected: u16,
) -> Value {
    let answered = apigw(router, method, path, body).await;
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
pub(crate) async fn lambda(
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
        "lambda",
        &[],
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
        "logs",
        &[("x-amz-target", &format!("Logs_20140328.{operation}"))],
        Bytes::from(body.to_string()),
    )
    .await;
    (answered.status, answered.json())
}

/// How many RUNS of the function CloudWatch Logs holds: one `START RequestId:` record per
/// run, and none at all for a function that was never invoked (which may hold no group
/// either).
pub(crate) async fn runs_logged(router: &MultiServiceRouter) -> usize {
    let (status, body) = logs(
        router,
        "FilterLogEvents",
        json!({ "logGroupName": FUNCTION_GROUP }),
    )
    .await;
    if status != 200 {
        return 0;
    }
    body.get("events")
        .and_then(Value::as_array)
        .map(|events| {
            events
                .iter()
                .filter(|event| {
                    event
                        .get("message")
                        .and_then(Value::as_str)
                        .is_some_and(|message| message.starts_with(START_RECORD))
                })
                .count()
        })
        .unwrap_or_default()
}

/// A .zip deployment package holding `source` as the module the handler names.
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

/// The api, its one `GET` method proxying to the function, and a deployment released to the
/// stage. Answers the api id and the id of the resource the method sits on.
pub(crate) async fn wired(router: &MultiServiceRouter) -> (String, String) {
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

    let api = accepted(
        router,
        "CreateRestApi",
        "POST",
        "/restapis",
        Some(json!({ "name": "simulated-invoke-api" })),
        201,
    )
    .await;
    let api = member("CreateRestApi", &api, "id");
    let resources = accepted(
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
    let resource = accepted(
        router,
        "CreateResource",
        "POST",
        &format!("/restapis/{api}/resources/{root}"),
        Some(json!({ "pathPart": RESOURCE_PATH })),
        201,
    )
    .await;
    let resource = member("CreateResource", &resource, "id");
    accepted(
        router,
        "PutMethod",
        "PUT",
        &format!("/restapis/{api}/resources/{resource}/methods/GET"),
        Some(json!({ "authorizationType": "NONE" })),
        201,
    )
    .await;
    accepted(
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
    accepted(
        router,
        "CreateDeployment",
        "POST",
        &format!("/restapis/{api}/deployments"),
        Some(json!({ "stageName": STAGE })),
        201,
    )
    .await;
    wait_until_active(router).await;
    (api, resource)
}

/// Grant `apigateway.amazonaws.com` `lambda:InvokeFunction` on the function, narrowed to
/// `source_arn`, as the statement `statement_id` — a policy holds as many statements as the
/// grants made on it, each under an id of its own.
pub(crate) async fn grant(
    router: &MultiServiceRouter,
    case: &str,
    statement_id: &str,
    source_arn: &str,
) {
    let (status, granted) = lambda(
        router,
        "POST",
        &format!("/2015-03-31/functions/{FUNCTION}/policy"),
        Some(json!({
            "StatementId": statement_id,
            "Action": "lambda:InvokeFunction",
            "Principal": "apigateway.amazonaws.com",
            "SourceArn": source_arn,
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup AddPermission({case:?}, {statement_id:?}): expected HTTP 201, got {status} with \
         {granted}"
    );
}

/// One row: the `SourceArn` the function's grant is narrowed to, and whether the simulated
/// invocation is then made.
struct Row {
    case: &'static str,
    /// The resource half of the grant's `SourceArn`, under the api's own id — `None` for a
    /// function carrying no grant at all. `{api}` stands for the api under test and
    /// `{other}` for another api of the account.
    source: Option<&'static str>,
    /// Whether the function is invoked, which decides both the reported response and
    /// whether a run is logged.
    invoked: bool,
}

/// The rows: every way a grant covers — or fails to cover — a SIMULATED invocation of the
/// method.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no grant at all",
            source: None,
            invoked: false,
        },
        Row {
            case: "a grant covering every stage of the api",
            source: Some("{api}/*/*/echo"),
            invoked: true,
        },
        Row {
            case: "a grant naming the reserved test-invoke stage",
            source: Some("{api}/{test-invoke-stage}/GET/echo"),
            invoked: true,
        },
        Row {
            case: "a grant covering a deployed stage only",
            source: Some("{api}/prod/GET/echo"),
            invoked: false,
        },
        Row {
            case: "a grant covering another api's method",
            source: Some("{other}/*/*/echo"),
            invoked: false,
        },
    ]
}

/// A simulated invocation is admitted by the function's own resource policy, against the
/// method's ARN under the stage AWS reserves for a test invocation.
#[tokio::test]
async fn a_simulated_invocation_is_decided_by_the_functions_resource_policy() {
    for row in rows() {
        let case = row.case;
        let router = emulator();
        let (api, resource) = wired(&router).await;
        if let Some(source) = row.source {
            let source_arn = format!(
                "arn:aws:execute-api:{REGION}:{account}:{resource_half}",
                account = engine::emulator_account_id(),
                resource_half = source
                    .replace("{api}", &api)
                    .replace("{test-invoke-stage}", TEST_INVOKE_STAGE)
                    .replace("{other}", "0123456789"),
            );
            grant(&router, case, "apigateway-invoke", &source_arn).await;
        }

        let simulated = accepted(
            &router,
            "TestInvokeMethod",
            "POST",
            &format!("/restapis/{api}/resources/{resource}/methods/GET"),
            Some(json!({ "pathWithQueryString": "/echo" })),
            200,
        )
        .await;
        let status = simulated
            .get("status")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| panic!("case {case:?}: no numeric status in {simulated}"));
        let body = simulated
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let log = simulated
            .get("log")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        if row.invoked {
            assert_eq!(
                (status, serde_json::from_str::<Value>(&body).ok()),
                (200, Some(json!({ "marker": MARKER }))),
                "case {case:?}: expected the handler's own 200 and body, got {status} with \
                 {body:?}"
            );
            assert!(
                log.contains("Successfully completed execution"),
                "case {case:?}: expected the execution log to report the invocation, got \
                 {log:?}"
            );
        } else {
            assert_eq!(
                (status, serde_json::from_str::<Value>(&body).ok()),
                (500, Some(json!({ "message": INTERNAL_SERVER_ERROR }))),
                "case {case:?}: expected the api's configuration-error refusal, got {status} \
                 with {body:?}"
            );
            assert!(
                log.contains(INVALID_PERMISSIONS),
                "case {case:?}: expected the execution log to name the configuration error \
                 {INVALID_PERMISSIONS:?}, got {log:?}"
            );
        }

        // What a status alone cannot show: whether the function RAN. One run is one run's
        // log in its own group, and a function that was never invoked has none.
        let runs = runs_logged(&router).await;
        assert_eq!(
            runs,
            usize::from(row.invoked),
            "case {case:?}: expected {} run(s) of the function logged, got {runs}",
            usize::from(row.invoked)
        );
    }
}
