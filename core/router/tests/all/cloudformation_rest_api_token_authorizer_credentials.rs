// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation-deployed REST API whose method is gated by a `TOKEN` Lambda authorizer
//! reaches a DECISION on its deployed stage, and what admits the authorizer's invocation is
//! the alternative the authorizer's own record names.
//!
//! The rule under test: **an `Authorizer` of type `TOKEN` is invoked by API Gateway and the
//! `policyDocument` it returns decides the call — an `Allow` on the invoked method runs that
//! method's integration and the client is answered with what the integration produced, and an
//! explicit `Deny` is refused with `403`. WHAT admits the invocation is the choice
//! `Authorizer$authorizerCredentials` states: a role named there is assumed, so the role's own
//! trust and permissions admit the call and the function's resource-based policy is not
//! consulted; "to use resource-based permissions on the Lambda function, specify null", so an
//! authorizer naming no role is admitted only by the grant on the function. An authorizer
//! neither admits earns the `500` `{"message":"Internal server error"}` envelope API Gateway
//! reserves for an authorizer it cannot invoke — and nothing else does.**
//!
//! Five crates have to agree for that to hold, which is why this suite lives here rather than
//! in any one of them: CloudFormation expands the template and makes every create call, IAM
//! answers for the two roles and their documents, STS is what the assume-role reading goes
//! through, AWS Lambda holds the authorizer function, runs it and answers for its resource
//! policy, and API Gateway holds the api, its authorizer, its method and the stage that
//! serves them. A stack that never settles, or a stage that answers neither leg, is what this
//! suite would catch: the stack is waited on by its own `StackStatus` and the legs are driven
//! through the invoke URL the stack published.
//!
//! The rows differ along what the rule generalises over: which alternative admits the
//! invocation (a role, or the function's own grant), and each way it is NOT admitted (a role
//! allowed no invocation of the function, a role this service cannot assume at all, and no
//! role beside a function that grants nothing) — plus the stage name and the resource path,
//! which the rule is indifferent to.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::query_wire::{elements, form_encode, outputs_of, physical_ids_of};
use crate::rds_docdb_neptune_signing_name_routing::authorization;

/// The Region every request here is signed for, and so the Region the stack's resources
/// belong to.
const REGION: &str = "us-east-1";

/// The `StackStatus` every row is waited for.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// How often the waits below look again, and how long they give up after. Generous: what is
/// waited on is a CONDITION another service owns, never an elapsed time this suite asserts on.
const LOOK: Duration = Duration::from_millis(25);
const DEADLINE: Duration = Duration::from_secs(120);

/// The token the authorizer's own handler returns an `Allow` for, and the one it returns an
/// explicit `Deny` for. They are the handler's inputs, not values this suite reads back.
const ALLOW_TOKEN: &str = "allow-me";
const DENY_TOKEN: &str = "deny-me";

/// The header every authorizer below declares as its identity source.
const IDENTITY_HEADER: &str = "authorization";

/// The body the gated method's `MOCK` integration answers an admitted call with, which is the
/// template's own fixed text.
const OK_BODY: &str = r#"{"message":"ok"}"#;

/// The sentence a call the returned policy explicitly DENIES is refused with, and the status
/// API Gateway refuses it at.
const EXPLICIT_DENY: &str = "User is not authorized to access this resource with an explicit deny";
const DENIED_STATUS: u16 = 403;

