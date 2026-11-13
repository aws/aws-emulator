// SPDX-License-Identifier: CC0-1.0

//! `TestInvokeMethod` against an `AWS_PROXY` method whose `uri` names a Lambda function —
//! or a version or alias of one — that resolves to nothing.
//!
//! The rule under test, one level above the recorded case:
//!
//! > API Gateway resolves the function an `AWS_PROXY` integration's `uri` names, WITH
//! > whatever version or alias the spelling qualifies it by, and takes the invoke grant on
//! > what that resolves to before invoking anything. A spelling Lambda holds no record for
//! > has no resource policy for a grant to live in, so it admits API Gateway nothing exactly
//! > as a resolvable function carrying no grant does: the function is not invoked, the method
//! > is answered through the api's own `API_CONFIGURATION_ERROR` gateway response (`500` with
//! > `{"message":"Internal server error"}`), and the execution log names that configuration
//! > error with the ONE permissions sentence API Gateway words it with — an unresolvable
//! > qualifier is reported as a permissions error, never as a not-found one.
//!
//! Only a cross-service suite can pin this: the api, the method and the simulation are API
//! Gateway's, while the function, its versions, its aliases and its policy are AWS Lambda's,
//! and the log of a run that HAPPENED is CloudWatch Logs' — all three on ONE engine, as
//! `core/router/src/lib.rs` mounts them. The run's log is what tells an invocation that
//! happened from one that did not, which is the half of the rule a status alone cannot show.
//!
//! The rows differ along what the rule generalises over: whether the `uri` carries a
//! qualifier at all, whether that qualifier is an ALIAS or a published VERSION number,
//! whether the function ahead of it is spelled as a full ARN or as a bare name, whether the
//! part that resolves to nothing is the qualifier or the function itself, and — for the row
//! whose spelling resolves perfectly well — whether the policy it resolves to grants this
//! method anything.

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for and every ARN names.
const REGION: &str = "us-east-1";

/// The function every row's `uri` addresses, the group its runs log to, the role it names and
/// the module and attribute its package carries the handler as.
const FUNCTION: &str = "unresolvable-qualifier-handler";
const FUNCTION_GROUP: &str = "/aws/lambda/unresolvable-qualifier-handler";
const ROLE: &str = "arn:aws:iam::000000000000:role/lambda-exec";
const HANDLER: &str = "lambda_function.lambda_handler";

/// The alias whose invocation the function's policy GRANTS, and one made exactly like it
/// whose own policy grants nothing.
const GRANTED_ALIAS: &str = "live";
const UNGRANTED_ALIAS: &str = "staging";

/// An alias name no `CreateAlias` ever made, and a function name this account holds no record
/// of — the two halves of a spelling that can resolve to nothing.
const ABSENT_ALIAS: &str = "sim2real-missing-alias";
const ABSENT_FUNCTION: &str = "unresolvable-qualifier-absent";

/// A version number no `PublishVersion` minted here, asserted distinct from the one it did.
const ABSENT_VERSION: &str = "97";

/// How long this suite waits for another service's clock-owed transition (a created function
/// becoming `Active`), and how often it looks. Generous: nothing here is timed, the deadline
/// only bounds a wait that should never be reached.
const WAIT: Duration = Duration::from_secs(30);
const LOOK: Duration = Duration::from_millis(100);

/// The marker the handler answers with, which is how a reported body is told from one the api
/// itself rendered.
const MARKER: &str = "the-handler-ran";

/// The record the Lambda runtime frames every run with, which is what tells a function that
/// RAN from one that was never invoked.
const START_RECORD: &str = "START RequestId: ";

/// The body API Gateway answers a method whose integration it could not perform with.
const INTERNAL_SERVER_ERROR: &str = "Internal server error";

/// The sentence API Gateway names this configuration error by in the execution log.
const INVALID_PERMISSIONS: &str = "Invalid permissions on Lambda function";

/// The handler behind the method: it answers the proxy response envelope carrying the marker.
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
struct Answered {
    status: u16,
    body: Bytes,
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
async fn dispatch(
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
        "apigateway",
        &[],
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
        "lambda",
        &[],
        Bytes::from(payload),
    )
    .await;
    (answered.status, answered.json())
}

