// SPDX-License-Identifier: CC0-1.0

//! What a RUNNING handler reaches when it builds an SDK client for the Bedrock Agent
//! BUILD-TIME plane, end to end over the wire: AWS Lambda, both Bedrock Agent planes, IAM and
//! the token service on ONE engine, and a `python3.x` function whose code does `import boto3`
//! and calls `bedrock-agent`.
//!
//! RULE: a managed runtime ships the AWS SDK with the service data of every service its call
//! seam reaches, so a client a handler builds for one of them CONSTRUCTS and its call is
//! dispatched to that service, under the operations THAT service's own model declares — and
//! what the handler observes is the service's own answer, each modelled outcome class
//! included: the authorization refusal a role without the grant for that operation's own
//! action gets, whether or not the resource it named exists; the not-found refusal a
//! well-formed identifier that resolves to nothing gets; the validation refusal a malformed
//! one gets; and the served answer, whose effect a client reads back through the service's own
//! API. A client-side failure of the SDK itself (`UnknownServiceError`, raised before anything
//! is signed) is none of those: it tells the handler nothing about the service, so a role that
//! is merely under-permissioned must not look like a broken runtime image.
//!
//! The rows differ along what the rule generalises over: WHICH operation of this plane is
//! called (an agent read, a knowledge-base read, a flow read and a flow create), which
//! resource kind the action is therefore decided on (`agent`, `knowledge-base`, `flow`, and
//! the whole collection for a create), which form the identifier takes
//! (well-formed-and-absent, malformed, and one naming a resource that really exists), and
//! which outcome class the call lands in (403 denial, 404 not-found, 400 validation and two
//! served calls). Every row runs as a role carrying the AWS-managed logging policy a Lambda
//! execution role carries, beside the statement the row itself puts in force.
//!
//! The RUNTIME plane's half of the same rule — the `bedrock-agent-runtime` token, its agent
//! invocation and its knowledge-base retrieval — is pinned by `handler_bedrock_agent_sdk.rs`,
//! and the negative half (a token NO service answers to still raises `UnknownServiceError`)
//! by `handler_sdk_service_inventory.rs`; neither is repeated here. That first suite's harness
//! is IMPORTED rather than copied, so the two decide the same question the same way.
//!
//! The suite RUNS handlers, so it takes the target's one invocation stream
//! ([`crate::invoke_request_id::one_invocation_stream_at_a_time`]): this emulator serves at
//! most `max_concurrent_runs` invocations at once for the whole process.

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mappings::{await_function_active, call, router_on};
// The one .zip writer this crate's suites package a handler with, exported by the suite that
// first needed it rather than written again here.
use crate::node_invoke_run::package;
// The harness the RUNTIME plane's suite already drives this same rule with: the Region every
// request is signed for, the SigV4 header that routes one, IAM's own operations for the
// execution role and the policy in force on it, and the sentence IAM refuses a request with.
// Imported rather than copied, so the two suites decide the same question the same way.
use crate::handler_bedrock_agent_sdk::{
    allow, authorization, denial, execution_role, iam, logs_only, role_policy, session_principal,
    REGION,
};

/// Where `CreateFunction` and `Invoke` are bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The module and attribute the package below carries its handler as.
const HANDLER: &str = "lambda_function.lambda_handler";

/// The AWS-managed policy a Lambda execution role carries for its own logging, attached to the
/// role every row runs as: its document — which IAM's own catalog publishes — allows the three
/// CloudWatch Logs actions and nothing else, so what a row's call is decided by is the
/// statement the row itself put in force.
const LOGGING_POLICY: &str = "arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole";

/// The token the AWS SDK for Python builds a client for this plane with, which is this
/// service's own endpoint prefix, and the signing name every Bedrock-family endpoint shares.
const BUILD_PLANE_TOKEN: &str = "bedrock-agent";
const BEDROCK_SIGNING_NAME: &str = "bedrock";