/// The status and the fixed generic body API Gateway answers when the authorizer cannot be
/// invoked at all.
const CONFIGURATION_ERROR: (u16, &str) = (500, r#"{"message":"Internal server error"}"#);

/// The authorizer function every row deploys: it reads the bearer token out of the `TOKEN`
/// event it is handed and returns a policy that allows the invoked method for one token and
/// explicitly denies it for every other, so the leg a client sees is the function's own
/// answer.
const AUTHORIZER_SOURCE: &str = r#"
def lambda_handler(event, context):
    token = event.get("authorizationToken")
    effect = "Allow" if token == "allow-me" else "Deny"
    return {
        "principalId": "caller",
        "policyDocument": {
            "Version": "2012-10-17",
            "Statement": [
                {
                    "Action": "execute-api:Invoke",
                    "Effect": effect,
                    "Resource": event["methodArn"],
                }
            ],
        },
    }
"#;

/// The emulator as it is ASSEMBLED, on one engine: the api CloudFormation creates is the api
/// this suite then invokes, and the authorizer behind it is the function AWS Lambda runs.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    // The signature is never checked here, as in every other suite of this crate; the
    // credential scope is what routes a request.
    router.set_request_authentication(false);
    router
}

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Read one API Gateway control-plane resource over its restJson1 wire.
async fn apigateway(router: &MultiServiceRouter, uri: &str) -> (u16, Value) {
    let request = http::Request::builder()
        .method("GET")
        .uri(uri)
        .header("host", format!("apigateway.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("apigateway"))
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let answered: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, answered)
}

/// One Lambda `restJson1` read, for the wait below.
async fn lambda(router: &MultiServiceRouter, uri: &str) -> Value {
    let request = http::Request::builder()
        .method("GET")
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("lambda"))
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    serde_json::from_slice(response.body()).unwrap_or(Value::Null)
}

/// Poll `GetFunctionConfiguration` until Lambda first reports the function `Active`.
///
/// The span a created function spends coming up is that service's own, so no fixed sleep
/// names the instant it ends: the wait is on the CONDITION, looking again until it holds.
async fn wait_until_active(router: &MultiServiceRouter, name: &str) -> String {
    let giving_up_at = Instant::now() + DEADLINE;
    loop {
        let configuration =
            lambda(router, &format!("/2015-03-31/functions/{name}/configuration")).await;
        let state = configuration
            .get("State")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if state == "Active" || Instant::now() >= giving_up_at {
            return state;
        }
        tokio::time::sleep(LOOK).await;
    }
}

