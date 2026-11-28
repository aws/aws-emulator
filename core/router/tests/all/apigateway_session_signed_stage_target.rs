// SPDX-License-Identifier: CC0-1.0

//! A deployed `AWS_IAM` REST method invoked by an STS ROLE SESSION on an emulator that VERIFIES
//! request signatures, over the request TARGETS a client spells the same resource with.
//!
//! The RULE the rows assert, one level above the recorded case:
//!
//! > A request to a deployed stage is answered about the method its request target selects, and
//! > the target is NORMALISED before anything is decided about it: the empty and dot segments
//! > RFC 3986 removes are removed, so `/{stage}//a/b`, `/{stage}/a//b` and `/{stage}/a/b` all
//! > select one method and are all decided about one method ARN. That normalisation is also what
//! > every AWS SDK signer applies to the target before it computes a signature over it, so the
//! > signature a client presents covers the NORMALISED target while the wire carries the target
//! > as the client wrote it — and a stage that verified a signature over the unnormalised target
//! > would refuse every such request as a signature mismatch instead of deciding it.
//! > The decision itself is IAM's: a session of a role that holds no `execute-api:Invoke` grant
//! > for the selected method's own ARN is refused `403` with `x-amzn-ErrorType:
//! > AccessDeniedException` and the sentence IAM words an authorization failure in, naming that
//! > method ARN, and a session of a role that does hold one is answered the integration's own
//! > response.
//!
//! The rows span what the rule generalises over: WHICH grant the signing session's role holds
//! (the stage's `GET` verb, another verb only, nothing at all), HOW the client spelled the
//! target (plain, a doubled slash after the stage — the spelling the recorded case sends — a
//! doubled slash inside the path, both doubled, a trailing slash), and WHICH value the method's
//! path parameter is bound to (two sibling ids of one method).
//!
//! The second test drives the same rule over the INTEGRATION a granted session is served: an
//! `AWS_PROXY` one, whose function API Gateway invokes as its own service principal — so a
//! session that holds nothing but the invoke grant is served the function's run, and the one the
//! gate refuses is refused with nothing run.
//!
//! Only the assembled router can pin this: the credential the request is signed with is minted
//! by STS, held in the emulator's own credential store and verified by the emulator's own
//! verifier, the grant is held by IAM, and the stage that asks all three is API Gateway's
//! `execute-api` data plane. The neighbouring suites pin the pieces around it and are not
//! repeated here: `apigateway_iam_authenticated_invoke` pins WHICH credentials authenticate at
//! all, `apigateway_iam_authorized_invoke` pins WHICH grant shapes the gate admits, and
//! `apigateway_iam_authorization_debug_lifecycle` pins the whole debugging lifecycle over a
//! Lambda-proxy integration reading DynamoDB.
//!
//! Assembling the router costs stack, so it runs on an explicitly sized thread for the same
//! reason `boot_serve.rs` does.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use aws_credential_types::Credentials;
use aws_emulator_router::{build_router, InMemoryEngine};
use aws_sigv4::http_request::{sign, SignableBody, SignableRequest, SigningSettings};
use aws_sigv4::sign::v4::SigningParams;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::query_wire::{element, form_encode};

/// Keep this at or above the size `boot_serve.rs` uses.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The Region every request here is signed for, and so the Region of the api's own invoke host
/// and of every ARN a grant names.
const REGION: &str = "us-east-1";

/// The account this emulator serves.
const ACCOUNT: &str = ::engine::EMULATOR_ACCOUNT_ID;

/// The stage the api is deployed to.
const STAGE: &str = "beta";

/// The literal collection resource and the path parameter under it that carries the method.
const COLLECTION: &str = "data-agreements";
const ITEM_TEMPLATE: &str = "{agreementId}";

/// The parameter's own name, which the integration echoes back so a served answer states WHICH
/// value the target bound.
const ITEM_PARAMETER: &str = "agreementId";

/// Two sibling values of that parameter: distinct objects of ONE deployed method.
const ITEM: &str = "dua-analytics-001";
const SIBLING_ITEM: &str = "dua-risk-002";