/// The four IAM actions the calls below are decided against, as a policy statement spells them:
/// `bedrock:<OperationName>`, under this plane's own ARN namespace.
const GET_AGENT_ACTION: &str = "bedrock:GetAgent";
const GET_KNOWLEDGE_BASE_ACTION: &str = "bedrock:GetKnowledgeBase";
const GET_FLOW_ACTION: &str = "bedrock:GetFlow";
const CREATE_FLOW_ACTION: &str = "bedrock:CreateFlow";

/// The error code and status AWS states an authorization failure with, and the two refusal
/// classes a caller tells a missing grant from an unusable identifier by.
const ACCESS_DENIED: &str = "AccessDeniedException";
const ACCESS_DENIED_STATUS: i64 = 403;
const NOT_FOUND: &str = "ResourceNotFoundException";
const NOT_FOUND_STATUS: i64 = 404;
const VALIDATION: &str = "ValidationException";
const VALIDATION_STATUS: i64 = 400;

/// A well-formed agent identifier no agent was ever created under, and a well-formed knowledge
/// base identifier nothing holds: ten characters of this plane's `Id` alphabet
/// (`smithy_query(query="operation",operation="GetAgent")` -> `agentId` @pattern
/// `^[0-9a-zA-Z]{10}$`, the same shape `GetKnowledgeBase`'s `knowledgeBaseId` targets).
///
/// [`STATED_ID_PATTERN`] is that expression as a refusal QUOTES it: without the whole-value
/// `^` / `$` the model writes around it, which is how that plane states a `@pattern`
/// requirement.
const STATED_ID_PATTERN: &str = "[0-9a-zA-Z]{10}";
const MISSING_AGENT_ID: &str = "ZZZZZZZZZ9";
const MISSING_KNOWLEDGE_BASE_ID: &str = "ZZZZZZZZZZ";

/// An agent identifier outside that alphabet, which names no agent at all.
const MALFORMED_AGENT_ID: &str = "not-an-agent-id";

/// The flow this suite creates before the rows run, as an operator creates one, so a row can
/// address a resource of this plane that really exists (`CreateFlow`'s `name` @pattern
/// `^([0-9a-zA-Z][_-]?){1,100}$`).
const SETUP_FLOW_NAME: &str = "handler-read-flow";

/// The flow the served create row has the HANDLER create, under a name of its own so the two
/// are told apart where a client reads them.
const CREATED_FLOW_NAME: &str = "handler-made-flow";

/// The version a flow's draft carries (`smithy_query(query="operation",operation="GetFlow")` ->
/// `version` targets `DraftVersion`, @pattern `^DRAFT$`), and the status `CreateFlow` states
/// for a flow it has just accepted ("When you submit this request, the status will be
/// NotPrepared", `CreateFlow`).
const DRAFT_VERSION: &str = "DRAFT";
const FLOW_NOT_PREPARED: &str = "NotPrepared";

/// Lambda, both Bedrock Agent planes, IAM and the token service over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them: the handler's SDK call is dispatched back into this
/// same router and signed as the session of its own execution role, so the service that
/// answers it is the service this suite reads. Both planes are mounted because they share the
/// `bedrock` signing name and three of their path segments, so a call reaching the BUILD plane
/// is a call the routing really told apart.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = router_on(&engine);
    router.register_service(
        service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_bedrockagentruntime_amazonbedrockagentruntimeservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(engine),
    );
    router
}

/// One `restJson1` call to the Bedrock Agent build-time plane, made as a CLIENT makes it, on
/// the method and URI the operation's own `@http` binding names — so what this suite sets up,
/// and what it reads a handler's work back through, is that service's own API.
async fn bedrock_agent(
    router: &MultiServiceRouter,
    what: &str,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Value {
    let host = format!("{BUILD_PLANE_TOKEN}.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header("authorization", authorization(BEDROCK_SIGNING_NAME))
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(
            body.map(|value| serde_json::to_vec(value).expect("the request body serializes"))
                .unwrap_or_default(),
        ))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert!(
        (200..300).contains(&status),
        "{what}: expected {method} {path} to be served, got {status} ({parsed})"
    );
    parsed
}

/// The ARN of a resource of this plane, as an identity-based policy for one of its actions
/// names it: `arn:aws:bedrock:<region>:<account>:<type>/<id>`, with the types this plane's own
/// ARNs spell (`agent`, `knowledge-base`, `flow`). Built here rather than read off the crate
/// under test.
fn plane_arn(resource_type: &str, identifier: &str) -> String {
    format!(
        "arn:aws:bedrock:{REGION}:{}:{resource_type}/{identifier}",
        engine::emulator_account_id()
    )
}

/// The sentence a request whose member misses that member's `@pattern` is refused with: AWS's
/// own validation wording, carrying the value the request sent, the member it was bound to and
/// the pattern it had to satisfy — all three taken from the request and the model.
fn pattern_violation(value: &str, member: &str, pattern: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{member}' failed to satisfy \
         constraint: Member must satisfy regular expression pattern: {pattern}"
    )
}

