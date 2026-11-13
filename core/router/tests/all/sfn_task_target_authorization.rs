// SPDX-License-Identifier: CC0-1.0

//! The EXECUTION ROLE a Step Functions `Task` state's integration call is made as,
//! over the integrations whose request addresses its target by something other than
//! that resource's ARN — driven across the four services the question spans, mounted
//! on ONE engine exactly as `core/router/src/lib.rs` mounts them.
//!
//! RULE: Step Functions makes a `Task` state's integration call ACTING AS the state
//! machine's `roleArn`, so the identity-based policies in force on that role decide
//! whether it is made at all, and the access request it is judged by is the
//! integration's OWN — the IAM action the invoked operation PUBLISHES
//! (`lambda:InvokeFunction` for an invocation, which is not the operation's name) over
//! the ARN of the resource the request addresses, whether the request names it by that
//! ARN, by a queue's url or by a function's name. A call the policies do not allow is
//! NOT made: the task fails with the invoked service's own access-denied refusal,
//! reported under that service's base SDK exception, the execution ends `FAILED`, and
//! the resource the call would have acted on is untouched. A call they DO allow is
//! made, and what the task reports is the target service's own answer.
//!
//! Four crates have to agree for that to hold — Step Functions decides and makes the
//! call, IAM holds the documents it is judged against, and Amazon SQS and Lambda serve
//! the call when it is made — so the suite lives here rather than in any of them.
//!
//! The rows differ along what the rule generalises over: which integration the
//! `Resource` names, which family names it (the optimized one and the AWS SDK one,
//! which make the same call on the same resource), and the form the request addresses
//! the target in (a queue's url, a function's ARN, a function's bare name).
//!
//! The harness this suite builds — the four services on one engine, the role, the queue,
//! the state machine and the execution reader — is `pub(crate)`, because
//! `all/sfn_integration_error_names.rs` asks a second question of the same four
//! services and `tests/all.rs` is one crate root: one wiring, reached through
//! `crate::sfn_task_target_authorization`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The percent-encoder for an `application/x-www-form-urlencoded` component, which
// `all/cloudfront_key_value_store_handshake.rs` already owns, and the reader of one
// element of an awsQuery answer, which `all/query_wire.rs` already owns: `tests/all.rs`
// is one crate root, so this suite borrows both rather than carrying a second copy.
use crate::cloudfront_key_value_store_handshake::segment as form_encode;
use crate::query_wire::element;

/// The region every request below is signed for.
pub(crate) const REGION: &str = "us-west-2";

/// The account id every ARN this emulator mints carries.
pub(crate) const ACCOUNT: &str = "000000000000";

/// How long a row waits for an execution to leave `RUNNING`.
const PATIENCE: Duration = Duration::from_secs(20);

/// The fragment the cause of an access-denied refusal carries: the code the answer came
/// back under.
const REFUSAL_CODE: &str = "Error Code: AccessDeniedException";

/// The ARN of the SESSION a refusal of one of an execution's calls blames: Step Functions
/// holds no credentials of the state machine's `roleArn` — it ASSUMES the role for the
/// run's work — so the identity the call is made as is a session of that role, spelled the
/// way `sts:AssumeRole` reports one, and named after the state machine the work is for.
///
/// `pub(crate)` for the same reason the harness is: `all/sfn_athena_run_a_job_sync.rs` reads
/// the same refusal and `tests/all.rs` is one crate root, so the spelling is stated once.
pub(crate) fn session_principal(role_arn: &str, session_name: &str) -> String {
    let (head, named) = role_arn
        .split_once(":role/")
        .unwrap_or_else(|| panic!("expected an IAM role ARN, got {role_arn:?}"));
    // A session ARN carries the role's NAME, never the path IAM holds it under.
    let role_name = named.rsplit('/').next().unwrap_or(named);
    let fields: Vec<&str> = head.split(':').collect();
    let partition = fields.get(1).copied().unwrap_or_default();
    let account = fields.get(4).copied().unwrap_or_default();
    format!("arn:{partition}:sts::{account}:assumed-role/{role_name}/{session_name}")
}

/// The history event a failed task writes
/// (`smithy_query(query="shape",shape="HistoryEventType")`).
const TASK_FAILED: &str = "TaskFailed";

/// The four services this question spans, over ONE shared engine.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_sfn_awsstepfunctions::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked: this router runs with request
/// authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// Call one operation of an `awsJson` service, requiring a 200.
async fn rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header(
            "content-type",
            format!("application/x-amz-json-{json_version}"),
        )
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header("authorization", authorization(signing_name))
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "case {operation}: expected 200, got {status} with {parsed:?}"
    );
    parsed
}