/// The signing name an invoke of a deployed stage is signed under: the `execute-api` service
/// AWS publishes the invoke endpoint and the `execute-api:Invoke` action under.
const INVOKE_SIGNING_NAME: &str = "execute-api";

/// The IAM action an invoke of a deployed method asks for.
const INVOKE_ACTION: &str = "execute-api:Invoke";

/// The IAM path the caller roles are created under, so the roles this suite asks about are
/// reachable only by the ARN IAM minted for them.
const ROLE_PATH: &str = "/xsvc-session-signed/";

/// The session name every credential here is minted under.
const SESSION_NAME: &str = "verifier-session";

/// The header a client reads the error class of a refused request out of, lowercased as HTTP
/// names compare.
const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";

/// The exception class an `execute-api` authorization refusal names.
const ACCESS_DENIED: &str = "AccessDeniedException";

/// The clause the IAM authorization sentence opens its verdict with, and the one it closes with
/// when nothing in the caller's own documents allowed the call.
const NOT_AUTHORIZED_CLAUSE: &str = "is not authorized to perform: execute-api:Invoke on resource:";
const IMPLICIT_DENY_CLAUSE: &str =
    "because no identity-based policy allows the execute-api:Invoke action";

/// The function the second suite's `AWS_PROXY` integration runs: it answers a proxy envelope
/// naming the parameter the request's target bound, so a served answer states that the run
/// happened and what it was handed.
const PROXY_HANDLER: &str = r#"
import json


def handler(event, context):
    asked = (event.get("pathParameters") or {}).get("agreementId")
    return {
        "statusCode": 200,
        "headers": {"Content-Type": "application/json"},
        "body": json.dumps({"agreementId": asked}),
    }
"#;

/// The function the proxy integration invokes, the role it runs as, and the stack that archives
/// its inline source into a deployment package this emulator can run.
const FUNCTION: &str = "xsvc-session-signed-reader";
const EXECUTION_ROLE: &str = "xsvc-session-signed-reader-role";
const STACK: &str = "xsvc-session-signed-stack";
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// How long one invocation is given, as the function's own `Timeout`: well past what a run needs
/// on any host, because what is under test is the answer and not how fast an interpreter gets
/// through the handler.
const FUNCTION_TIMEOUT: u64 = 120;

/// How often to look while waiting for work the clock owes — a created function reaching
/// `Active` — and how many looks to take. The span is that service's own, so no fixed sleep
/// names the instant it ends; the budget is a minute, far above the seconds a transition takes
/// so a loaded host cannot decide the outcome.
const LOOK: Duration = Duration::from_millis(100);
const LOOKS: usize = 600;

/// One answered request: its status, the error class it named, and its body.
struct Answered {
    status: u16,
    error_type: Option<String>,
    text: String,
}

impl Answered {
    fn json(&self) -> Value {
        serde_json::from_str(&self.text).unwrap_or(Value::Null)
    }

    /// The `Message` or `message` member an API Gateway answer carries, or the whole body.
    fn message(&self) -> String {
        let body = self.json();
        for name in ["Message", "message"] {
            if let Some(message) = body.get(name).and_then(Value::as_str) {
                return message.to_string();
            }
        }
        self.text.clone()
    }

    fn summary(&self) -> String {
        format!("{} {:?} {}", self.status, self.error_type, self.text)
    }
}

