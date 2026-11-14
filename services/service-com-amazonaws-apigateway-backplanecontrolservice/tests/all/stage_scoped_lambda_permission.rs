// SPDX-License-Identifier: CC0-1.0

//! Which STAGE of a deployed REST api may invoke its proxy integration's function, and what
//! the stage's execution log says about a request whose invocation the function refuses.
//!
//! The rule under test, one level above the recorded case:
//!
//! > API Gateway invokes a proxy integration's function as its own service principal, so the
//! > FUNCTION's resource policy decides each request — judged against the ARN of the METHOD
//! > the request reached, which names the stage. Two stages of ONE deployment are therefore
//! > decided separately: a grant narrowed to one stage admits that stage's requests and no
//! > other's. A request the policy does not admit is answered through the api's
//! > `API_CONFIGURATION_ERROR` gateway response (`500` with
//! > `{"message":"Internal server error"}`) with the function never invoked, and the stage's
//! > execution log records the step that stopped it — the configuration error and the gateway
//! > response the request was answered through — in records naming the request they are about.
//!
//! Only a cross-service suite can pin this: the api, its two stages and the execution log's
//! emission are API Gateway's, the function and its resource policy are AWS Lambda's, and
//! both the execution log and the log of a run that HAPPENED are CloudWatch Logs' — all three
//! on ONE engine, as `core/router/src/lib.rs` mounts them. Every assertion reads the outcome
//! back through CloudWatch Logs' own reader (`FilterLogEvents`), so a line that did not reach
//! that store cannot pass, and what the function's own group holds is what tells an
//! invocation that happened from one that did not.
//!
//! The rows differ along what the rule generalises over: the resource half of each grant's
//! `SourceArn` — no grant at all, one named stage, the other named stage, one grant for each
//! of them, every stage of the api, and a path the request does not address — each driven
//! through BOTH stages of the same deployment.

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::stage_access_logging::traced_line;

/// The Region every request below is signed for and every ARN names.
const REGION: &str = "us-east-1";

/// The two stages ONE deployment is released to, and the resource path of its one method.
const FIRST_STAGE: &str = "allowed";
const SECOND_STAGE: &str = "denied";
const RESOURCE_PATH: &str = "echo";

/// The verb the method is put on: `ANY`, the catch-all a method serves every verb through,
/// which is what the request below is served by.
const METHOD_VERB: &str = "ANY";

/// The function the method is integrated with, the group its runs log to, and the role the
/// created function names. No IAM store is mounted here, so the role is a value the create
/// carries rather than a record this suite relies on.
const FUNCTION: &str = "stage-scoped-proxy";
const FUNCTION_GROUP: &str = "/aws/lambda/stage-scoped-proxy";
const ROLE: &str = "arn:aws:iam::000000000000:role/lambda-exec";
const HANDLER: &str = "lambda_function.lambda_handler";

/// The account-level role API Gateway writes the account's logs as
/// (`Account$cloudwatchRoleArn`), which is what an execution log needs before a stage may
/// deliver one.
const CLOUDWATCH_ROLE: &str = "arn:aws:iam::000000000000:role/apigw-cloudwatch";

/// The `loggingLevel` both stages carry: `ERROR`, which traces exactly the requests the stage
/// answered with an error — the half of the differential under test.
const LOGGING_LEVEL: &str = "ERROR";

/// The body API Gateway answers a method whose integration it could not perform with.
const INTERNAL_SERVER_ERROR: &str = "Internal server error";

/// The gateway response an api answers such a request through, and the sentence API Gateway
/// names this configuration error by.
const CONFIGURATION_ERROR_TYPE: &str = "API_CONFIGURATION_ERROR";
const INVALID_PERMISSIONS: &str = "Invalid permissions on Lambda function";

/// The header API Gateway reports the id of the request it processed in, which is also what
/// every record of that request's execution log names it by.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// The record the Lambda runtime frames every run with, which is what tells a function that
/// RAN from one that was never invoked.
const START_RECORD: &str = "START RequestId: ";

