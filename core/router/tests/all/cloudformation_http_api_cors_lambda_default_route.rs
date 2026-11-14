// SPDX-License-Identifier: CC0-1.0

//! What a client of a CloudFormation-deployed HTTP API's `$default` stage is served when the
//! `$default` route fronts an `AWS::Lambda::Function` through an `AWS_PROXY` integration.
//!
//! The rule under test: **an HTTP api's `CorsConfiguration` decides the CORS HEADERS on the
//! answer and nothing else about a request the route serves, so the same request answered
//! with and without an `Origin` header is answered by the same run of the same function —
//! while the PREFLIGHT a user agent sends instead of the request is answered by the GATEWAY
//! itself, from that configuration, with no run of the function at all.** CloudFormation provisions the function and the api, API Gateway v2
//! resolves the stage and the route and performs the proxy integration, and AWS Lambda runs
//! the deployment package — four crates, so the suite lives here rather than in any one of
//! them.
//!
//! The rows differ along what the rule generalises over: whether the request carries an
//! `Origin` at all, whether the configuration admits the one it carries, and — for the
//! preflight the user agent sends instead of the request — whether the configuration admits
//! the method it asks about. Every row asserts BOTH halves: the status and the body the
//! client was served, which is the handler's own answer for the request it was given, and
//! the CORS headers the api's configuration owed that answer.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::query_wire::{form_encode, outputs_of, physical_ids_of};

/// The Region every request below is signed for and every ARN names.
const REGION: &str = "us-east-1";

/// The `StackStatus` the setup waits for.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
const DELETE_COMPLETE: &str = "DELETE_COMPLETE";

/// The header a `restJson1` service names the error shape of a refusal in.
const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";

/// The one origin the deployed api's `CorsConfiguration` allows, and one it does not.
const ALLOWED_ORIGIN: &str = "https://example.test";
const OTHER_ORIGIN: &str = "https://denied.test";

/// The one method that configuration's `AllowMethods` allows, and one it does not.
const ALLOWED_METHOD: &str = "GET";
const OTHER_METHOD: &str = "DELETE";

/// The three response headers a CORS answer carries, as Fetch names them.
const ALLOW_ORIGIN_HEADER: &str = "access-control-allow-origin";
const ALLOW_METHODS_HEADER: &str = "access-control-allow-methods";

/// The status the GATEWAY answers a preflight of a CORS-configured api with: it answers the
/// preflight itself, so the route's integration is not reached and there is no body.
const PREFLIGHT_STATUS: u16 = 204;

/// How long one invocation of this handler is given, as the function's own `Timeout`.
const FUNCTION_TIMEOUT: u64 = 30;

/// How long to wait between polls of a clock-owed transition, and how many polls to make.
const POLL: Duration = Duration::from_millis(50);
const POLLS: usize = 400;

/// The handler the stack deploys, inline in the template exactly as a synthesised app
/// carries it: it echoes back the marker the request carried, the path it arrived on and the
/// method it used, which is what tells one run of it from another.
const HANDLER_SOURCE: &str = r#"
import json


def handler(event, context):
    params = event.get("queryStringParameters") or {}
    return {
        "statusCode": 200,
        "headers": {"Content-Type": "application/json"},
        "body": json.dumps(
            {
                "marker": params.get("marker"),
                "path": event.get("rawPath"),
                "method": event["requestContext"]["http"]["method"],
            }
        ),
    }
"#;

/// One answered request: its status, its headers and its body verbatim.
struct Answered {
    status: u16,
    headers: BTreeMap<String, String>,
    body: Bytes,
}

impl Answered {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }
}

/// The emulator as it is ASSEMBLED, on one engine: the stack's function and api are held by
/// the same router the invoke below reaches.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    // The suite exercises the services, not SigV4: a control-plane call carries a credential
    // scope so the router can name the target service, and the invoke of the published stage
    // URL carries none, as a client of one sends none.
    router.set_request_authentication(false);
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
        headers: parts
            .headers
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_ascii_lowercase(),
                    String::from_utf8_lossy(value.as_bytes()).to_string(),
                )
            })
            .collect(),
        body,
    }
}

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> Answered {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    dispatch(
        router,
        "POST",
        &format!("cloudformation.{REGION}.amazonaws.com"),
        "/",
        &[
            ("content-type", "application/x-www-form-urlencoded"),
            ("authorization", &authorization("cloudformation")),
        ],
        Bytes::from(body),
    )
    .await
}

