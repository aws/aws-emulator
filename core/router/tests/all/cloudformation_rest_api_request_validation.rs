// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares a REST method's REQUEST VALIDATOR really creates that
//! validator — and the model its payload check reads — in Amazon API Gateway, so the deployed
//! stage refuses a call the method's declarations do not admit BEFORE the integration behind
//! it runs.
//!
//! The rule under test: **a method's request validation is enforced by the records API Gateway
//! holds for it, so CloudFormation creates the `AWS::ApiGateway::RequestValidator` and the
//! `AWS::ApiGateway::Model` a template declares in that service through its own public API and
//! publishes the identity that service assigned — and the deployed stage then refuses a request
//! that does not carry what the method declares required with API Gateway's request-validation
//! refusal, without invoking the integration, while a request that does carry it is served by
//! that integration.** Four crates have to agree for that to hold (CloudFormation makes the
//! calls, API Gateway holds the validator and then decides the invoke, Lambda runs the function
//! behind the integration and must NOT run for a refused call, IAM answers for the function's
//! role, CloudWatch Logs holds the records that say which calls really reached it), so the
//! suite lives here rather than in any one of them, and it drives the emulator's own assembled
//! service set.
//!
//! The stack is the shape a Serverless-Framework deploy of an `http` event produces: plain
//! `AWS::ApiGateway::*` resources, one validator per method, a Lambda proxy integration behind
//! each method, and one deployment and stage publishing them.
//!
//! The rows differ along what the rule generalises over: which HALF of the request the
//! validator declares checked (parameters, payload, neither), where the required parameter is
//! read from (the query string and a header), how MANY required parameters the call omits (one
//! and two), and whether the parameter is declared required at all.
//!
//! Every expected value is computed here from what the row declares — the api id, the validator
//! ids and the invoke URL come out of the stack's own outputs and resources, which is what a
//! client deploying the stack reads — never out of the answer being checked.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::query_wire::{elements, form_encode, outputs_of, physical_ids_of};

/// The Region every request here is signed for, and so the Region the stack's resources
/// belong to.
const REGION: &str = "us-east-1";

/// The `StackStatus` the rule is stated about.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The stage the api is deployed to, which is the stage a Serverless-Framework deploy
/// publishes by default.
const STAGE: &str = "dev";

/// How long one invocation of this handler is given, as the function's own `Timeout`.
const FUNCTION_TIMEOUT: u64 = 30;

/// How long to wait between polls of a clock-owed transition, and how many polls to make.
const POLL: Duration = Duration::from_millis(50);
const POLLS: usize = 400;

/// The content type the methods map their model to, and the media type every request here
/// carries.
const JSON: &str = "application/json";

/// The line the handler prints for every run, so the records in the function's log group say
/// exactly which calls reached the integration.
const INVOKED: &str = "INVOKED";

/// The handler behind every method: it prints one line naming the path it was reached on and
/// answers the proxy envelope, echoing the marker the call carried.
const HANDLER_SOURCE: &str = r#"
import json


def handler(event, context):
    print("INVOKED %s" % event.get("path"))
    params = event.get("queryStringParameters") or {}
    headers = event.get("headers") or {}
    posted = {}
    if event.get("body"):
        try:
            posted = json.loads(event["body"])
        except ValueError:
            posted = {}
    probe = ""
    for name, value in headers.items():
        # An HTTP field name is case-insensitive, so the probe is read that way.
        if name.lower() == "x-probe":
            probe = value
    marker = params.get("marker") or probe or posted.get("marker") or ""
    return {
        "statusCode": 200,
        "headers": {"content-type": "application/json"},
        "body": json.dumps({"echo": marker, "path": event.get("path")}),
    }
"#;

/// One answered request: its status and its body verbatim.
struct Answered {
    status: u16,
    body: Bytes,
}