/// Dispatch one request through the assembled router, as it arrives on the wire.
async fn dispatch(router: &MultiServiceRouter, request: http::Request<Vec<u8>>) -> Answered {
    let (parts, body) = request.into_parts();
    let response = router
        .dispatch(http::Request::from_parts(parts, bytes::Bytes::from(body)))
        .await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get(ERROR_TYPE_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    Answered {
        status,
        error_type,
        text: String::from_utf8_lossy(&response.into_body()).to_string(),
    }
}

/// The credential scope an unsigned SETUP call carries, so the router can name the service it
/// addresses while authentication is still off.
fn credential_scope(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One API Gateway control-plane call, which must answer `expected`.
async fn apigw(
    router: &MultiServiceRouter,
    what: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    expected: u16,
) -> Value {
    let host = format!("apigateway.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", &host)
        .header("content-type", "application/json")
        .header("authorization", credential_scope("apigateway"))
        .body(
            body.map(|body| body.to_string().into_bytes())
                .unwrap_or_default(),
        )
        .expect("the test request is well formed");
    let answered = dispatch(router, request).await;
    assert_eq!(
        answered.status,
        expected,
        "setup {what}: must answer HTTP {expected}, got {}",
        answered.summary()
    );
    answered.json()
}

/// One awsQuery call of `service`, which must answer 200, as its XML text.
async fn query(
    router: &MultiServiceRouter,
    service: &str,
    version: &str,
    action: &str,
    members: &[(&str, &str)],
) -> String {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let host = format!("{service}.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", &host)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", credential_scope(service))
        .body(body.into_bytes())
        .expect("the test request is well formed");
    let answered = dispatch(router, request).await;
    assert_eq!(
        answered.status,
        200,
        "setup {service} {action}: must answer 200, got {}",
        answered.summary()
    );
    answered.text
}

/// A required string member of a response body.
fn as_str(what: &str, body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| panic!("setup {what} must carry a non-empty {member:?}, got {body}"))
}

/// A trust policy letting this account's own principals assume the role, which is what a caller
/// of the api assumes a role of its own account with.
fn account_trust_policy() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "AWS": format!("arn:aws:iam::{ACCOUNT}:root") },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// The credential of one assumed-role session, and the role it speaks for.
struct Session {
    role: String,
    key_id: String,
    secret: String,
    token: String,
}

/// Create a role under [`ROLE_PATH`] holding `grant` as its one inline document, assume it, and
/// answer the session credential STS minted.
async fn session(router: &MultiServiceRouter, role: &str, grant: Option<&str>) -> Session {
    let created = query(
        router,
        "iam",
        "2010-05-08",
        "CreateRole",
        &[
            ("RoleName", role),
            ("Path", ROLE_PATH),
            ("AssumeRolePolicyDocument", &account_trust_policy()),
        ],
    )
    .await;
    let role_arn = element(&created, "Arn");
    if let Some(document) = grant {
        query(
            router,
            "iam",
            "2010-05-08",
            "PutRolePolicy",
            &[
                ("RoleName", role),
                ("PolicyName", "invoke"),
                ("PolicyDocument", document),
            ],
        )
        .await;
    }
    let assumed = query(
        router,
        "sts",
        "2011-06-15",
        "AssumeRole",
        &[("RoleArn", &role_arn), ("RoleSessionName", SESSION_NAME)],
    )
    .await;
    Session {
        role: role.to_string(),
        key_id: element(&assumed, "AccessKeyId"),
        secret: element(&assumed, "SecretAccessKey"),
        token: element(&assumed, "SessionToken"),
    }
}

/// One identity-based document allowing `INVOKE_ACTION` on `resource`.
fn allow(resource: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": INVOKE_ACTION, "Resource": resource }],
    })
    .to_string()
}

/// The request target NORMALISED as an AWS SDK's signer normalises it before signing: the empty
/// and `.` segments RFC 3986's "remove dot segments" drops are dropped and a `..` segment takes
/// the previous one with it, with the leading slash kept and a trailing slash preserved.
///
/// This is what makes the signature a client presents one over the COLLAPSED target while the
/// wire carries the target the client wrote. It is spelled out here rather than left to the
/// signer's own normalisation, so what each row signs is the collapsed target whatever the
/// signer's settings do with the written one — the assertion is about the STAGE's reading of the
/// two, not about the signing crate's.
fn normalised(target: &str) -> String {
    let mut segments: Vec<&str> = Vec::new();
    for segment in target.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            other => segments.push(other),
        }
    }
    let trailing = target.len() > 1 && target.ends_with('/');
    format!(
        "/{}{}",
        segments.join("/"),
        if trailing { "/" } else { "" }
    )
}