/// The handler behind the method: it prints the marker the request carried and answers the
/// proxy response envelope naming it, so one request's own run is identifiable in the
/// function's log group.
const HANDLER_SOURCE: &str = r#"
import json


def lambda_handler(event, context):
    marker = (event.get("queryStringParameters") or {}).get("marker") or "none"
    print("MARKER:" + marker)
    return {
        "statusCode": 200,
        "headers": {"Content-Type": "application/json"},
        "body": json.dumps({"marker": marker}),
    }
"#;

/// One answered request: its status, its body verbatim and the id it was reported under.
struct Answered {
    status: u16,
    body: Bytes,
    request_id: Option<String>,
}

impl Answered {
    /// The body as text.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    /// The body as JSON, `Null` when it is not JSON at all.
    fn json(&self) -> Value {
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
/// `{rest-api-id}.execute-api.{region}.amazonaws.com` — through `stage`, carrying the marker
/// this request is identified by and no credential at all, as a client of a public method
/// sends none.
async fn invoke(router: &MultiServiceRouter, api: &str, stage: &str, marker: &str) -> Answered {
    dispatch(
        router,
        "GET",
        &format!("{api}.execute-api.{REGION}.amazonaws.com"),
        &format!("/{stage}/{RESOURCE_PATH}?marker={marker}"),
        &[("content-type", "application/json")],
        Bytes::new(),
    )
    .await
}

/// The messages `FilterLogEvents` reports for `group`, or `None` when the account holds no
/// such group at all.
async fn delivered(router: &MultiServiceRouter, group: &str) -> Option<Vec<String>> {
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

/// What the FUNCTION's own log group holds: how many runs it frames, and whether the marker
/// of this request's own run is among the lines.
async fn runs_logged(router: &MultiServiceRouter, marker: &str) -> (usize, bool) {
    let messages = delivered(router, FUNCTION_GROUP).await.unwrap_or_default();
    let runs = messages
        .iter()
        .filter(|message| message.starts_with(START_RECORD))
        .count();
    let printed = format!("MARKER:{marker}");
    (runs, messages.contains(&printed))
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

/// How long this suite waits for another service's clock-owed transition (a created function
/// becoming `Active`), and how often it looks.
const WAIT: Duration = Duration::from_secs(20);
const LOOK: Duration = Duration::from_millis(100);

/// Poll `GetFunctionConfiguration` until Lambda first reports the function `Active`.
///
/// The span a created function spends `Pending` is that service's own, so nothing here
/// sleeps a fixed span on its behalf.
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

/// The whole setup: the function, the api with one `ANY` method proxying to it, ONE
/// deployment released to both stages, the account's CloudWatch role and the method setting
/// that turns each stage's execution log on. Answers the api id.
async fn wired(router: &MultiServiceRouter) -> String {
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
        Some(json!({ "name": "stage-scoped-permission-api" })),
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
        &format!("/restapis/{api}/resources/{resource}/methods/{METHOD_VERB}"),
        Some(json!({ "authorizationType": "NONE" })),
        201,
    )
    .await;
    apigw(
        router,
        "PutIntegration",
        "PUT",
        &format!("/restapis/{api}/resources/{resource}/methods/{METHOD_VERB}/integration"),
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

    // The account's CloudWatch role, which API Gateway writes the account's logs as — and
    // which `UpdateStage` requires before a method setting may turn execution logging on.
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

    // ONE deployment, released to BOTH stages: the stages differ in nothing but their name,
    // which is what makes the per-stage decision the only thing under test.
    let deployment = apigw(
        router,
        "CreateDeployment",
        "POST",
        &format!("/restapis/{api}/deployments"),
        Some(json!({ "stageName": FIRST_STAGE })),
        201,
    )
    .await;
    let deployment = member("CreateDeployment", &deployment, "id");
    apigw(
        router,
        "CreateStage",
        "POST",
        &format!("/restapis/{api}/stages"),
        Some(json!({ "stageName": SECOND_STAGE, "deploymentId": deployment })),
        201,
    )
    .await;
    for stage in [FIRST_STAGE, SECOND_STAGE] {
        apigw(
            router,
            "UpdateStage methodSettings",
            "PATCH",
            &format!("/restapis/{api}/stages/{stage}"),
            Some(json!({
                "patchOperations": [
                    { "op": "replace", "path": "/*/*/logging/loglevel", "value": LOGGING_LEVEL },
                ],
            })),
            200,
        )
        .await;
    }
    wait_until_active(router).await;
    api
}

/// Grant `apigateway.amazonaws.com` `lambda:InvokeFunction` on the function, narrowed to
/// `source_arn`, under the statement id `statement`.
async fn grant(router: &MultiServiceRouter, case: &str, statement: &str, source_arn: &str) {
    let (status, granted) = lambda(
        router,
        "POST",
        &format!("/2015-03-31/functions/{FUNCTION}/policy"),
        Some(json!({
            "StatementId": statement,
            "Action": "lambda:InvokeFunction",
            "Principal": "apigateway.amazonaws.com",
            "SourceArn": source_arn,
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup AddPermission({case:?}): expected HTTP 201, got {status} with {granted}"
    );
}

/// Take that statement off the function again, so the next row starts from a function that
/// admits API Gateway nothing.
async fn revoke(router: &MultiServiceRouter, case: &str, statement: &str) {
    let (status, removed) = lambda(
        router,
        "DELETE",
        &format!("/2015-03-31/functions/{FUNCTION}/policy/{statement}"),
        None,
    )
    .await;
    assert_eq!(
        status, 204,
        "setup RemovePermission({case:?}): expected HTTP 204, got {status} with {removed}"
    );
}

/// The records of ONE request's execution log: those naming `request_id`, which is what
/// separates this request's trace from the traces of the requests before it in the group.
fn trace_of(records: &[String], request_id: &str) -> Vec<String> {
    let prefix = traced_line(request_id, "");
    records
        .iter()
        .filter(|record| record.starts_with(&prefix))
        .cloned()
        .collect()
}

/// One row: the resource halves of the grants the function carries, and which of the two
/// stages they admit.
struct Row {
    case: &'static str,
    /// `{api}` stands for the api under test, `{first}` and `{second}` for its two stages.
    sources: &'static [&'static str],
    /// Whether a request through [`FIRST_STAGE`] is invoked.
    first_invoked: bool,
    /// Whether a request through [`SECOND_STAGE`] is invoked.
    second_invoked: bool,
}

/// The rows: every way the function's grants cover — or fail to cover — the method of each
/// stage.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no grant at all",
            sources: &[],
            first_invoked: false,
            second_invoked: false,
        },
        Row {
            case: "a grant naming the first stage only",
            sources: &["{api}/{first}/*/*"],
            first_invoked: true,
            second_invoked: false,
        },
        Row {
            case: "a grant naming the second stage only",
            sources: &["{api}/{second}/*/*"],
            first_invoked: false,
            second_invoked: true,
        },
        Row {
            case: "one grant for each stage",
            sources: &["{api}/{first}/*/*", "{api}/{second}/*/*"],
            first_invoked: true,
            second_invoked: true,
        },
        Row {
            case: "a grant covering every stage of the api",
            sources: &["{api}/*/*/echo"],
            first_invoked: true,
            second_invoked: true,
        },
        Row {
            case: "a grant naming a path the request does not address",
            sources: &["{api}/*/*/other"],
            first_invoked: false,
            second_invoked: false,
        },
    ]
}

/// Each stage of one deployment is admitted by the function's own resource policy against
/// the ARN of the method THAT stage's request reached, and a request it does not admit is
/// answered through the api's configuration-error gateway response with the refusal recorded
/// in that stage's execution log.
#[tokio::test]
async fn each_stage_is_admitted_by_the_functions_policy_against_its_own_method_arn() {
    let router = emulator();
    let api = wired(&router).await;

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        for (statement, source) in row.sources.iter().enumerate() {
            let source_arn = format!(
                "arn:aws:execute-api:{REGION}:{account}:{resource_half}",
                account = engine::emulator_account_id(),
                resource_half = source
                    .replace("{api}", &api)
                    .replace("{first}", FIRST_STAGE)
                    .replace("{second}", SECOND_STAGE),
            );
            grant(
                &router,
                case,
                &format!("apigateway-invoke-{statement}"),
                &source_arn,
            )
            .await;
        }

        for (stage, invoked) in [
            (FIRST_STAGE, row.first_invoked),
            (SECOND_STAGE, row.second_invoked),
        ] {
            let case = &format!("{case} through {stage:?}");
            // This request's own marker, which is how its run — if it was made at all — is
            // found among the runs the function's group already holds.
            let marker = format!("m{index}-{stage}");
            let (runs_before, _) = runs_logged(&router, &marker).await;
            let answered = invoke(&router, &api, stage, &marker).await;
            let request_id = answered
                .request_id
                .clone()
                .unwrap_or_else(|| panic!("case {case:?}: the response named no request id"));
            let expected = if invoked {
                (200, json!({ "marker": marker }))
            } else {
                (500, json!({ "message": INTERNAL_SERVER_ERROR }))
            };
            assert_eq!(
                (answered.status, answered.json()),
                expected,
                "case {case:?}: expected {expected:?}, got {} with {:?}",
                answered.status,
                answered.text()
            );

            // What a status alone cannot show: whether the function RAN. A request served by
            // an invocation is one run's log, carrying that request's marker, in the
            // function's own group; a request the policy refused leaves none.
            let (runs_after, marked) = runs_logged(&router, &marker).await;
            assert_eq!(
                (runs_after - runs_before, marked),
                (usize::from(invoked), invoked),
                "case {case:?}: expected {} run(s) of the function logged by this request and \
                 its marker present={invoked}, got {} and {marked}",
                usize::from(invoked),
                runs_after - runs_before
            );

            // And what the STAGE logged about it, read back through CloudWatch Logs' own
            // reader: the records of this request in the group API Gateway publishes for this
            // stage, each naming the request they are about. At `ERROR` the stage traces the
            // requests it answered with an error, which is the refused leg of this
            // differential; the served leg leaves no trace of its own, and so names no
            // configuration error either.
            let group = format!("API-Gateway-Execution-Logs_{api}/{stage}");
            let records = delivered(&router, &group)
                .await
                .unwrap_or_else(|| panic!("case {case:?}: expected the group {group:?} to exist"));
            let trace = trace_of(&records, &request_id);
            let refused = traced_line(
                &request_id,
                &format!("Execution failed due to configuration error: {INVALID_PERMISSIONS}"),
            );
            let answered_through = traced_line(
                &request_id,
                &format!("Answered through this API's {CONFIGURATION_ERROR_TYPE} gateway response"),
            );
            let status_line = traced_line(
                &request_id,
                &format!("Method completed with status: {}", expected.0),
            );
            if invoked {
                assert!(
                    !trace.contains(&refused) && !trace.contains(&answered_through),
                    "case {case:?}: expected no configuration error recorded for a request the \
                     function served, got {trace:?}"
                );
            } else {
                for line in [&status_line, &refused, &answered_through] {
                    assert!(
                        trace.contains(line),
                        "case {case:?}: expected the record {line:?} in this request's trace, \
                         got {trace:?}"
                    );
                }
            }
        }

        for statement in 0..row.sources.len() {
            revoke(&router, case, &format!("apigateway-invoke-{statement}")).await;
        }
    }
}