/// One AWS Lambda call that has to be accepted with `expected`.
async fn lambda_accepted(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    expected: u16,
) -> Value {
    let (status, answered) = lambda(router, method, path, body).await;
    assert_eq!(
        status, expected,
        "setup {case:?}: expected HTTP {expected} from {method} {path}, got {status} with \
         {answered}"
    );
    answered
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
async fn runs_logged(router: &MultiServiceRouter) -> usize {
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

/// Grant `apigateway.amazonaws.com` `lambda:InvokeFunction` on the function — on the
/// qualifier `qualifier` names, since "a resource policy is attached to the ARN the permission
/// names" — narrowed to every method of `api`.
async fn grant(router: &MultiServiceRouter, api: &str, qualifier: Option<&str>) {
    let query = qualifier
        .map(|qualifier| format!("?Qualifier={qualifier}"))
        .unwrap_or_default();
    lambda_accepted(
        router,
        &format!("AddPermission({qualifier:?})"),
        "POST",
        &format!("/2015-03-31/functions/{FUNCTION}/policy{query}"),
        Some(json!({
            "StatementId": format!("apigateway-invoke-{}", qualifier.unwrap_or("unqualified")),
            "Action": "lambda:InvokeFunction",
            "Principal": "apigateway.amazonaws.com",
            "SourceArn": format!(
                "arn:aws:execute-api:{REGION}:{}:{api}/*/*/*",
                engine::emulator_account_id()
            ),
        })),
        201,
    )
    .await;
}

/// One row: the function spelling the method's integration `uri` carries, and whether the
/// records that spelling resolves to let the invocation be made.
struct Row {
    case: &'static str,
    /// The resource path part the method sits on, which is also the simulated path.
    part: &'static str,
    /// The function (and qualifier) the integration's `uri` names.
    function: String,
    /// Whether the function is invoked, which decides both the reported response and whether
    /// a run is logged.
    invoked: bool,
}

#[tokio::test]
async fn a_uri_whose_function_resolves_to_nothing_is_a_permissions_configuration_error() {
    let router = emulator();

    // The api first: the grants below are narrowed to its own methods.
    let api = member(
        "CreateRestApi",
        &apigw(
            &router,
            "CreateRestApi",
            "POST",
            "/restapis",
            Some(json!({ "name": "unresolvable-qualifier-api" })),
            201,
        )
        .await,
        "id",
    );
    let root = apigw(
        &router,
        "GetResources",
        "GET",
        &format!("/restapis/{api}/resources"),
        None,
        200,
    )
    .await
    .get("item")
    .and_then(Value::as_array)
    .and_then(|items| items.first())
    .map(|item| member("GetResources", item, "id"))
    .expect("the api's root resource");

    // The function, one published version, and two aliases on it — one whose policy grants
    // this api an invocation and one whose policy grants nothing.
    let created = lambda_accepted(
        &router,
        "CreateFunction",
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
        201,
    )
    .await;
    let function_arn = member("CreateFunction", &created, "FunctionArn");
    wait_until_active(&router).await;
    let published = member(
        "PublishVersion",
        &lambda_accepted(
            &router,
            "PublishVersion",
            "POST",
            &format!("/2015-03-31/functions/{FUNCTION}/versions"),
            Some(json!({})),
            201,
        )
        .await,
        "Version",
    );
    assert_ne!(
        published, ABSENT_VERSION,
        "setup \"PublishVersion\": the row naming an unpublished version needs a label no \
         version carries, got {published:?}"
    );
    for alias in [GRANTED_ALIAS, UNGRANTED_ALIAS] {
        lambda_accepted(
            &router,
            &format!("CreateAlias({alias})"),
            "POST",
            &format!("/2015-03-31/functions/{FUNCTION}/aliases"),
            Some(json!({ "Name": alias, "FunctionVersion": published })),
            201,
        )
        .await;
    }
    grant(&router, &api, None).await;
    grant(&router, &api, Some(GRANTED_ALIAS)).await;
    grant(&router, &api, Some(published.as_str())).await;

    let rows = [
        Row {
            case: "a uri carrying no qualifier at all, on a function whose policy grants this \
                   api the invocation",
            part: "unqualified",
            function: function_arn.clone(),
            invoked: true,
        },
        Row {
            case: "a uri qualified by an alias CreateAlias made, whose own policy grants it",
            part: "granted-alias",
            function: format!("{function_arn}:{GRANTED_ALIAS}"),
            invoked: true,
        },
        Row {
            case: "a uri qualified by a published version number, whose own policy grants it",
            part: "published-version",
            function: format!("{function_arn}:{published}"),
            invoked: true,
        },
        Row {
            case: "a uri qualified by an alias no CreateAlias made",
            part: "absent-alias",
            function: format!("{function_arn}:{ABSENT_ALIAS}"),
            invoked: false,
        },
        Row {
            case: "a uri naming the function by name alone, qualified by an alias no \
                   CreateAlias made",
            part: "name-and-absent-alias",
            function: format!("{FUNCTION}:{ABSENT_ALIAS}"),
            invoked: false,
        },
        Row {
            case: "a uri qualified by a version number no PublishVersion minted",
            part: "absent-version",
            function: format!("{function_arn}:{ABSENT_VERSION}"),
            invoked: false,
        },
        Row {
            case: "a uri naming a function this account holds no record of",
            part: "absent-function",
            function: ABSENT_FUNCTION.to_string(),
            invoked: false,
        },
        Row {
            case: "a uri qualified by an alias that resolves, whose own policy grants this \
                   method nothing",
            part: "ungranted-alias",
            function: format!("{function_arn}:{UNGRANTED_ALIAS}"),
            invoked: false,
        },
    ];

    for row in &rows {
        let case = row.case;
        let resource = member(
            "CreateResource",
            &apigw(
                &router,
                &format!("CreateResource({})", row.part),
                "POST",
                &format!("/restapis/{api}/resources/{root}"),
                Some(json!({ "pathPart": row.part })),
                201,
            )
            .await,
            "id",
        );
        apigw(
            &router,
            &format!("PutMethod({})", row.part),
            "PUT",
            &format!("/restapis/{api}/resources/{resource}/methods/GET"),
            Some(json!({ "authorizationType": "NONE" })),
            201,
        )
        .await;
        apigw(
            &router,
            &format!("PutIntegration({})", row.part),
            "PUT",
            &format!("/restapis/{api}/resources/{resource}/methods/GET/integration"),
            Some(json!({
                "type": "AWS_PROXY",
                "httpMethod": "POST",
                "uri": format!(
                    "arn:aws:apigateway:{REGION}:lambda:path/2015-03-31/functions/\
                     {}/invocations",
                    row.function
                ),
            })),
            201,
        )
        .await;

        let before = runs_logged(&router).await;
        let simulated = apigw(
            &router,
            &format!("TestInvokeMethod({})", row.part),
            "POST",
            &format!("/restapis/{api}/resources/{resource}/methods/GET"),
            Some(json!({ "pathWithQueryString": format!("/{}", row.part) })),
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
                "case {case:?} (uri naming {:?}): expected the handler's own 200 and body, got \
                 {status} with {body:?}",
                row.function
            );
            assert!(
                log.contains("Successfully completed execution"),
                "case {case:?} (uri naming {:?}): expected the execution log to report the \
                 invocation, got {log:?}",
                row.function
            );
        } else {
            assert_eq!(
                (status, serde_json::from_str::<Value>(&body).ok()),
                (500, Some(json!({ "message": INTERNAL_SERVER_ERROR }))),
                "case {case:?} (uri naming {:?}): expected the api's configuration-error \
                 refusal, got {status} with {body:?}",
                row.function
            );
            assert!(
                log.contains(&format!(
                    "Execution failed due to configuration error: {INVALID_PERMISSIONS}"
                )),
                "case {case:?} (uri naming {:?}): expected the execution log to word the \
                 configuration error {INVALID_PERMISSIONS:?}, got {log:?}",
                row.function
            );
        }

        // What a status alone cannot show: whether the function RAN. One run is one run's
        // own `START` record in the function's group, and a spelling that resolves to nothing
        // leaves none behind.
        let after = runs_logged(&router).await;
        assert_eq!(
            after.saturating_sub(before),
            usize::from(row.invoked),
            "case {case:?} (uri naming {:?}): expected {} further run(s) of the function \
             logged, got {} (from {before} to {after})",
            row.function,
            usize::from(row.invoked),
            after.saturating_sub(before)
        );
    }
}