/// One AWS Lambda call over its `restJson1` wire.
async fn lambda(router: &MultiServiceRouter, method: &str, path: &str) -> Answered {
    dispatch(
        router,
        method,
        &format!("lambda.{REGION}.amazonaws.com"),
        path,
        &[
            ("content-type", "application/json"),
            ("authorization", &authorization("lambda")),
        ],
        Bytes::new(),
    )
    .await
}

/// One API Gateway v2 control-plane call over its `restJson1` wire.
async fn apigatewayv2(router: &MultiServiceRouter, method: &str, path: &str) -> Answered {
    dispatch(
        router,
        method,
        &format!("apigateway.{REGION}.amazonaws.com"),
        path,
        &[
            ("content-type", "application/json"),
            ("authorization", &authorization("apigateway")),
        ],
        Bytes::new(),
    )
    .await
}

/// Poll `GetFunctionConfiguration` until Lambda first reports the function `Active`. The span
/// is that service's own, so nothing here sleeps a fixed span on its behalf.
async fn wait_until_active(router: &MultiServiceRouter, name: &str) {
    for _ in 0..POLLS {
        let reported = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{name}/configuration"),
        )
        .await;
        if reported.json().get("State").and_then(Value::as_str) == Some("Active") {
            return;
        }
        tokio::time::sleep(POLL).await;
    }
    panic!("setup: Lambda never reported {name} Active");
}

/// The template the suite deploys: the function with its code INLINE, the api with its CORS
/// configuration, the `AWS_PROXY` integration on that function, the `$default` route in front
/// of it, the auto-deployed `$default` stage, and the permission that admits the gateway's
/// invocation — the shape a synthesised HTTP-api app carries.
fn template() -> String {
    json!({
        "Resources": {
            "Role": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "AssumeRolePolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": { "Service": "lambda.amazonaws.com" },
                            "Action": "sts:AssumeRole"
                        }]
                    }
                }
            },
            "Fn": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    "Role": { "Fn::GetAtt": ["Role", "Arn"] },
                    "Timeout": FUNCTION_TIMEOUT,
                    "MemorySize": 128,
                    "Code": { "ZipFile": HANDLER_SOURCE }
                }
            },
            "Api": {
                "Type": "AWS::ApiGatewayV2::Api",
                "Properties": {
                    "Name": "cors-lambda-api",
                    "ProtocolType": "HTTP",
                    "CorsConfiguration": {
                        "AllowOrigins": [ALLOWED_ORIGIN],
                        "AllowMethods": [ALLOWED_METHOD]
                    }
                }
            },
            "Integration": {
                "Type": "AWS::ApiGatewayV2::Integration",
                "Properties": {
                    "ApiId": { "Ref": "Api" },
                    "IntegrationType": "AWS_PROXY",
                    "IntegrationUri": { "Fn::GetAtt": ["Fn", "Arn"] },
                    "PayloadFormatVersion": "2.0"
                }
            },
            "Route": {
                "Type": "AWS::ApiGatewayV2::Route",
                "Properties": {
                    "ApiId": { "Ref": "Api" },
                    "RouteKey": "$default",
                    "Target": {
                        "Fn::Join": ["/", ["integrations", { "Ref": "Integration" }]]
                    }
                }
            },
            "Stage": {
                "Type": "AWS::ApiGatewayV2::Stage",
                "Properties": {
                    "ApiId": { "Ref": "Api" },
                    "StageName": "$default",
                    "AutoDeploy": true
                }
            },
            "Permission": {
                "Type": "AWS::Lambda::Permission",
                "Properties": {
                    "Action": "lambda:InvokeFunction",
                    "FunctionName": { "Fn::GetAtt": ["Fn", "Arn"] },
                    "Principal": "apigateway.amazonaws.com"
                }
            }
        },
        "Outputs": {
            "ApiId": { "Value": { "Ref": "Api" } },
            "FunctionName": { "Value": { "Ref": "Fn" } }
        }
    })
    .to_string()
}