/// The request one row sends: built over the target the client wrote, signed with `session` over
/// the NORMALISED target, and sent with the written one.
fn signed_request(session: &Session, host: &str, target: &str) -> http::Request<Vec<u8>> {
    let built = http::Request::builder()
        .method("GET")
        .uri(format!("https://{host}{}", normalised(target)))
        .header("host", host)
        .body(Vec::new())
        .expect("the test request is well formed");
    let identity = Credentials::new(
        &session.key_id,
        &session.secret,
        Some(session.token.clone()),
        None,
        "test",
    )
    .into();
    let params = SigningParams::builder()
        .identity(&identity)
        .region(REGION)
        .name(INVOKE_SIGNING_NAME)
        .time(SystemTime::now())
        .settings(SigningSettings::default())
        .build()
        .expect("the signing parameters are complete")
        .into();
    let (method, uri) = (built.method().to_string(), built.uri().to_string());
    let headers: Vec<(String, String)> = built
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                value.to_str().expect("a text header").to_string(),
            )
        })
        .collect();
    let signable = SignableRequest::new(
        &method,
        &uri,
        headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str())),
        SignableBody::Bytes(built.body()),
    )
    .expect("the request is signable");
    let (instructions, _) = sign(signable, &params)
        .expect("the request signs")
        .into_parts();
    let mut request = built;
    instructions.apply_to_request_http1x(&mut request);
    // The TARGET the client wrote, which the signature above covers only once normalised.
    *request.uri_mut() = format!("https://{host}{target}")
        .parse()
        .expect("the written target is a uri");
    request
}

/// What the stage owes one row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Owed {
    /// The session's grant covers the selected method: the integration's own answer.
    Served,
    /// Nothing in the session's documents allows the invoke: `403` with the IAM sentence.
    NotAuthorized,
}

/// Which of the three seeded sessions a row signs with, each holding one of the grants the rule
/// generalises over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Signer {
    /// Granted `execute-api:Invoke` on every `GET` of the stage.
    Granted,
    /// Granted it on another verb only.
    OtherVerb,
    /// Holding no identity-based document at all.
    Ungranted,
}

/// One request to the deployed stage: the session that signs it, the target the client wrote,
/// the value the method's parameter is bound to, and what the stage owes it.
struct Row {
    case: &'static str,
    signer: Signer,
    /// What the client wrote after the stage segment, verbatim.
    within_stage: String,
    /// The value the method's path parameter is bound to by that target.
    bound: &'static str,
    owed: Owed,
}

/// One AWS Lambda call over its `restJson1` wire, made while authentication is still off.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answered {
    let host = format!("lambda.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", &host)
        .header("content-type", "application/json")
        .header("authorization", credential_scope("lambda"))
        .body(
            body.map(|body| body.to_string().into_bytes())
                .unwrap_or_default(),
        )
        .expect("the test request is well formed");
    dispatch(router, request).await
}

/// Deploy the stack declaring the execution role and the function — the inline source is
/// archived into a deployment package by CloudFormation, which is how a function whose handler
/// is written here comes to exist at all — and answer the ARN Lambda reports for it once it
/// reports it `Active`.
async fn deployed_function(router: &MultiServiceRouter) -> String {
    let template = json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "Role": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "RoleName": EXECUTION_ROLE,
                    "AssumeRolePolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": { "Service": "lambda.amazonaws.com" },
                            "Action": "sts:AssumeRole",
                        }],
                    },
                },
            },
            "Reader": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": FUNCTION,
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    "Role": { "Fn::GetAtt": ["Role", "Arn"] },
                    "Timeout": FUNCTION_TIMEOUT,
                    "Code": { "ZipFile": PROXY_HANDLER },
                },
            },
        },
    })
    .to_string();
    query(
        router,
        "cloudformation",
        "2010-05-15",
        "CreateStack",
        &[
            ("StackName", STACK),
            ("TemplateBody", &template),
            ("Capabilities.member.1", "CAPABILITY_NAMED_IAM"),
        ],
    )
    .await;
    let (reached, described) = wait_for_stack_status(router, STACK, CREATE_COMPLETE).await;
    assert_eq!(
        reached, CREATE_COMPLETE,
        "setup CreateStack: expected the stack to reach {CREATE_COMPLETE}, got {described}"
    );

    // The invoke the integration makes needs the function RUNNABLE, which is a transition its
    // own service owes: look until it reports it rather than sleeping a span on its behalf.
    let mut configuration = Value::Null;
    for _ in 0..LOOKS {
        configuration = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{FUNCTION}/configuration"),
            None,
        )
        .await
        .json();
        if configuration.get("State").and_then(Value::as_str) == Some("Active") {
            break;
        }
        tokio::time::sleep(LOOK).await;
    }
    assert_eq!(
        configuration.get("State").and_then(Value::as_str),
        Some("Active"),
        "setup GetFunctionConfiguration: expected Lambda to report {FUNCTION:?} Active, got \
         {configuration}"
    );
    as_str("GetFunctionConfiguration", &configuration, "FunctionArn")
}