impl Answered {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

/// The emulator as it is ASSEMBLED, on one engine (`aws_emulator_router::build_router`): the
/// validator CloudFormation creates is the validator that decides the invoke, and the function
/// behind the integration is the one AWS Lambda runs.
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

/// Read one API Gateway control-plane resource over its restJson1 wire.
async fn apigateway(router: &MultiServiceRouter, path: &str) -> Answered {
    dispatch(
        router,
        "GET",
        &format!("apigateway.{REGION}.amazonaws.com"),
        path,
        &[
            ("content-type", JSON),
            ("authorization", &authorization("apigateway")),
        ],
        Bytes::new(),
    )
    .await
}

/// One AWS Lambda call over its `restJson1` wire.
async fn lambda(router: &MultiServiceRouter, path: &str) -> Answered {
    dispatch(
        router,
        "GET",
        &format!("lambda.{REGION}.amazonaws.com"),
        path,
        &[
            ("content-type", JSON),
            ("authorization", &authorization("lambda")),
        ],
        Bytes::new(),
    )
    .await
}

/// One Amazon CloudWatch Logs call over its `awsJson1_1` wire.
async fn logs(router: &MultiServiceRouter, operation: &str, body: Value) -> Answered {
    dispatch(
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
    .await
}

/// Invoke the deployed stage at the invoke URL the stack published: the request's host is the
/// api's own `{rest-api-id}.execute-api.{region}.amazonaws.com` endpoint and its path is the
/// stage and the resource path, with the query string the row sends.
async fn invoke(
    router: &MultiServiceRouter,
    invoke_url: &str,
    verb: &str,
    suffix: &str,
    query: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Answered {
    let without_scheme = invoke_url
        .strip_prefix("https://")
        .or_else(|| invoke_url.strip_prefix("http://"))
        .unwrap_or(invoke_url);
    let (host, base) = match without_scheme.split_once('/') {
        Some((host, path)) => (host.to_string(), format!("/{path}")),
        None => (without_scheme.to_string(), String::new()),
    };
    let mut sent: Vec<(&str, &str)> = vec![("content-type", JSON)];
    sent.extend_from_slice(headers);
    dispatch(
        router,
        verb,
        &host,
        &format!("{base}{suffix}{query}"),
        &sent,
        Bytes::from(body.to_owned()),
    )
    .await
}

/// Poll `GetFunctionConfiguration` until Lambda first reports the function `Active`, and answer
/// the configuration it reported then. The span is that service's own, so nothing here sleeps a
/// fixed span on its behalf.
async fn wait_until_active(router: &MultiServiceRouter, name: &str) -> Value {
    for _ in 0..POLLS {
        let reported = lambda(router, &format!("/2015-03-31/functions/{name}/configuration")).await;
        let configuration = reported.json();
        if configuration.get("State").and_then(Value::as_str) == Some("Active") {
            return configuration;
        }
        tokio::time::sleep(POLL).await;
    }
    panic!("setup: Lambda never reported {name} Active");
}

/// Every message CloudWatch Logs holds in `group`, polled until it holds at least `at_least` of
/// them or the polls run out. The delivery is the logging service's own, so the wait is for the
/// RECORDS rather than for a span.
async fn messages_in(router: &MultiServiceRouter, group: &str, at_least: usize) -> Vec<String> {
    let mut held: Vec<String> = Vec::new();
    for _ in 0..POLLS {
        let answered = logs(router, "FilterLogEvents", json!({ "logGroupName": group })).await;
        if answered.status == 200 {
            held = answered
                .json()
                .get("events")
                .and_then(Value::as_array)
                .map(|events| {
                    events
                        .iter()
                        .filter_map(|event| event.get("message").and_then(Value::as_str))
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            if held.iter().filter(|line| line.contains(INVOKED)).count() >= at_least {
                return held;
            }
        }
        tokio::time::sleep(POLL).await;
    }
    held
}

/// One request a row sends to the deployed stage.
struct Call {
    /// The query string, as a client appends one (empty for none).
    query: &'static str,
    /// The headers the call carries beside its content type.
    headers: &'static [(&'static str, &'static str)],
    /// The payload the call carries.
    body: &'static str,
}

/// One row: a method of the deployed api, the validator declared for it, and the two calls that
/// say what that validator does — the one it must refuse (where it declares a check at all) and
/// the one it must admit.
struct Row {
    case: &'static str,
    /// The path part of the resource the method is on, which is also its path under the stage.
    path_part: &'static str,
    /// The verb the method answers.
    verb: &'static str,
    /// What the validator declares checked.
    validate_parameters: bool,
    validate_body: bool,
    /// The request parameters the method declares, as `(mapping expression, required)`.
    parameters: &'static [(&'static str, bool)],
    /// Whether the method maps a model for [`JSON`], whose schema requires `marker`.
    model: bool,
    /// The call that must be refused, with the `message` API Gateway's refusal carries, or
    /// `None` for a row whose validator declares no check the call could fail.
    refused: Option<(Call, &'static str)>,
    /// The call that must be served by the integration, and the `echo` the handler answers it
    /// with.
    served: (Call, &'static str),
}

const ROWS: &[Row] = &[
    Row {
        case: "a required query string the call omits",
        path_part: "echo",
        verb: "GET",
        validate_parameters: true,
        validate_body: false,
        parameters: &[("method.request.querystring.marker", true)],
        model: false,
        refused: Some((
            Call {
                query: "",
                headers: &[],
                body: "",
            },
            "Missing required request parameters: [marker]",
        )),
        served: (
            Call {
                query: "?marker=alpha",
                headers: &[],
                body: "",
            },
            "alpha",
        ),
    },
    Row {
        case: "a required header the call omits",
        path_part: "probe",
        verb: "GET",
        validate_parameters: true,
        validate_body: false,
        parameters: &[("method.request.header.x-probe", true)],
        model: false,
        refused: Some((
            Call {
                query: "",
                headers: &[],
                body: "",
            },
            "Missing required request parameters: [x-probe]",
        )),
        served: (
            Call {
                query: "",
                headers: &[("x-probe", "beta")],
                body: "",
            },
            "beta",
        ),
    },
    Row {
        case: "two required query strings, both omitted",
        path_part: "pair",
        verb: "GET",
        validate_parameters: true,
        validate_body: false,
        parameters: &[
            ("method.request.querystring.marker", true),
            ("method.request.querystring.trace", true),
        ],
        model: false,
        refused: Some((
            Call {
                query: "",
                headers: &[],
                body: "",
            },
            "Missing required request parameters: [marker, trace]",
        )),
        served: (
            Call {
                query: "?marker=gamma&trace=t",
                headers: &[],
                body: "",
            },
            "gamma",
        ),
    },
    Row {
        case: "an OPTIONAL query string the call omits",
        path_part: "open",
        verb: "GET",
        validate_parameters: true,
        validate_body: false,
        parameters: &[("method.request.querystring.marker", false)],
        model: false,
        refused: None,
        served: (
            Call {
                query: "",
                headers: &[],
                body: "",
            },
            "",
        ),
    },
    Row {
        case: "a validator that checks NEITHER half, with a required query string declared",
        path_part: "unchecked",
        verb: "GET",
        validate_parameters: false,
        validate_body: false,
        parameters: &[("method.request.querystring.marker", true)],
        model: false,
        refused: None,
        served: (
            Call {
                query: "",
                headers: &[],
                body: "",
            },
            "",
        ),
    },
    Row {
        case: "a payload the method's model does not admit",
        path_part: "payload",
        verb: "POST",
        validate_parameters: false,
        validate_body: true,
        parameters: &[],
        model: true,
        refused: Some((
            Call {
                query: "",
                headers: &[],
                body: "{}",
            },
            "Invalid request body",
        )),
        served: (
            Call {
                query: "",
                headers: &[],
                body: r#"{"marker":"delta"}"#,
            },
            "delta",
        ),
    },
];

/// The logical id of one row's resource, method, validator and model.
fn resource_id(index: usize) -> String {
    format!("Resource{index}")
}
fn method_id(index: usize) -> String {
    format!("Method{index}")
}
fn validator_id(index: usize) -> String {
    format!("Validator{index}")
}
fn model_id(index: usize) -> String {
    format!("Model{index}")
}

/// The name of one row's model: `CreateModel$name` "must be alphanumeric", so the name is the
/// row's own, written in that alphabet.
fn model_name(index: usize) -> String {
    format!("Payload{index}")
}

/// The schema one row's model declares, as a template states one — a JSON document, which is
/// what the type's `Schema` property is.
fn model_schema() -> Value {
    json!({
        "$schema": "http://json-schema.org/draft-04/schema#",
        "title": "Payload",
        "type": "object",
        "required": ["marker"],
        "properties": { "marker": { "type": "string" } }
    })
}

/// The template the suite deploys: the function behind every method, the api, and one resource,
/// validator and method per row, published by one deployment and stage.
fn template(stack: &str) -> String {
    let function = format!("{stack}-echo");
    let mut resources = Map::new();
    resources.insert(
        "Role".to_string(),
        json!({
            "Type": "AWS::IAM::Role",
            "Properties": {
                "AssumeRolePolicyDocument": {
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": { "Service": "lambda.amazonaws.com" },
                        "Action": "sts:AssumeRole"
                    }]
                },
                // The execution role a Serverless-Framework deploy writes for its functions,
                // which is what lets a run create its log stream and put its records.
                "Policies": [{
                    "PolicyName": "lambda-logs",
                    "PolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Action": [
                                "logs:CreateLogGroup",
                                "logs:CreateLogStream",
                                "logs:PutLogEvents"
                            ],
                            "Resource": "*"
                        }]
                    }
                }]
            }
        }),
    );
    resources.insert(
        "Fn".to_string(),
        json!({
            "Type": "AWS::Lambda::Function",
            "Properties": {
                "FunctionName": function,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Timeout": FUNCTION_TIMEOUT,
                "Role": { "Fn::GetAtt": ["Role", "Arn"] },
                "Code": { "ZipFile": HANDLER_SOURCE }
            }
        }),
    );
    resources.insert(
        "Permission".to_string(),
        json!({
            "Type": "AWS::Lambda::Permission",
            "Properties": {
                "Action": "lambda:InvokeFunction",
                "FunctionName": { "Fn::GetAtt": ["Fn", "Arn"] },
                "Principal": "apigateway.amazonaws.com"
            }
        }),
    );
    resources.insert(
        "Api".to_string(),
        json!({
            "Type": "AWS::ApiGateway::RestApi",
            "Properties": {
                "Name": format!("{stack}-api"),
                "EndpointConfiguration": { "Types": ["REGIONAL"] }
            }
        }),
    );
    let mut methods: Vec<String> = Vec::new();
    for (index, row) in ROWS.iter().enumerate() {
        resources.insert(
            resource_id(index),
            json!({
                "Type": "AWS::ApiGateway::Resource",
                "Properties": {
                    "RestApiId": { "Ref": "Api" },
                    "ParentId": { "Fn::GetAtt": ["Api", "RootResourceId"] },
                    "PathPart": row.path_part
                }
            }),
        );
        resources.insert(
            validator_id(index),
            json!({
                "Type": "AWS::ApiGateway::RequestValidator",
                "Properties": {
                    "RestApiId": { "Ref": "Api" },
                    "Name": format!("{stack}-{}-validator", row.path_part),
                    "ValidateRequestParameters": row.validate_parameters,
                    "ValidateRequestBody": row.validate_body
                }
            }),
        );
        let mut properties = Map::new();
        properties.insert("RestApiId".to_string(), json!({ "Ref": "Api" }));
        properties.insert(
            "ResourceId".to_string(),
            json!({ "Ref": resource_id(index) }),
        );
        properties.insert("HttpMethod".to_string(), json!(row.verb));
        properties.insert("AuthorizationType".to_string(), json!("NONE"));
        properties.insert(
            "RequestValidatorId".to_string(),
            json!({ "Ref": validator_id(index) }),
        );
        let declared: Map<String, Value> = row
            .parameters
            .iter()
            .map(|(expression, required)| ((*expression).to_string(), json!(required)))
            .collect();
        properties.insert("RequestParameters".to_string(), Value::Object(declared));
        if row.model {
            resources.insert(
                model_id(index),
                json!({
                    "Type": "AWS::ApiGateway::Model",
                    "Properties": {
                        "RestApiId": { "Ref": "Api" },
                        "Name": model_name(index),
                        "ContentType": JSON,
                        "Schema": model_schema()
                    }
                }),
            );
            properties.insert(
                "RequestModels".to_string(),
                json!({ JSON: { "Ref": model_id(index) } }),
            );
        }
        properties.insert("MethodResponses".to_string(), json!([{ "StatusCode": "200" }]));
        properties.insert(
            "Integration".to_string(),
            json!({
                "Type": "AWS_PROXY",
                "IntegrationHttpMethod": "POST",
                "Uri": {
                    "Fn::Sub":
                        "arn:aws:apigateway:${AWS::Region}:lambda:path/2015-03-31/functions/\
                         ${Fn.Arn}/invocations"
                }
            }),
        );
        resources.insert(
            method_id(index),
            json!({
                "Type": "AWS::ApiGateway::Method",
                "Properties": Value::Object(properties)
            }),
        );
        methods.push(method_id(index));
    }
    resources.insert(
        "Deployment".to_string(),
        json!({
            "Type": "AWS::ApiGateway::Deployment",
            "DependsOn": methods,
            "Properties": { "RestApiId": { "Ref": "Api" } }
        }),
    );
    resources.insert(
        "Stage".to_string(),
        json!({
            "Type": "AWS::ApiGateway::Stage",
            "Properties": {
                "RestApiId": { "Ref": "Api" },
                "DeploymentId": { "Ref": "Deployment" },
                "StageName": STAGE
            }
        }),
    );
    json!({
        "Resources": Value::Object(resources),
        "Outputs": {
            "ApiId": { "Value": { "Ref": "Api" } },
            "FunctionName": { "Value": { "Ref": "Fn" } },
            "InvokeUrl": {
                "Value": {
                    "Fn::Sub":
                        format!("https://${{Api}}.execute-api.${{AWS::Region}}.amazonaws.com/{STAGE}")
                }
            }
        }
    })
    .to_string()
}