/// One row: the request made on the deployed stage, and the CORS headers its answer owes.
struct Row {
    case: &'static str,
    /// The HTTP method the client uses.
    method: &'static str,
    /// The `Origin` the request carries, or `None` for a request carrying none.
    origin: Option<&'static str>,
    /// The method a preflight asks about, for a row that sends one.
    asked_about: Option<&'static str>,
    /// `access-control-allow-origin` the answer must carry, or `None` for an answer that must
    /// carry none.
    allow_origin: Option<&'static str>,
    /// `access-control-allow-methods` the answer must carry, likewise.
    allow_methods: Option<&'static str>,
    /// Whether the answer is the INTEGRATION's — the handler's own 200 and body — rather than
    /// the gateway's own answer to a preflight it handled itself, which carries the
    /// configuration's headers on [`PREFLIGHT_STATUS`] and no body at all.
    served_by_the_function: bool,
}

impl Row {
    /// The status this row's answer must carry: the handler's own `200` for a request it
    /// served, and the gateway's own [`PREFLIGHT_STATUS`] for a preflight it answered itself.
    fn status(&self) -> u16 {
        if self.served_by_the_function {
            200
        } else {
            PREFLIGHT_STATUS
        }
    }
}

const ROWS: &[Row] = &[
    Row {
        case: "a plain GET, which carries no Origin at all",
        method: "GET",
        origin: None,
        asked_about: None,
        allow_origin: None,
        allow_methods: None,
        served_by_the_function: true,
    },
    Row {
        case: "a GET carrying an allowed Origin",
        method: "GET",
        origin: Some(ALLOWED_ORIGIN),
        asked_about: None,
        allow_origin: Some(ALLOWED_ORIGIN),
        allow_methods: None,
        served_by_the_function: true,
    },
    Row {
        case: "a GET carrying an Origin the configuration does not allow",
        method: "GET",
        origin: Some(OTHER_ORIGIN),
        asked_about: None,
        allow_origin: None,
        allow_methods: None,
        served_by_the_function: true,
    },
    Row {
        case: "a preflight from an allowed Origin, about an allowed method",
        method: "OPTIONS",
        origin: Some(ALLOWED_ORIGIN),
        asked_about: Some(ALLOWED_METHOD),
        allow_origin: Some(ALLOWED_ORIGIN),
        allow_methods: Some(ALLOWED_METHOD),
        served_by_the_function: false,
    },
    Row {
        case: "a preflight from an allowed Origin, about a method the configuration does not \
               allow",
        method: "OPTIONS",
        origin: Some(ALLOWED_ORIGIN),
        asked_about: Some(OTHER_METHOD),
        allow_origin: None,
        allow_methods: None,
        served_by_the_function: false,
    },
    Row {
        case: "a preflight from an Origin the configuration does not allow",
        method: "OPTIONS",
        origin: Some(OTHER_ORIGIN),
        asked_about: Some(ALLOWED_METHOD),
        allow_origin: None,
        allow_methods: None,
        served_by_the_function: false,
    },
];