/// The handler every row runs: it builds a client for the build-time plane, makes the one call
/// the event names, and reports what the SDK gave it — the answer, or the error code, message
/// and status of a refusal.
///
/// A client-side failure of the SDK is reported under the exception class that was raised, so a
/// row that never reached a service is distinguishable from one that was refused by it.
const HANDLER_SOURCE: &str = r#"
import boto3
import botocore.exceptions


def lambda_handler(event, context):
    client = boto3.client(event["service"])
    try:
        if event["call"] == "get_agent":
            read = client.get_agent(agentId=event["agentId"])
            answered = {
                "agentId": read["agent"]["agentId"],
                "agentName": read["agent"]["agentName"],
            }
        elif event["call"] == "get_knowledge_base":
            read = client.get_knowledge_base(knowledgeBaseId=event["knowledgeBaseId"])
            answered = {"name": read["knowledgeBase"]["name"]}
        elif event["call"] == "get_flow":
            read = client.get_flow(flowIdentifier=event["flowIdentifier"])
            answered = {
                "id": read["id"],
                "name": read["name"],
                "version": read["version"],
            }
        else:
            created = client.create_flow(
                name=event["flow"], executionRoleArn=event["executionRoleArn"]
            )
            answered = {
                "id": created["id"],
                "name": created["name"],
                "arn": created["arn"],
                "status": created["status"],
                "version": created["version"],
            }
    except botocore.exceptions.ClientError as error:
        return {
            "ok": False,
            "raised": "ClientError",
            "code": error.response["Error"]["Code"],
            "message": error.response["Error"]["Message"],
            "status": error.response["ResponseMetadata"]["HTTPStatusCode"],
        }
    except botocore.exceptions.BotoCoreError as error:
        return {"ok": False, "raised": type(error).__name__, "message": str(error)}
    return {"ok": True, "answered": answered}
"#;