#[tokio::test]
async fn a_declared_request_validator_refuses_the_invalid_call_before_the_integration_runs() {
    let router = emulator();
    let stack = "rest-api-request-validation";
    let body = template(stack);

    let created = cloudformation(
        &router,
        "CreateStack",
        &[
            ("StackName", stack),
            ("TemplateBody", &body),
            // The role the function assumes is what CAPABILITY_IAM acknowledges.
            ("Capabilities.member.1", "CAPABILITY_IAM"),
        ],
    )
    .await;
    assert_eq!(
        created.status,
        200,
        "setup: expected HTTP 200 from CreateStack, got {} with {:?}",
        created.status,
        created.text()
    );
    let (reached, described) = wait_for_stack_status(&router, stack, CREATE_COMPLETE).await;
    let failures = if reached == CREATE_COMPLETE {
        Vec::new()
    } else {
        let events = cloudformation(&router, "DescribeStackEvents", &[("StackName", stack)]).await;
        elements(&events.text(), "ResourceStatusReason")
    };
    assert_eq!(
        reached, CREATE_COMPLETE,
        "setup: expected the stack to reach {CREATE_COMPLETE}, got {described} with resource \
         failures {failures:?}"
    );

    let outputs = outputs_of(&described);
    let api_id = outputs
        .get("ApiId")
        .cloned()
        .unwrap_or_else(|| panic!("setup: the stack published no ApiId output in {described}"));
    let invoke_url = outputs
        .get("InvokeUrl")
        .cloned()
        .unwrap_or_else(|| panic!("setup: the stack published no InvokeUrl output in {described}"));
    let function = outputs.get("FunctionName").cloned().unwrap_or_else(|| {
        panic!("setup: the stack published no FunctionName output in {described}")
    });
    let stack_resources =
        cloudformation(&router, "DescribeStackResources", &[("StackName", stack)]).await;
    let physical: BTreeMap<String, String> = physical_ids_of(&stack_resources.text());

    // 1. Every validator the template declared is one API Gateway HOLDS, under the id the
    //    stack published for it — the stack's own records are not evidence of anything, the
    //    owning service's read is — and the method really names that id.
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let validator = physical.get(&validator_id(index)).cloned().unwrap_or_else(|| {
            panic!(
                "case {case:?}: no physical id for {} in {}",
                validator_id(index),
                stack_resources.text()
            )
        });
        let read = apigateway(
            &router,
            &format!("/restapis/{api_id}/requestvalidators/{validator}"),
        )
        .await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: expected HTTP 200 from GetRequestValidator for the validator the \
             stack published ({validator}), got {} with {:?}",
            read.status,
            read.text()
        );
        let held = read.json();
        assert_eq!(
            held.get("id").and_then(Value::as_str),
            Some(validator.as_str()),
            "case {case:?}: expected the validator to report the id the stack published, got \
             {held}"
        );
        assert_eq!(
            held.get("validateRequestParameters").and_then(Value::as_bool),
            Some(row.validate_parameters),
            "case {case:?}: expected validateRequestParameters {}, got {held}",
            row.validate_parameters
        );
        assert_eq!(
            held.get("validateRequestBody").and_then(Value::as_bool),
            Some(row.validate_body),
            "case {case:?}: expected validateRequestBody {}, got {held}",
            row.validate_body
        );

        let resource = physical.get(&resource_id(index)).cloned().unwrap_or_else(|| {
            panic!(
                "case {case:?}: no physical id for {} in {}",
                resource_id(index),
                stack_resources.text()
            )
        });
        let method = apigateway(
            &router,
            &format!(
                "/restapis/{api_id}/resources/{resource}/methods/{}",
                row.verb
            ),
        )
        .await;
        assert_eq!(
            method.status,
            200,
            "case {case:?}: expected HTTP 200 from GetMethod for {} /{}, got {} with {:?}",
            row.verb,
            row.path_part,
            method.status,
            method.text()
        );
        let declared = method.json();
        assert_eq!(
            declared.get("requestValidatorId").and_then(Value::as_str),
            Some(validator.as_str()),
            "case {case:?}: expected the method to name the validator API Gateway holds \
             ({validator}), got {declared}"
        );

        // A row with a model asserts the same of it: the model the method maps its content
        // type to is one that service holds, under the name the stack published, carrying the
        // schema the template declared.
        if row.model {
            let name = model_name(index);
            assert_eq!(
                physical.get(&model_id(index)).map(String::as_str),
                Some(name.as_str()),
                "case {case:?}: expected the stack to publish the model's name, got {:?}",
                physical.get(&model_id(index))
            );
            let model = apigateway(&router, &format!("/restapis/{api_id}/models/{name}")).await;
            assert_eq!(
                model.status,
                200,
                "case {case:?}: expected HTTP 200 from GetModel for {name}, got {} with {:?}",
                model.status,
                model.text()
            );
            let held = model.json();
            assert_eq!(
                held.get("name").and_then(Value::as_str),
                Some(name.as_str()),
                "case {case:?}: expected the model to report its own name, got {held}"
            );
            let stored: Value = held
                .get("schema")
                .and_then(Value::as_str)
                .map(|text| serde_json::from_str(text).unwrap_or(Value::Null))
                .unwrap_or(Value::Null);
            assert_eq!(
                stored,
                model_schema(),
                "case {case:?}: expected the model to hold the schema the template declared, \
                 got {held}"
            );
            assert_eq!(
                declared
                    .get("requestModels")
                    .and_then(|models| models.get(JSON))
                    .and_then(Value::as_str),
                Some(name.as_str()),
                "case {case:?}: expected the method to map {JSON} to the model, got {declared}"
            );
        }
    }

    // 2. Every call the validators must REFUSE, made first, so what the function's log group
    //    holds afterwards says whether any of them reached the integration.
    let configuration = wait_until_active(&router, &function).await;
    let group = configuration
        .get("LoggingConfig")
        .and_then(|logging| logging.get("LogGroup"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        group,
        format!("/aws/lambda/{function}"),
        "setup: expected the function's own log group, got {group:?}"
    );
    for row in ROWS {
        let case = row.case;
        let Some((call, message)) = &row.refused else {
            continue;
        };
        let answered = invoke(
            &router,
            &invoke_url,
            row.verb,
            &format!("/{}", row.path_part),
            call.query,
            call.headers,
            call.body,
        )
        .await;
        assert_eq!(
            answered.status,
            400,
            "case {case:?}: expected the deployed stage to refuse the call with HTTP 400, got \
             {} with {:?}",
            answered.status,
            answered.text()
        );
        assert_eq!(
            answered.json().get("message").and_then(Value::as_str),
            Some(*message),
            "case {case:?}: expected the request-validation message {message:?}, got {:?}",
            answered.text()
        );
    }

    // 3. Every call the validators must ADMIT is served by the integration behind the method.
    for row in ROWS {
        let case = row.case;
        let (call, echo) = &row.served;
        let answered = invoke(
            &router,
            &invoke_url,
            row.verb,
            &format!("/{}", row.path_part),
            call.query,
            call.headers,
            call.body,
        )
        .await;
        assert_eq!(
            answered.status,
            200,
            "case {case:?}: expected the deployed stage to serve the call with HTTP 200, got {} \
             with {:?}",
            answered.status,
            answered.text()
        );
        let payload = answered.json();
        assert_eq!(
            payload.get("echo").and_then(Value::as_str),
            Some(*echo),
            "case {case:?}: expected the integration to answer the marker {echo:?}, got {:?}",
            answered.text()
        );
        assert_eq!(
            payload.get("path").and_then(Value::as_str),
            Some(format!("/{}", row.path_part).as_str()),
            "case {case:?}: expected the integration to see its own path, got {:?}",
            answered.text()
        );
    }

    // 4. And the integration ran for exactly the admitted calls: the function's log group holds
    //    one of the handler's own lines per served call and none for a refused one, which is
    //    what "refused before the integration runs" means.
    let served: Vec<String> = ROWS
        .iter()
        .map(|row| format!("{INVOKED} /{}", row.path_part))
        .collect();
    let messages = messages_in(&router, &group, served.len()).await;
    let mut invocations: Vec<String> = messages
        .iter()
        .filter(|line| line.contains(INVOKED))
        .map(|line| line.trim().to_string())
        .collect();
    invocations.sort();
    let mut expected = served.clone();
    expected.sort();
    assert_eq!(
        invocations, expected,
        "case \"the integration ran for exactly the admitted calls\": expected one handler line \
         per served call and none for a refused one, got {messages:?}"
    );
}