#[test]
fn a_session_signed_invoke_the_gate_admits_is_served_its_proxy_integrations_run() {
    std::thread::Builder::new()
        .name("apigateway-session-signed-proxy-run".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build a current-thread runtime");
            runtime.block_on(async {
                let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
                let mut router = build_router(engine);
                router.set_request_authentication(false);

                // The backend the stage fronts, and the api that fronts it: a `GET` on
                // `/{COLLECTION}/{ITEM_TEMPLATE}` behind `AWS_IAM`, proxied to the function.
                let function_arn = deployed_function(&router).await;
                let api = apigw(
                    &router,
                    "CreateRestApi",
                    "POST",
                    "/restapis",
                    Some(json!({
                        "name": "session-signed-proxy-api",
                        "endpointConfiguration": { "types": ["REGIONAL"] },
                    })),
                    201,
                )
                .await;
                let api_id = as_str("CreateRestApi", &api, "id");
                let root = as_str("CreateRestApi", &api, "rootResourceId");
                let collection = apigw(
                    &router,
                    "CreateResource(collection)",
                    "POST",
                    &format!("/restapis/{api_id}/resources/{root}"),
                    Some(json!({ "pathPart": COLLECTION })),
                    201,
                )
                .await;
                let collection_id = as_str("CreateResource(collection)", &collection, "id");
                let item = apigw(
                    &router,
                    "CreateResource(item)",
                    "POST",
                    &format!("/restapis/{api_id}/resources/{collection_id}"),
                    Some(json!({ "pathPart": ITEM_TEMPLATE })),
                    201,
                )
                .await;
                let item_id = as_str("CreateResource(item)", &item, "id");
                let base = format!("/restapis/{api_id}/resources/{item_id}/methods/GET");
                apigw(
                    &router,
                    "PutMethod",
                    "PUT",
                    &base,
                    Some(json!({ "authorizationType": "AWS_IAM" })),
                    201,
                )
                .await;
                apigw(
                    &router,
                    "PutIntegration",
                    "PUT",
                    &format!("{base}/integration"),
                    Some(json!({
                        "type": "AWS_PROXY",
                        // `integrationHttpMethod` travels under its `jsonName`, which is what
                        // `smithy_query(query="operation",operation="PutIntegration")` reports.
                        "httpMethod": "POST",
                        "uri": format!(
                            "arn:aws:apigateway:{REGION}:lambda:path/2015-03-31/functions/\
                             {function_arn}/invocations"
                        ),
                    })),
                    201,
                )
                .await;
                let stage_arn = format!("arn:aws:execute-api:{REGION}:{ACCOUNT}:{api_id}/{STAGE}");
                // The grant API Gateway's own invocation of the function is admitted by: it is
                // made as the service principal, narrowed to the methods of this stage.
                let permitted = lambda(
                    &router,
                    "POST",
                    &format!("/2015-03-31/functions/{FUNCTION}/policy"),
                    Some(json!({
                        "StatementId": "session-signed-stage",
                        "Action": "lambda:InvokeFunction",
                        "Principal": "apigateway.amazonaws.com",
                        "SourceArn": format!("{stage_arn}/GET/{COLLECTION}/*"),
                    })),
                )
                .await;
                assert_eq!(
                    permitted.status,
                    201,
                    "setup AddPermission: must answer HTTP 201, got {}",
                    permitted.summary()
                );
                apigw(
                    &router,
                    "CreateDeployment",
                    "POST",
                    &format!("/restapis/{api_id}/deployments"),
                    Some(json!({ "stageName": STAGE })),
                    201,
                )
                .await;

                let granted = session(
                    &router,
                    "xsvc-proxy-session-get",
                    Some(&allow(&format!("{stage_arn}/GET/*"))),
                )
                .await;
                let other_verb = session(
                    &router,
                    "xsvc-proxy-session-post",
                    Some(&allow(&format!("{stage_arn}/POST/*"))),
                )
                .await;

                router.set_request_authentication(true);
                let host = format!("{api_id}.execute-api.{REGION}.amazonaws.com");
                let target = format!("/{STAGE}//{COLLECTION}/{ITEM}");

                // The session the gate admits is served the RUN: the function is invoked as the
                // api's own service principal, which its resource policy admits, and never as
                // the caller — whose own documents allow nothing but the invoke.
                let served = dispatch(&router, signed_request(&granted, &host, &target)).await;
                assert_eq!(
                    served.status,
                    200,
                    "case \"the granted session, a doubled slash after the stage\": expected the \
                     handler's own 200, got {}",
                    served.summary()
                );
                assert_eq!(
                    served.json().get(ITEM_PARAMETER).and_then(Value::as_str),
                    Some(ITEM),
                    "case \"the granted session, a doubled slash after the stage\": expected the \
                     parameter the target bound, got {}",
                    served.summary()
                );

                // The session the gate refuses gets the refusal, about the method its target
                // selected, and no run at all.
                let refused = dispatch(&router, signed_request(&other_verb, &host, &target)).await;
                let message = refused.message();
                assert_eq!(
                    (refused.status, refused.error_type.as_deref()),
                    (403, Some(ACCESS_DENIED)),
                    "case \"the session granted another verb only\": expected the gate's 403 \
                     {ACCESS_DENIED}, got {}",
                    refused.summary()
                );
                assert!(
                    message.starts_with(&format!(
                        "User: arn:aws:sts::{ACCOUNT}:assumed-role/{}/{SESSION_NAME} \
                         {NOT_AUTHORIZED_CLAUSE} ",
                        other_verb.role
                    )) && message.ends_with(&format!(
                        ":{api_id}/{STAGE}/GET/{COLLECTION}/{ITEM} {IMPLICIT_DENY_CLAUSE}"
                    )),
                    "case \"the session granted another verb only\": expected the IAM \
                     authorization sentence for the method the target selected, got {}",
                    refused.summary()
                );
                assert!(
                    refused.json().get(ITEM_PARAMETER).is_none(),
                    "case \"the session granted another verb only\": the integration must not \
                     have been run, got {}",
                    refused.summary()
                );
            });
        })
        .expect("spawn the sized thread")
        .join()
        .expect("the suite thread must not panic");
}