/// What an authorization refusal must name: the IAM action the call is decided against, and the
/// ARN of the resource it is decided on, given the flow this suite created.
type DeniedOn = (&'static str, fn(&str) -> String);

/// What the sentence a refusal carries must be. Every form is built from the request and the
/// account, never copied off a recording.
enum Message {
    /// IAM's own denial sentence, about this action on this resource.
    Denial(DeniedOn),
    /// The service's own sentence about an identifier that resolved to nothing, which must name
    /// the identifier the REQUEST carried — the one value in it the request decides.
    Names(&'static str),
    /// AWS's validation sentence, whole: the value the request carried, the member it was bound
    /// to, and the constraint that member missed.
    Exactly(fn() -> String),
}

/// What a row's call must come back as.
enum Expected {
    /// The handler read the flow this suite created, through this plane's own `GetFlow`.
    ReadTheFlow,
    /// The handler CREATED a flow, read back afterwards through this plane's own `GetFlow`.
    CreatedAFlow,
    /// The service refused the call with this error code, HTTP status and sentence.
    Refused {
        code: &'static str,
        status: i64,
        message: Message,
    },
}

/// One row: the grant in force, the call the handler makes, and what it must observe.
struct Row {
    case: &'static str,
    /// The inline policy the execution role carries when the handler calls.
    policy: fn() -> String,
    /// The event the handler is invoked with, given the flow this suite created and the ARN of
    /// the role a created flow names.
    event: fn(&str, &str) -> Value,
    expected: Expected,
}

/// The rows, spanning this plane's operations, the resource kinds its actions are decided on,
/// the identifier forms and every outcome class a handler has to be able to tell apart.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an agent read by a role granted no action of this plane",
            policy: logs_only,
            event: |_flow, _role| {
                json!({
                    "service": BUILD_PLANE_TOKEN,
                    "call": "get_agent",
                    "agentId": MISSING_AGENT_ID,
                })
            },
            expected: Expected::Refused {
                code: ACCESS_DENIED,
                status: ACCESS_DENIED_STATUS,
                message: Message::Denial((GET_AGENT_ACTION, |_flow| {
                    plane_arn("agent", MISSING_AGENT_ID)
                })),
            },
        },
        Row {
            case: "an agent read by a granted role, on an agent nobody created",
            policy: || allow(&[GET_AGENT_ACTION]),
            event: |_flow, _role| {
                json!({
                    "service": BUILD_PLANE_TOKEN,
                    "call": "get_agent",
                    "agentId": MISSING_AGENT_ID,
                })
            },
            expected: Expected::Refused {
                code: NOT_FOUND,
                status: NOT_FOUND_STATUS,
                message: Message::Names(MISSING_AGENT_ID),
            },
        },
        Row {
            case: "an agent read by a granted role, on a malformed agent identifier",
            policy: || allow(&[GET_AGENT_ACTION]),
            event: |_flow, _role| {
                json!({
                    "service": BUILD_PLANE_TOKEN,
                    "call": "get_agent",
                    "agentId": MALFORMED_AGENT_ID,
                })
            },
            expected: Expected::Refused {
                code: VALIDATION,
                status: VALIDATION_STATUS,
                message: Message::Exactly(|| {
                    pattern_violation(MALFORMED_AGENT_ID, "agentId", STATED_ID_PATTERN)
                }),
            },
        },
        Row {
            case: "a knowledge-base read by a role granted no action of this plane",
            policy: logs_only,
            event: |_flow, _role| {
                json!({
                    "service": BUILD_PLANE_TOKEN,
                    "call": "get_knowledge_base",
                    "knowledgeBaseId": MISSING_KNOWLEDGE_BASE_ID,
                })
            },
            expected: Expected::Refused {
                code: ACCESS_DENIED,
                status: ACCESS_DENIED_STATUS,
                message: Message::Denial((GET_KNOWLEDGE_BASE_ACTION, |_flow| {
                    plane_arn("knowledge-base", MISSING_KNOWLEDGE_BASE_ID)
                })),
            },
        },
        Row {
            case: "a knowledge-base read by a granted role, on a knowledge base nobody created",
            policy: || allow(&[GET_KNOWLEDGE_BASE_ACTION]),
            event: |_flow, _role| {
                json!({
                    "service": BUILD_PLANE_TOKEN,
                    "call": "get_knowledge_base",
                    "knowledgeBaseId": MISSING_KNOWLEDGE_BASE_ID,
                })
            },
            expected: Expected::Refused {
                code: NOT_FOUND,
                status: NOT_FOUND_STATUS,
                message: Message::Names(MISSING_KNOWLEDGE_BASE_ID),
            },
        },
        Row {
            case: "a flow read by a role granted another action of this plane, on a flow that \
                   exists",
            policy: || allow(&[GET_AGENT_ACTION]),
            event: |flow, _role| {
                json!({
                    "service": BUILD_PLANE_TOKEN,
                    "call": "get_flow",
                    "flowIdentifier": flow,
                })
            },
            expected: Expected::Refused {
                code: ACCESS_DENIED,
                status: ACCESS_DENIED_STATUS,
                message: Message::Denial((GET_FLOW_ACTION, |flow| plane_arn("flow", flow))),
            },
        },
        Row {
            case: "a flow read by a granted role, on the flow this account holds",
            policy: || allow(&[GET_FLOW_ACTION]),
            event: |flow, _role| {
                json!({
                    "service": BUILD_PLANE_TOKEN,
                    "call": "get_flow",
                    "flowIdentifier": flow,
                })
            },
            expected: Expected::ReadTheFlow,
        },
        Row {
            case: "a flow created by a granted role",
            policy: || allow(&[CREATE_FLOW_ACTION]),
            event: |_flow, role| {
                json!({
                    "service": BUILD_PLANE_TOKEN,
                    "call": "create_flow",
                    "flow": CREATED_FLOW_NAME,
                    "executionRoleArn": role,
                })
            },
            expected: Expected::CreatedAFlow,
        },
    ]
}