/// Invoke the deployed stage at the invoke URL the stack published, carrying `token` as the
/// value of the authorizer's identity source.
///
/// The URL is taken apart rather than rebuilt here, so what is driven is the value the stack
/// told the caller to use.
async fn invoke(router: &MultiServiceRouter, invoke_url: &str, token: &str) -> (u16, String) {
    let without_scheme = invoke_url
        .strip_prefix("https://")
        .or_else(|| invoke_url.strip_prefix("http://"))
        .unwrap_or(invoke_url);
    let (host, path) = match without_scheme.split_once('/') {
        Some((host, path)) => (host.to_string(), format!("/{path}")),
        None => (without_scheme.to_string(), "/".to_string()),
    };
    let request = http::Request::builder()
        .method("GET")
        .uri(path)
        .header("host", host)
        .header(IDENTITY_HEADER, token)
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// The message an API Gateway data-plane answer carries, under either spelling the service
/// renders one by, or the whole text where it carries none.
fn message(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|answered| {
            ["message", "Message"]
                .into_iter()
                .find_map(|member| {
                    answered
                        .get(member)
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
        })
        .unwrap_or_else(|| body.to_string())
}

/// One row: how the authorizer's invocation is meant to be admitted, and whether the stage
/// therefore reaches the function's own decision.
struct Row {
    case: &'static str,
    /// The principal the api-side role's trust policy names, and the action its inline policy
    /// allows — `None` for the row that declares no such role at all, so the authorizer names
    /// no credentials.
    role: Option<(&'static str, &'static str)>,
    /// Whether the template grants `apigateway.amazonaws.com` an invocation of the function
    /// through the FUNCTION's own resource policy.
    function_grant: bool,
    /// The stage the api is deployed to, and the path part the gated method sits on.
    stage: &'static str,
    path_part: &'static str,
    /// Whether the authorizer is invocable, so the legs are its function's own decision.
    decided: bool,
}

/// The logical id of the api-side role, the function and the api in every template below.
const ROLE_ID: &str = "ApiGatewayInvokeRole";
const FUNCTION_ID: &str = "AuthorizerFunction";

/// The template one row deploys: the authorizer function and its execution role, the api-side
/// role the row describes, and the api — a resource, a `TOKEN` authorizer, a `CUSTOM` method
/// with a `MOCK` integration behind it, a deployment and a stage.
fn template(row: &Row) -> String {
    let mut resources = json!({
        "LambdaExecutionRole": {
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
        FUNCTION_ID: {
            "Type": "AWS::Lambda::Function",
            "Properties": {
                "Runtime": "python3.12",
                "Handler": "index.lambda_handler",
                "Timeout": 30,
                "Role": { "Fn::GetAtt": ["LambdaExecutionRole", "Arn"] },
                "Code": { "ZipFile": AUTHORIZER_SOURCE }
            }
        },
        "RestApi": {
            "Type": "AWS::ApiGateway::RestApi",
            "Properties": {
                "Name": format!("token-authorizer-{}", row.path_part),
                "EndpointConfiguration": { "Types": ["REGIONAL"] }
            }
        },
        "GatedResource": {
            "Type": "AWS::ApiGateway::Resource",
            "Properties": {
                "RestApiId": { "Ref": "RestApi" },
                "ParentId": { "Fn::GetAtt": ["RestApi", "RootResourceId"] },
                "PathPart": row.path_part
            }
        },
        "TokenAuthorizer": {
            "Type": "AWS::ApiGateway::Authorizer",
            "Properties": {
                "RestApiId": { "Ref": "RestApi" },
                "Name": format!("{}-token-authorizer", row.path_part),
                "Type": "TOKEN",
                "IdentitySource": "method.request.header.Authorization",
                "AuthorizerResultTtlInSeconds": 0,
                "AuthorizerUri": {
                    "Fn::Sub": format!(
                        "arn:aws:apigateway:${{AWS::Region}}:lambda:path/2015-03-31/functions/\
                         ${{{FUNCTION_ID}.Arn}}/invocations"
                    )
                }
            }
        },
        "SecureMethod": {
            "Type": "AWS::ApiGateway::Method",
            "Properties": {
                "RestApiId": { "Ref": "RestApi" },
                "ResourceId": { "Ref": "GatedResource" },
                "HttpMethod": "GET",
                "AuthorizationType": "CUSTOM",
                "AuthorizerId": { "Ref": "TokenAuthorizer" },
                "MethodResponses": [{ "StatusCode": "200" }],
                "Integration": {
                    "Type": "MOCK",
                    "RequestTemplates": { "application/json": r#"{"statusCode": 200}"# },
                    "IntegrationResponses": [{
                        "StatusCode": "200",
                        "ResponseTemplates": { "application/json": OK_BODY }
                    }]
                }
            }
        },
        "ApiDeployment": {
            "Type": "AWS::ApiGateway::Deployment",
            "DependsOn": ["SecureMethod"],
            "Properties": {
                "RestApiId": { "Ref": "RestApi" },
                "Description": "the gated deployment"
            }
        },
        "ApiStage": {
            "Type": "AWS::ApiGateway::Stage",
            "Properties": {
                "RestApiId": { "Ref": "RestApi" },
                "DeploymentId": { "Ref": "ApiDeployment" },
                "StageName": row.stage
            }
        }
    });
    let resources = resources
        .as_object_mut()
        .expect("the resources object just built");

    if let Some((principal, action)) = row.role {
        resources.insert(
            ROLE_ID.to_string(),
            json!({
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "AssumeRolePolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": { "Service": principal },
                            "Action": "sts:AssumeRole"
                        }]
                    },
                    "Policies": [{
                        "PolicyName": "invoke-the-authorizer",
                        "PolicyDocument": {
                            "Version": "2012-10-17",
                            "Statement": [{
                                "Effect": "Allow",
                                "Action": action,
                                "Resource": { "Fn::GetAtt": [FUNCTION_ID, "Arn"] }
                            }]
                        }
                    }]
                }
            }),
        );
        // The authorizer names that role, which is the alternative to a grant on the function.
        resources
            .get_mut("TokenAuthorizer")
            .and_then(|authorizer| authorizer.get_mut("Properties"))
            .and_then(Value::as_object_mut)
            .expect("the authorizer's properties")
            .insert(
                "AuthorizerCredentials".to_string(),
                json!({ "Fn::GetAtt": [ROLE_ID, "Arn"] }),
            );
    }
    if row.function_grant {
        resources.insert(
            "AuthorizerPermission".to_string(),
            json!({
                "Type": "AWS::Lambda::Permission",
                "Properties": {
                    "Action": "lambda:InvokeFunction",
                    "FunctionName": { "Fn::GetAtt": [FUNCTION_ID, "Arn"] },
                    "Principal": "apigateway.amazonaws.com"
                }
            }),
        );
    }

    json!({
        "Resources": resources,
        "Outputs": {
            "ApiId": { "Value": { "Ref": "RestApi" } },
            "InvokeUrl": {
                "Value": {
                    "Fn::Sub": format!(
                        "https://${{RestApi}}.execute-api.${{AWS::Region}}.amazonaws.com/{}/{}",
                        row.stage, row.path_part
                    )
                }
            }
        }
    })
    .to_string()
}