#[test]
fn a_session_signed_stage_invoke_is_decided_about_the_method_its_target_selects() {
    std::thread::Builder::new()
        .name("apigateway-session-signed-stage-target".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build a current-thread runtime");
            runtime.block_on(async {
                let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
                let mut router = build_router(engine);

                // SETUP, with authentication off so the control-plane calls need no real
                // signature: one `AWS_IAM` `GET` on `/{COLLECTION}/{ITEM_TEMPLATE}` behind a
                // `MOCK` integration that echoes the parameter the target bound, deployed.
                router.set_request_authentication(false);
                let api = apigw(
                    &router,
                    "CreateRestApi",
                    "POST",
                    "/restapis",
                    Some(json!({ "name": "session-signed-target-api" })),
                    201,
                )
                .await;
                let api_id = as_str("CreateRestApi", &api, "id");
                let root = as_str("CreateRestApi", &api, "rootResourceId");
                let collection = apigw(
                    &router,
                    "CreateResource(collection)",
                    "POST",
                    &format!("/restapis/{api_id}/resources/{root}"),
                    Some(json!({ "pathPart": COLLECTION })),
                    201,
                )
                .await;
                let collection_id = as_str("CreateResource(collection)", &collection, "id");
                let item = apigw(
                    &router,
                    "CreateResource(item)",
                    "POST",
                    &format!("/restapis/{api_id}/resources/{collection_id}"),
                    Some(json!({ "pathPart": ITEM_TEMPLATE })),
                    201,
                )
                .await;
                let item_id = as_str("CreateResource(item)", &item, "id");
                let base = format!("/restapis/{api_id}/resources/{item_id}/methods/GET");
                apigw(
                    &router,
                    "PutMethod",
                    "PUT",
                    &base,
                    Some(json!({ "authorizationType": "AWS_IAM" })),
                    201,
                )
                .await;
                apigw(
                    &router,
                    "PutIntegration",
                    "PUT",
                    &format!("{base}/integration"),
                    Some(json!({ "type": "MOCK" })),
                    201,
                )
                .await;
                apigw(
                    &router,
                    "PutMethodResponse",
                    "PUT",
                    &format!("{base}/responses/200"),
                    Some(json!({})),
                    201,
                )
                .await;
                apigw(
                    &router,
                    "PutIntegrationResponse",
                    "PUT",
                    &format!("{base}/integration/responses/200"),
                    Some(json!({
                        "responseTemplates": {
                            "application/json": format!(
                                "{{\"{ITEM_PARAMETER}\":\"$input.params('{ITEM_PARAMETER}')\"}}"
                            ),
                        },
                    })),
                    201,
                )
                .await;
                apigw(
                    &router,
                    "CreateDeployment",
                    "POST",
                    &format!("/restapis/{api_id}/deployments"),
                    Some(json!({ "stageName": STAGE })),
                    201,
                )
                .await;

                // The three callers: one session per grant the rule generalises over.
                let stage_arn = format!("arn:aws:execute-api:{REGION}:{ACCOUNT}:{api_id}/{STAGE}");
                let granted = session(
                    &router,
                    "xsvc-session-get",
                    Some(&allow(&format!("{stage_arn}/GET/*"))),
                )
                .await;
                let other_verb = session(
                    &router,
                    "xsvc-session-post",
                    Some(&allow(&format!("{stage_arn}/POST/*"))),
                )
                .await;
                let ungranted = session(&router, "xsvc-session-none", None).await;

                // From here on the SAME router verifies request signatures, as the emulator does
                // when it is started with authentication on.
                router.set_request_authentication(true);

                let rows: Vec<Row> = vec![
                    Row {
                        case: "the granted session, the plain target",
                        signer: Signer::Granted,
                        within_stage: format!("/{COLLECTION}/{ITEM}"),
                        bound: ITEM,
                        owed: Owed::Served,
                    },
                    Row {
                        case: "the granted session, a doubled slash after the stage",
                        signer: Signer::Granted,
                        within_stage: format!("//{COLLECTION}/{ITEM}"),
                        bound: ITEM,
                        owed: Owed::Served,
                    },
                    Row {
                        case: "the granted session, a doubled slash inside the path",
                        signer: Signer::Granted,
                        within_stage: format!("/{COLLECTION}//{ITEM}"),
                        bound: ITEM,
                        owed: Owed::Served,
                    },
                    Row {
                        case: "the granted session, doubled slashes throughout",
                        signer: Signer::Granted,
                        within_stage: format!("//{COLLECTION}//{ITEM}"),
                        bound: ITEM,
                        owed: Owed::Served,
                    },
                    Row {
                        case: "the granted session, a trailing slash",
                        signer: Signer::Granted,
                        within_stage: format!("/{COLLECTION}/{ITEM}/"),
                        bound: ITEM,
                        owed: Owed::Served,
                    },
                    Row {
                        case: "the granted session at a sibling id",
                        signer: Signer::Granted,
                        within_stage: format!("//{COLLECTION}/{SIBLING_ITEM}"),
                        bound: SIBLING_ITEM,
                        owed: Owed::Served,
                    },
                    Row {
                        case: "the session granted another verb only, the plain target",
                        signer: Signer::OtherVerb,
                        within_stage: format!("/{COLLECTION}/{ITEM}"),
                        bound: ITEM,
                        owed: Owed::NotAuthorized,
                    },
                    Row {
                        case: "the session granted another verb only, a doubled slash after the \
                               stage",
                        signer: Signer::OtherVerb,
                        within_stage: format!("//{COLLECTION}/{ITEM}"),
                        bound: ITEM,
                        owed: Owed::NotAuthorized,
                    },
                    Row {
                        case: "the session granted another verb only, doubled slashes throughout",
                        signer: Signer::OtherVerb,
                        within_stage: format!("//{COLLECTION}//{ITEM}"),
                        bound: ITEM,
                        owed: Owed::NotAuthorized,
                    },
                    Row {
                        case: "the session granted another verb only, at a sibling id",
                        signer: Signer::OtherVerb,
                        within_stage: format!("//{COLLECTION}/{SIBLING_ITEM}"),
                        bound: SIBLING_ITEM,
                        owed: Owed::NotAuthorized,
                    },
                    Row {
                        case: "a session whose role holds no document at all",
                        signer: Signer::Ungranted,
                        within_stage: format!("//{COLLECTION}/{ITEM}"),
                        bound: ITEM,
                        owed: Owed::NotAuthorized,
                    },
                ];

                let host = format!("{api_id}.execute-api.{REGION}.amazonaws.com");
                for row in &rows {
                    let case = row.case;
                    let signer = match row.signer {
                        Signer::Granted => &granted,
                        Signer::OtherVerb => &other_verb,
                        Signer::Ungranted => &ungranted,
                    };
                    let target = format!("/{STAGE}{}", row.within_stage);
                    let answered =
                        dispatch(&router, signed_request(signer, &host, &target)).await;
                    match row.owed {
                        Owed::Served => {
                            assert_eq!(
                                answered.status,
                                200,
                                "case {case:?}: expected the integration's own 200, got {}",
                                answered.summary()
                            );
                            assert_eq!(
                                answered.json().get(ITEM_PARAMETER).and_then(Value::as_str),
                                Some(row.bound),
                                "case {case:?}: expected the parameter the target bound, got {}",
                                answered.summary()
                            );
                        }
                        Owed::NotAuthorized => {
                            let message = answered.message();
                            assert_eq!(
                                (answered.status, answered.error_type.as_deref()),
                                (403, Some(ACCESS_DENIED)),
                                "case {case:?}: expected the gate's 403 {ACCESS_DENIED}, got {}",
                                answered.summary()
                            );
                            let opens = format!(
                                "User: arn:aws:sts::{ACCOUNT}:assumed-role/{}/{SESSION_NAME} \
                                 {NOT_AUTHORIZED_CLAUSE} ",
                                signer.role
                            );
                            assert!(
                                message.starts_with(&opens),
                                "case {case:?}: expected the sentence to name the session \
                                 refused, got {}",
                                answered.summary()
                            );
                            // The resource the verdict is about is an `execute-api` ARN of this
                            // partition and Region, whose own fields the clause below pins.
                            assert!(
                                message.contains(&format!(
                                    "{NOT_AUTHORIZED_CLAUSE} arn:aws:execute-api:{REGION}:"
                                )),
                                "case {case:?}: expected the method ARN's partition and Region, \
                                 got {}",
                                answered.summary()
                            );
                            let closes = format!(
                                ":{api_id}/{STAGE}/GET/{COLLECTION}/{} {IMPLICIT_DENY_CLAUSE}",
                                row.bound
                            );
                            assert!(
                                message.ends_with(&closes),
                                "case {case:?}: expected the sentence to end naming the method \
                                 ARN the target selected ({closes:?}), got {}",
                                answered.summary()
                            );
                            assert!(
                                !message.contains(ITEM_TEMPLATE),
                                "case {case:?}: expected the addressed path rather than \
                                 {ITEM_TEMPLATE}, got {}",
                                answered.summary()
                            );
                        }
                    }
                }
            });
        })
        .expect("spawn the sized thread")
        .join()
        .expect("the suite thread must not panic");
}