/// RULE: a client a handler builds for the Bedrock Agent build-time plane is CONSTRUCTED and
/// its call dispatched under that plane's own operations, so what the handler observes is that
/// service's own answer — the authorization refusal for the operation's own action, the
/// not-found refusal, the validation refusal or the served answer — and never a client-side
/// failure of the SDK.
#[tokio::test]
async fn a_handler_calls_the_bedrock_agent_build_plane_through_the_runtimes_sdk() {
    let _stream = crate::invoke_request_id::one_invocation_stream_at_a_time().await;
    let router = emulator();
    let role_name = "lambda-bedrock-agent-build-caller";
    let function_name = "bedrock-agent-build-sdk-caller";
    let role = execution_role(&router, role_name, &logs_only()).await;
    // The attachment a Lambda execution role carries for its own logging, put on with IAM's
    // own operation: it is in force for every row, and grants no action of this plane.
    iam(
        &router,
        "the attached logging policy",
        "AttachRolePolicy",
        &[
            ("RoleName", role_name.to_string()),
            ("PolicyArn", LOGGING_POLICY.to_string()),
        ],
    )
    .await;

    let created = call(
        &router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": function_name,
            "Role": role,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": 25,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(&[("lambda_function.py", HANDLER_SOURCE)])),
            },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "setup: expected CreateFunction to answer 201, got {} {}",
        created.status,
        created.text()
    );
    await_function_active(&router, "setup", function_name).await;
    let principal = session_principal(role_name, function_name);

    // The one resource of this plane that exists before any row runs, created through the
    // plane's own `CreateFlow` as an operator creates it: the rows that address a resource
    // which really resolves address this one.
    let setup = bedrock_agent(
        &router,
        "setup: the flow a row reads",
        "POST",
        "/flows/",
        Some(&json!({ "name": SETUP_FLOW_NAME, "executionRoleArn": role })),
    )
    .await;
    let flow_id = setup["id"].as_str().unwrap_or_default().to_string();
    assert!(
        !flow_id.is_empty(),
        "setup: expected CreateFlow to mint a flowId, got {setup}"
    );

    // The flow the served create row has the handler create, as the handler reported it: read
    // back through this plane's own API once the rows are done.
    let mut handler_made_flow: Option<Value> = None;

    for row in rows() {
        let case = row.case;
        role_policy(&router, case, role_name, &(row.policy)()).await;
        let answered = call(
            &router,
            "POST",
            &format!("{FUNCTIONS_URI}/{function_name}/invocations"),
            &(row.event)(&flow_id, &role),
        )
        .await;
        assert_eq!(
            answered.status,
            200,
            "case {case:?}: expected HTTP 200 from Invoke, got {} {}",
            answered.status,
            answered.text()
        );
        assert_eq!(
            answered.header("x-amz-function-error"),
            None,
            "case {case:?}: the handler must run to completion, got {}",
            answered.text()
        );
        let payload = answered.json();
        // Whatever the outcome, the CLIENT was built: the SDK raised no unknown-service failure
        // of its own, which is what a missing service model in the runtime's SDK looks like from
        // inside a handler.
        assert_ne!(
            payload["raised"].as_str(),
            Some("UnknownServiceError"),
            "case {case:?}: expected the client to be built for {BUILD_PLANE_TOKEN:?}, got \
             {payload}"
        );
        match row.expected {
            Expected::ReadTheFlow => {
                assert_eq!(
                    payload["ok"],
                    json!(true),
                    "case {case:?}: expected the read to be served, got {payload}"
                );
                assert_eq!(
                    payload["answered"]["id"],
                    json!(flow_id),
                    "case {case:?}: expected the flow this account holds, got {payload}"
                );
                assert_eq!(
                    payload["answered"]["name"],
                    json!(SETUP_FLOW_NAME),
                    "case {case:?}: expected that flow's own name, got {payload}"
                );
                assert_eq!(
                    payload["answered"]["version"],
                    json!(DRAFT_VERSION),
                    "case {case:?}: expected the draft version, got {payload}"
                );
            }
            Expected::CreatedAFlow => {
                assert_eq!(
                    payload["ok"],
                    json!(true),
                    "case {case:?}: expected the create to be served, got {payload}"
                );
                assert_eq!(
                    payload["answered"]["name"],
                    json!(CREATED_FLOW_NAME),
                    "case {case:?}: expected the name the handler asked for, got {payload}"
                );
                assert_eq!(
                    payload["answered"]["status"],
                    json!(FLOW_NOT_PREPARED),
                    "case {case:?}: expected an accepted create to report {FLOW_NOT_PREPARED}, \
                     got {payload}"
                );
                let made = payload["answered"]["id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                assert!(
                    !made.is_empty(),
                    "case {case:?}: expected the service's own minted flowId, got {payload}"
                );
                assert_ne!(
                    made, flow_id,
                    "case {case:?}: expected a flow of its own rather than the one this suite \
                     created, got {payload}"
                );
                assert_eq!(
                    payload["answered"]["arn"],
                    json!(plane_arn("flow", &made)),
                    "case {case:?}: expected that flow's own ARN, got {payload}"
                );
                handler_made_flow = Some(payload["answered"].clone());
            }
            Expected::Refused {
                code,
                status,
                message,
            } => {
                assert_eq!(
                    payload["ok"],
                    json!(false),
                    "case {case:?}: expected the service's own refusal, got {payload}"
                );
                assert_eq!(
                    payload["raised"],
                    json!("ClientError"),
                    "case {case:?}: expected the refusal to reach the handler as the SDK's \
                     service error, got {payload}"
                );
                assert_eq!(
                    payload["code"],
                    json!(code),
                    "case {case:?}: expected the refusal {code:?}, got {payload}"
                );
                assert_eq!(
                    payload["status"],
                    json!(status),
                    "case {case:?}: expected HTTP {status} for that refusal, got {payload}"
                );
                match message {
                    Message::Denial((action, resource)) => assert_eq!(
                        payload["message"],
                        json!(denial(&principal, action, &resource(&flow_id))),
                        "case {case:?}: expected IAM's own denial sentence, got {payload}"
                    ),
                    Message::Names(identifier) => assert!(
                        payload["message"]
                            .as_str()
                            .is_some_and(|sentence| sentence.contains(identifier)),
                        "case {case:?}: expected the refusal to name {identifier:?}, the \
                         identifier the request carried, got {payload}"
                    ),
                    Message::Exactly(sentence) => assert_eq!(
                        payload["message"],
                        json!(sentence()),
                        "case {case:?}: expected AWS's validation sentence, got {payload}"
                    ),
                }
            }
        }
    }

    // The served create did the work it reported: the flow the handler created is the flow the
    // build-time plane itself holds, read back through that plane's own `GetFlow`.
    let made = handler_made_flow.expect("the served create row ran");
    let made_id = made["id"].as_str().expect("the handler reported a flowId");
    let read = bedrock_agent(
        &router,
        "the flow the handler created",
        "GET",
        &format!("/flows/{made_id}/"),
        None,
    )
    .await;
    assert_eq!(
        read["id"],
        json!(made_id),
        "the flow the handler created must be the one this plane holds, got {read}"
    );
    assert_eq!(
        read["name"],
        json!(CREATED_FLOW_NAME),
        "the stored flow must carry the name the handler asked for, got {read}"
    );
    assert_eq!(
        read["executionRoleArn"],
        json!(role),
        "the stored flow must carry the role the handler named, got {read}"
    );
    assert_eq!(
        read["version"],
        json!(DRAFT_VERSION),
        "the stored flow must be the draft, got {read}"
    );
}