// The emulator boots a multi-thread runtime (`#[tokio::main]`), and the seam that serves a
// cross-origin invoke performs the route's integration as a task of the runtime it is served
// on, so the suite drives the router on that same flavour.
#[tokio::test(flavor = "multi_thread")]
async fn a_deployed_http_api_serves_its_default_lambda_route_with_the_apis_cors_decoration() {
    let router = emulator();

    // 1. The stack: the function, the api, its integration, its route and its stage.
    let stack = "cdk-http-api-cors-lambda";
    let body = template();
    let created = cloudformation(
        &router,
        "CreateStack",
        &[
            ("StackName", stack),
            ("TemplateBody", &body),
            ("Capabilities.member.1", "CAPABILITY_IAM"),
        ],
    )
    .await;
    assert_eq!(
        created.status, 200,
        "setup: expected HTTP 200 from CreateStack, got {} with {:?}",
        created.status,
        created.text()
    );
    let (reached, described) = wait_for_stack_status(&router, stack, CREATE_COMPLETE).await;
    assert_eq!(
        reached, CREATE_COMPLETE,
        "setup: expected the stack to reach {CREATE_COMPLETE}, got {described}"
    );
    let outputs = outputs_of(&described);
    let api_id = outputs
        .get("ApiId")
        .cloned()
        .unwrap_or_else(|| panic!("setup: no ApiId output in {described}"));
    let function_name = outputs
        .get("FunctionName")
        .cloned()
        .unwrap_or_else(|| panic!("setup: no FunctionName output in {described}"));

    // 2. The function the stack published really exists in AWS Lambda: a read of it through
    // that service's own `GetFunction` answers, rather than reporting a function nothing
    // created.
    let read = lambda(
        &router,
        "GET",
        &format!("/2015-03-31/functions/{function_name}"),
    )
    .await;
    assert_eq!(
        read.status, 200,
        "setup: expected HTTP 200 from GetFunction on the stack's own {function_name:?}, got \
         {} with {:?}",
        read.status,
        read.text()
    );
    wait_until_active(&router, &function_name).await;

    // The resources the stack reports are the ones it provisioned, which is what names the
    // function the integration fronts.
    let resources =
        cloudformation(&router, "DescribeStackResources", &[("StackName", stack)]).await;
    let physical = physical_ids_of(&resources.text());
    assert_eq!(
        physical.get("Fn").map(String::as_str),
        Some(function_name.as_str()),
        "setup: expected the stack's Fn resource to be the published function, got {physical:?}"
    );

    // 3. The stage the stack deployed: exactly `$default`, auto-deployed.
    let stages = apigatewayv2(&router, "GET", &format!("/v2/apis/{api_id}/stages")).await;
    assert_eq!(
        stages.status, 200,
        "setup: expected HTTP 200 from GetStages, got {} with {:?}",
        stages.status,
        stages.text()
    );
    let listed: Vec<(String, bool)> = stages
        .json()
        .get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|stage| {
                    (
                        stage
                            .get("stageName")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        stage
                            .get("autoDeploy")
                            .and_then(Value::as_bool)
                            .unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        listed,
        vec![("$default".to_string(), true)],
        "setup: expected the stack to deploy exactly the auto-deployed $default stage, got \
         {listed:?}"
    );

    // 4. Every row, on the api's own endpoint, unsigned, as a client of a published stage
    // calls it.
    let host = format!("{api_id}.execute-api.{REGION}.amazonaws.com");
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let marker = format!("marker-{index}");
        let mut headers: Vec<(&str, &str)> = Vec::new();
        if let Some(origin) = row.origin {
            headers.push(("origin", origin));
        }
        if let Some(asked_about) = row.asked_about {
            headers.push(("access-control-request-method", asked_about));
        }
        let served = dispatch(
            &router,
            row.method,
            &host,
            &format!("/?marker={marker}"),
            &headers,
            Bytes::new(),
        )
        .await;
        assert_eq!(
            served.status,
            row.status(),
            "case {case:?}: expected HTTP {} from {} {host}/?marker={marker}, got {} with \
             {:?}",
            row.status(),
            row.method,
            served.status,
            served.text()
        );
        if row.served_by_the_function {
            let answered = served.json();
            let expected = json!({
                "marker": marker,
                "path": "/",
                "method": row.method,
            });
            assert_eq!(
                answered, expected,
                "case {case:?}: expected the handler's own answer {expected}, got {answered}"
            );
        }
        let allow_origin = served.header(ALLOW_ORIGIN_HEADER);
        assert_eq!(
            allow_origin, row.allow_origin,
            "case {case:?}: expected {ALLOW_ORIGIN_HEADER} {:?}, got {allow_origin:?} in {:?}",
            row.allow_origin, served.headers
        );
        let allow_methods = served.header(ALLOW_METHODS_HEADER);
        assert_eq!(
            allow_methods, row.allow_methods,
            "case {case:?}: expected {ALLOW_METHODS_HEADER} {:?}, got {allow_methods:?} in \
             {:?}",
            row.allow_methods, served.headers
        );
    }

    // 5. The stack is torn down, and the endpoint stops serving: an `execute-api` endpoint
    // that resolves to no api answers 404 `{"message": "Not Found"}`, whether the api behind
    // it was deleted or never existed at all.
    let deleted = cloudformation(&router, "DeleteStack", &[("StackName", stack)]).await;
    assert_eq!(
        deleted.status, 200,
        "setup: expected HTTP 200 from DeleteStack, got {} with {:?}",
        deleted.status,
        deleted.text()
    );
    let (reached, described) = wait_until_deleted(&router, stack).await;
    assert!(
        reached,
        "setup: expected the stack to be deleted, got {described}"
    );

    // The rows differ along WHY the endpoint resolves to no api, and along which spelling of
    // the endpoint the client was handed: the api the stack deleted, an id of the same shape
    // nothing ever created, and the emulator's own rewriting of either.
    let local = "localhost:9297";
    let gone: [(&str, String, String); 4] = [
        ("the api the stack deleted, host-style", host.clone(), String::new()),
        (
            "an api id nothing ever created, host-style",
            format!("aaaaaaaaaa.execute-api.{REGION}.amazonaws.com"),
            String::new(),
        ),
        (
            "the api the stack deleted, on the endpoint this emulator announces",
            local.to_string(),
            format!("/_aws/execute-api/{api_id}"),
        ),
        (
            "an api id nothing ever created, on the endpoint this emulator announces",
            local.to_string(),
            "/_aws/execute-api/aaaaaaaaaa".to_string(),
        ),
    ];
    for (case, gone_host, prefix) in gone {
        for path in ["/", "/anything"] {
            let addressed = format!("{prefix}{path}");
            let answered =
                dispatch(&router, "GET", &gone_host, &addressed, &[], Bytes::new()).await;
            assert_eq!(
                answered.status, 404,
                "case {case:?}: expected HTTP 404 from GET {gone_host}{addressed}, got {} \
                 with {:?}",
                answered.status,
                answered.text()
            );
            assert_eq!(
                answered.json(),
                json!({ "message": "Not Found" }),
                "case {case:?}: expected the gateway's own not-found body from GET \
                 {gone_host}{addressed}, got {:?}",
                answered.text()
            );
        }
    }

    // And the resources the stack held are gone from the services that held them: the api
    // from API Gateway v2, the function from AWS Lambda.
    let read_api = apigatewayv2(&router, "GET", &format!("/v2/apis/{api_id}")).await;
    assert_eq!(
        read_api.status, 404,
        "case \"GetApi on the deleted api\": expected HTTP 404, got {} with {:?}",
        read_api.status,
        read_api.text()
    );
    assert_eq!(
        read_api.header(ERROR_TYPE_HEADER).map(error_code),
        Some("NotFoundException".to_string()),
        "case \"GetApi on the deleted api\": expected NotFoundException, got {:?}",
        read_api.text()
    );
    let read_function = lambda(
        &router,
        "GET",
        &format!("/2015-03-31/functions/{function_name}"),
    )
    .await;
    assert_eq!(
        read_function.status, 404,
        "case \"GetFunction on the deleted function\": expected HTTP 404, got {} with {:?}",
        read_function.status,
        read_function.text()
    );
    assert_eq!(
        read_function
            .json()
            .get("__type")
            .and_then(Value::as_str)
            .map(error_code),
        Some("ResourceNotFoundException".to_string()),
        "case \"GetFunction on the deleted function\": expected ResourceNotFoundException, \
         got {:?}",
        read_function.text()
    );
}

/// Poll `DescribeStacks` until the stack is gone: either it reports [`DELETE_COMPLETE`], or
/// the name resolves to no stack at all, which is what a completed delete leaves a name
/// meaning. Answers whether it got there and the last answer it read.
async fn wait_until_deleted(router: &MultiServiceRouter, stack: &str) -> (bool, String) {
    let mut last = String::new();
    for _ in 0..POLLS {
        let described = cloudformation(router, "DescribeStacks", &[("StackName", stack)]).await;
        last = described.text();
        if described.status != 200 || last.contains(DELETE_COMPLETE) {
            return (true, last);
        }
        tokio::time::sleep(POLL).await;
    }
    (false, last)
}

/// An error code as a client reads it off the wire: the shape name, with any Smithy
/// namespace prefix the envelope carries dropped.
fn error_code(stated: &str) -> String {
    stated
        .rsplit(['#', ':'])
        .next()
        .unwrap_or(stated)
        .trim()
        .to_string()
}