/// What the stack published, once it has settled: the api id and the invoke URL of its stage.
async fn deployed(router: &MultiServiceRouter, row: &Row, stack: &str) -> (String, String) {
    let case = row.case;
    let body = template(row);
    let (status, answered) = cloudformation(
        router,
        "CreateStack",
        &[
            ("StackName", stack),
            ("TemplateBody", &body),
            // The template declares roles, which is what CAPABILITY_IAM acknowledges.
            ("Capabilities.member.1", "CAPABILITY_IAM"),
            ("Capabilities.member.2", "CAPABILITY_NAMED_IAM"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {answered}"
    );

    let (settled, described) = wait_for_stack_status(router, stack, CREATE_COMPLETE).await;
    let failures = if settled == CREATE_COMPLETE {
        Vec::new()
    } else {
        let (_, events) =
            cloudformation(router, "DescribeStackEvents", &[("StackName", stack)]).await;
        elements(&events, "ResourceStatusReason")
    };
    assert_eq!(
        settled, CREATE_COMPLETE,
        "case {case:?}: expected the stack to reach {CREATE_COMPLETE}, got {described} with \
         resource failures {failures:?}"
    );

    let outputs = outputs_of(&described);
    let api_id = outputs
        .get("ApiId")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no ApiId output in {described}"));
    let invoke_url = outputs
        .get("InvokeUrl")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no InvokeUrl output in {described}"));

    // The function the authorizer names is the one AWS Lambda holds, and the authorizer the
    // api holds is the one the template declared — read back out of each OWNING service
    // rather than out of the stack's own records.
    let (_, stack_resources) =
        cloudformation(router, "DescribeStackResources", &[("StackName", stack)]).await;
    let physical: BTreeMap<String, String> = physical_ids_of(&stack_resources);
    let function = physical
        .get(FUNCTION_ID)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no physical id for {FUNCTION_ID} in {stack_resources}"));
    let state = wait_until_active(router, &function).await;
    assert_eq!(
        state, "Active",
        "case {case:?}: expected Lambda to report {function} Active before the invoke, got \
         {state:?}"
    );

    let (status, authorizers) =
        apigateway(router, &format!("/restapis/{api_id}/authorizers")).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetAuthorizers, got {status} with body \
         {authorizers}"
    );
    let authorizer = authorizers
        .get("item")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the api holds no authorizer: {authorizers}"));
    assert_eq!(
        authorizer.get("type").and_then(Value::as_str),
        Some("TOKEN"),
        "case {case:?}: expected the stack's authorizer to be a TOKEN one, got {authorizer}"
    );
    let reported = authorizer
        .get("authorizerCredentials")
        .and_then(Value::as_str);
    match row.role {
        Some(_) => {
            let role = physical
                .get(ROLE_ID)
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: no physical id for {ROLE_ID}"));
            let expected = format!(
                "arn:aws:iam::{account}:role/{role}",
                account = engine::emulator_account_id()
            );
            assert_eq!(
                reported,
                Some(expected.as_str()),
                "case {case:?}: expected the authorizer to name the role the stack created, \
                 got {authorizer}"
            );
        }
        None => assert_eq!(
            reported, None,
            "case {case:?}: expected the authorizer to name no credentials, got {authorizer}"
        ),
    }

    (api_id, invoke_url)
}

/// Drive both legs of one row against the stage the stack deployed.
async fn assert_legs(router: &MultiServiceRouter, row: &Row, invoke_url: &str) {
    let case = row.case;
    let legs: [(&str, &str, u16, String); 2] = [
        (
            "the token the authorizer allows",
            ALLOW_TOKEN,
            200,
            OK_BODY.to_string(),
        ),
        (
            "the token the authorizer explicitly denies",
            DENY_TOKEN,
            DENIED_STATUS,
            EXPLICIT_DENY.to_string(),
        ),
    ];
    for (which, token, allowed_status, allowed_body) in legs {
        let (expected_status, expected_body) = if row.decided {
            (allowed_status, allowed_body)
        } else {
            (CONFIGURATION_ERROR.0, CONFIGURATION_ERROR.1.to_string())
        };
        let (status, answered) = invoke(router, invoke_url, token).await;
        assert_eq!(
            status, expected_status,
            "case {case:?} / {which}: expected HTTP {expected_status} at {invoke_url}, got \
             {status} with body {answered:?}"
        );
        // An admitted call is answered with the integration's own body; a call the returned
        // policy refused with the sentence its gateway response carries; and one whose
        // authorizer was never invoked with the whole fixed generic envelope.
        let reported = if status == 200 || !row.decided {
            answered.clone()
        } else {
            message(&answered)
        };
        assert_eq!(
            reported, expected_body,
            "case {case:?} / {which}: expected {expected_body:?}, got {answered:?}"
        );
    }
}

/// The rows whose authorizer IS invocable, so each leg is the function's own decision.
const DECIDED: &[Row] = &[
    Row {
        case: "an authorizerCredentials role that trusts this service and allows the invocation",
        role: Some(("apigateway.amazonaws.com", "lambda:InvokeFunction")),
        function_grant: false,
        stage: "test",
        path_part: "secure",
        decided: true,
    },
    Row {
        case: "no authorizerCredentials at all, with the grant on the function instead",
        role: None,
        function_grant: true,
        stage: "prod",
        path_part: "guarded",
        decided: true,
    },
];

/// The rows whose authorizer is NOT invocable, each by a different missing permission.
const REFUSED: &[Row] = &[
    Row {
        case: "an authorizerCredentials role allowed no invocation of the function",
        role: Some(("apigateway.amazonaws.com", "logs:PutLogEvents")),
        function_grant: false,
        stage: "beta",
        path_part: "powerless",
        decided: false,
    },
    Row {
        case: "an authorizerCredentials role this service cannot assume at all",
        role: Some(("lambda.amazonaws.com", "lambda:InvokeFunction")),
        function_grant: false,
        stage: "v1",
        path_part: "untrusted",
        decided: false,
    },
    Row {
        case: "no authorizerCredentials and no grant on the function either",
        role: None,
        function_grant: false,
        stage: "stage-2",
        path_part: "ungranted",
        decided: false,
    },
];

/// RULE, decided legs: the stack settles, and its stage reaches the authorizer's own decision
/// — the `Allow` leg runs the method's `MOCK` integration and is answered with the body the
/// template declared, and the explicitly denied leg is refused `403`.
#[tokio::test]
async fn a_cfn_deployed_token_authorizer_decides_its_stage() {
    let router = build_router();
    for (index, row) in DECIDED.iter().enumerate() {
        let stack = format!("token-authorizer-decided-{index}");
        let (api_id, invoke_url) = deployed(&router, row, &stack).await;
        // The stage the stack deployed is one API Gateway HOLDS, which is what serves the
        // invoke URL below.
        let (status, stage) =
            apigateway(&router, &format!("/restapis/{api_id}/stages/{}", row.stage)).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from GetStage for {}, got {status} with body {stage}",
            row.case, row.stage
        );
        assert_legs(&router, row, &invoke_url).await;
    }
}

/// RULE, refused legs: an authorizer nothing admits an invocation of earns the `500` generic
/// envelope on every leg — the case a caller comparing two configurations has to be able to
/// tell from a working one, which is why it is asserted beside the decided rows above.
#[tokio::test]
async fn a_cfn_deployed_token_authorizer_nothing_admits_earns_the_configuration_error() {
    let router = build_router();
    for (index, row) in REFUSED.iter().enumerate() {
        let stack = format!("token-authorizer-refused-{index}");
        let (_, invoke_url) = deployed(&router, row, &stack).await;
        assert_legs(&router, row, &invoke_url).await;
    }
}