pub(crate) async fn sfn(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    rpc(router, "states", "AWSStepFunctions", "1.0", operation, body).await
}

pub(crate) async fn sqs(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    rpc(router, "sqs", "AmazonSQS", "1.0", operation, body).await
}

/// The text a member of a response carries.
pub(crate) fn string(body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a {name} in {body}"))
        .to_string()
}

/// One awsQuery call to IAM, requiring a 200, answered with the XML body.
async fn iam(router: &MultiServiceRouter, action: &str, params: &[(&str, String)]) -> String {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("iam"))
        .body(Bytes::from(body))
        .expect("a well-formed awsQuery request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {action}: expected 200 from IAM, got {status} with {text}"
    );
    text
}

/// The trust document a Step Functions execution role carries, which is what lets that
/// service act as it.
fn trust_document() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "states.amazonaws.com" },
            "Action": "sts:AssumeRole"
        }]
    })
}

/// An IAM role carrying `policy` as its inline policy, by its own ARN.
pub(crate) async fn role(router: &MultiServiceRouter, name: &str, policy: &Value) -> String {
    let created = iam(
        router,
        "CreateRole",
        &[
            ("RoleName", name.to_string()),
            ("AssumeRolePolicyDocument", trust_document().to_string()),
        ],
    )
    .await;
    let arn = element(&created, "Arn");
    iam(
        router,
        "PutRolePolicy",
        &[
            ("RoleName", name.to_string()),
            ("PolicyName", "inline".to_string()),
            ("PolicyDocument", policy.to_string()),
        ],
    )
    .await;
    arn
}

/// A queue called `name`; answers its url and the ARN a policy names it by.
pub(crate) async fn queue(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let created = sqs(router, "CreateQueue", &json!({ "QueueName": name })).await;
    queue_handles(router, &created).await
}

/// The url and the ARN of the queue a `CreateQueue` answered.
pub(crate) async fn queue_handles(
    router: &MultiServiceRouter,
    created: &Value,
) -> (String, String) {
    let url = string(created, "QueueUrl");
    let attributes = sqs(
        router,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    let arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("GetQueueAttributes answers the QueueArn, got {attributes}"))
        .to_string();
    (url, arn)
}

/// How many messages are waiting on the queue at `url`.
pub(crate) async fn waiting(router: &MultiServiceRouter, url: &str) -> usize {
    let received = sqs(
        router,
        "ReceiveMessage",
        &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
    )
    .await;
    received["Messages"].as_array().map_or(0, Vec::len)
}

/// A definition whose one `Task` state makes `resource`'s call with `parameters`.
fn definition(resource: &str, parameters: &Value) -> String {
    json!({
        "StartAt": "Call",
        "States": {
            "Call": {
                "Type": "Task",
                "Resource": resource,
                "Parameters": parameters,
                "End": true,
            }
        }
    })
    .to_string()
}

/// Create a state machine under `role_arn` and return its ARN.
pub(crate) async fn machine(
    router: &MultiServiceRouter,
    name: &str,
    resource: &str,
    parameters: &Value,
    role_arn: &str,
) -> String {
    let created = sfn(
        router,
        "CreateStateMachine",
        &json!({
            "name": name,
            "definition": definition(resource, parameters),
            "roleArn": role_arn,
        }),
    )
    .await;
    string(&created, "stateMachineArn")
}

/// Start one execution and report it once it has left `RUNNING`.
pub(crate) async fn ran(
    router: &MultiServiceRouter,
    machine_arn: &str,
    case: &str,
) -> (String, Value) {
    ran_with_input(router, machine_arn, None, case).await
}

/// [`ran`] for an execution that carries an `input` document, which is what a definition
/// reading its states' parameters out of the execution's own data needs.
///
/// `pub(crate)` for the same reason the rest of this harness is: `tests/all.rs` is one
/// crate root, so the one `StartExecution` + poll-to-terminal loop every Step Functions
/// suite here drives an execution through lives in one place.
pub(crate) async fn ran_with_input(
    router: &MultiServiceRouter,
    machine_arn: &str,
    input: Option<&str>,
    case: &str,
) -> (String, Value) {
    let mut request = json!({ "stateMachineArn": machine_arn, "name": "only" });
    if let Some(document) = input {
        request["input"] = Value::String(document.to_string());
    }
    let started = sfn(router, "StartExecution", &request).await;
    let execution_arn = string(&started, "executionArn");
    let began = Instant::now();
    loop {
        let described = sfn(
            router,
            "DescribeExecution",
            &json!({ "executionArn": execution_arn }),
        )
        .await;
        if described.get("status").and_then(Value::as_str) != Some("RUNNING") {
            return (execution_arn, described);
        }
        assert!(
            began.elapsed() < PATIENCE,
            "case {case:?}: expected a terminal status within {PATIENCE:?}, got {described:?}"
        );
        ::tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The details of the one `TaskFailed` event of an execution's history.
pub(crate) async fn task_failure(
    router: &MultiServiceRouter,
    execution_arn: &str,
    case: &str,
) -> Value {
    let answered = sfn(
        router,
        "GetExecutionHistory",
        &json!({ "executionArn": execution_arn, "maxResults": 100 }),
    )
    .await;
    let events = answered
        .get("events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let failed: Vec<&Value> = events
        .iter()
        .filter(|event| event.get("type").and_then(Value::as_str) == Some(TASK_FAILED))
        .collect();
    assert_eq!(
        failed.len(),
        1,
        "case {case:?}: expected exactly one {TASK_FAILED}, got {events:?}"
    );
    failed[0]
        .get("taskFailedEventDetails")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected taskFailedEventDetails"))
}

/// A statement allowing `action` on `resource`.
pub(crate) fn allow(action: &str, resource: &str) -> Value {
    json!({ "Effect": "Allow", "Action": action, "Resource": resource })
}

/// The statement every role below carries whatever else it does: the log writes a state
/// machine makes, which say nothing about any integration call.
pub(crate) fn logs_only() -> Value {
    json!({
        "Effect": "Allow",
        "Action": ["logs:CreateLogDelivery", "logs:PutLogEvents"],
        "Resource": "*",
    })
}

/// One row: the case, the `Resource` the `Task` names, its parameters, the IAM action the
/// refusal must name, and the error name a `Task` state reports for it.
struct Row {
    case: &'static str,
    resource: &'static str,
    parameters: Value,
    action: &'static str,
    refusal_error: &'static str,
}

/// A call whose action the execution role is not granted is refused, with the invoked
/// service's own access-denied sentence, and the resource it would have acted on is
/// untouched.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_integration_call_the_role_is_not_granted_is_not_made() {
    let router = emulator();
    let (queue_url, queue_arn) = queue(&router, "denied-target").await;
    // The function the invocation rows name. It need not exist for a DENIED row —
    // authorization is settled before the call — and the granted test below reads
    // Lambda's own answer for it, which is what shows the call was really made.
    let function_arn = format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:absent-handler");
    let role_arn = role(
        &router,
        "TargetsDeniedRole",
        &json!({ "Version": "2012-10-17", "Statement": [logs_only()] }),
    )
    .await;

    let rows = [
        Row {
            case: "a send to a queue, addressed by its url",
            resource: "arn:aws:states:::sqs:sendMessage",
            parameters: json!({ "QueueUrl": queue_url.clone(), "MessageBody": "hello" }),
            action: "sqs:SendMessage",
            refusal_error: "SQS.AmazonSQSException",
        },
        Row {
            // The other family makes the same call on the same queue, so the refusal
            // must hold through it too.
            case: "the same send through the AWS SDK integration",
            resource: "arn:aws:states:::aws-sdk:sqs:sendMessage",
            parameters: json!({ "QueueUrl": queue_url.clone(), "MessageBody": "hello" }),
            action: "sqs:SendMessage",
            refusal_error: "SQS.AmazonSQSException",
        },
        Row {
            case: "an invocation, the function named by its ARN",
            resource: "arn:aws:states:::lambda:invoke",
            parameters: json!({ "FunctionName": function_arn.clone(), "Payload": {} }),
            action: "lambda:InvokeFunction",
            refusal_error: "Lambda.AWSLambdaException",
        },
        Row {
            case: "the same invocation through the AWS SDK integration",
            resource: "arn:aws:states:::aws-sdk:lambda:invoke",
            parameters: json!({ "FunctionName": function_arn.clone(), "Payload": {} }),
            action: "lambda:InvokeFunction",
            refusal_error: "Lambda.AWSLambdaException",
        },
        Row {
            // A bare function name names the same function, so it is the same access
            // request: the ARN is built in the Region the call is made in.
            case: "the same invocation, the function named by its bare name",
            resource: "arn:aws:states:::lambda:invoke",
            parameters: json!({ "FunctionName": "absent-handler" }),
            action: "lambda:InvokeFunction",
            refusal_error: "Lambda.AWSLambdaException",
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let machine_name = format!("denied-{index}");
        let machine_arn = machine(
            &router,
            &machine_name,
            row.resource,
            &row.parameters,
            &role_arn,
        )
        .await;
        let (execution_arn, described) = ran(&router, &machine_arn, case).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("FAILED"),
            "case {case:?}: expected FAILED, got {described:?}"
        );
        assert_eq!(
            described.get("error").and_then(Value::as_str),
            Some(row.refusal_error),
            "case {case:?}: expected {:?}, got {described:?}",
            row.refusal_error
        );
        let details = task_failure(&router, &execution_arn, case).await;
        let cause = string(&details, "cause");
        assert!(
            cause.contains(REFUSAL_CODE),
            "case {case:?}: expected the cause to carry {REFUSAL_CODE:?}, got {cause:?}"
        );
        assert!(
            cause.contains(row.action),
            "case {case:?}: expected the cause to name the action {:?}, got {cause:?}",
            row.action
        );
        // The principal is the SESSION of the role the call was made as, named after the
        // state machine the work is for.
        let principal = session_principal(&role_arn, &machine_name);
        assert!(
            cause.contains(&principal),
            "case {case:?}: expected the cause to name the principal {principal}, got {cause:?}"
        );
        // The resource it was refused ON is the ARN of what the request addressed,
        // whichever form the request named it in.
        let resource = match row.action {
            "sqs:SendMessage" => queue_arn.clone(),
            _ => function_arn.clone(),
        };
        assert!(
            cause.contains(&format!("on resource: {resource} ")),
            "case {case:?}: expected the cause to name the resource {resource}, got {cause:?}"
        );
        assert_eq!(
            waiting(&router, &queue_url).await,
            0,
            "case {case:?}: expected the queue the role may not send to to be untouched"
        );
    }
}

/// A call whose action the role IS granted is made, and what the task reports is the
/// target service's own answer.
///
/// The two halves are the two integrations: the send, whose answer is the message
/// waiting on the queue, and the invocation, whose answer is Lambda's own — the function
/// named does not exist, and that refusal is what shows the request reached Lambda's
/// `Invoke` rather than being stopped by the authorization gate.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_integration_call_the_role_is_granted_is_made() {
    let router = emulator();
    let (queue_url, queue_arn) = queue(&router, "granted-target").await;
    let function_arn = format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:absent-handler");
    let role_arn = role(
        &router,
        "TargetsGrantedRole",
        &json!({
            "Version": "2012-10-17",
            "Statement": [
                logs_only(),
                allow("sqs:SendMessage", &queue_arn),
                allow("lambda:InvokeFunction", &function_arn),
            ],
        }),
    )
    .await;

    let send = machine(
        &router,
        "granted-send",
        "arn:aws:states:::sqs:sendMessage",
        &json!({ "QueueUrl": queue_url.clone(), "MessageBody": "hello" }),
        &role_arn,
    )
    .await;
    let case = "a send the role is granted";
    let (_execution_arn, described) = ran(&router, &send, case).await;
    assert_eq!(
        described.get("status").and_then(Value::as_str),
        Some("SUCCEEDED"),
        "case {case:?}: expected SUCCEEDED, got {described:?}"
    );
    assert_eq!(
        waiting(&router, &queue_url).await,
        1,
        "case {case:?}: expected the message the task sent"
    );

    let invoke = machine(
        &router,
        "granted-invoke",
        "arn:aws:states:::lambda:invoke",
        &json!({ "FunctionName": function_arn.clone(), "Payload": {} }),
        &role_arn,
    )
    .await;
    let case = "an invocation the role is granted, of a function that does not exist";
    let (execution_arn, described) = ran(&router, &invoke, case).await;
    assert_eq!(
        described.get("status").and_then(Value::as_str),
        Some("FAILED"),
        "case {case:?}: expected FAILED, got {described:?}"
    );
    assert_eq!(
        described.get("error").and_then(Value::as_str),
        Some("Lambda.ResourceNotFoundException"),
        "case {case:?}: expected Lambda's own refusal of the call, got {described:?}"
    );
    let details = task_failure(&router, &execution_arn, case).await;
    let cause = string(&details, "cause");
    assert!(
        !cause.contains(REFUSAL_CODE),
        "case {case:?}: expected no access denial once the role is granted, got {cause:?}"
    );
    assert!(
        cause.contains(&function_arn),
        "case {case:?}: expected Lambda's answer to name the function it looked for, got {cause:?}"
    );
}
