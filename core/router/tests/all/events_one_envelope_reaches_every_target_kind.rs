// SPDX-License-Identifier: CC0-1.0

//! ONE published event reaching targets of THREE different services on the same rule, and the
//! envelope each of them is handed.
//!
//! The rule pinned here: **every target of a rule whose `EventPattern` matched is handed the
//! SAME event — one envelope, carrying the `id` the publication was answered with and the
//! `source`, `detail-type` and `detail` as submitted — whatever service that target names and
//! whichever order the targets are written in; so a client reading what arrived at an Amazon
//! SQS queue target, at the input a Step Functions state-machine target's execution was
//! started with, and at what a Lambda function target's handler was invoked with reads one and
//! the same document.**
//!
//! Five crates have to agree for that to hold (EventBridge matches and hands over, IAM answers
//! for the role a state-machine handover is made under, CloudFormation archives the inline
//! handler into the function's deployment package, Lambda decides the invocation against the
//! function's own policy and RUNS the handler, Step Functions starts the execution, and Amazon
//! SQS holds both what the queue target received and what the handler forwarded), so the suite
//! lives here rather than in any one of them. Every observation is one a client makes: the
//! message on the queue target, the execution's own `input`, and the queue the invoked handler
//! forwards its event to.
//!
//! The rows differ along what the rule generalises over: WHICH target kinds the rule carries
//! and HOW MANY, the ORDER they are written in, and the shape of the `detail` the envelope has
//! to carry to each of them.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::query_wire::{element, form_encode};

/// The Region every request below is signed for — the one the shared stack reader
/// ([`wait_for_stack_status`]) signs for.
const REGION: &str = "us-east-1";

/// The `StackStatus` each row's function stack has to reach.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The `source` every event here is published under, and the one every rule matches on.
const SOURCE: &str = "one.envelope.suite";

/// The bus every rule here lives on.
const BUS: &str = "one-envelope-bus";

/// The service principal EventBridge hands a resource-policy-governed target over as, and the
/// action an invocation of a function performs: what the function's own policy has to grant for
/// the handover to happen at all ("for Lambda functions, Amazon SNS topics and Amazon SQS
/// queues, EventBridge relies on resource-based policies", `PutTargets`).
const EVENTS_PRINCIPAL: &str = "events.amazonaws.com";
const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// The roles this suite provisions in IAM: the one EventBridge starts a state-machine target
/// under ("for EC2 instances, Kinesis data streams, Step Functions state machines and API
/// Gateway APIs, EventBridge relies on IAM roles that you specify in the `RoleARN` argument in
/// `PutTargets`"), the one each state machine itself runs as, and the one each function runs as.
const TARGET_ROLE: &str = "one-envelope-events-invoke-states";
const MACHINE_ROLE: &str = "one-envelope-state-machine";
const FUNCTION_ROLE: &str = "one-envelope-function";

/// How long an observation of work the CLOCK owes — the stack coming up, the function going
/// `Active`, a delivery, an execution, the handler's own forward — waits before giving up, and
/// how long between two looks.
const BUDGET: Duration = Duration::from_secs(60);
/// See [`BUDGET`].
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// The handler every row deploys inline: it forwards the WHOLE event it was invoked with to the
/// sink queue its environment names, addressed by NAME through the service's own `GetQueueUrl`,
/// so what EventBridge handed the function is readable where a client reads a queue.
const FORWARDING_SOURCE: &str = r#"
import json
import os

import boto3


def handler(event, context):
    client = boto3.client("sqs")
    url = client.get_queue_url(QueueName=os.environ["SINK_QUEUE"])["QueueUrl"]
    client.send_message(QueueUrl=url, MessageBody=json.dumps(event))
    return {"forwarded": 1}
"#;

/// The definition every state machine here is created with: one state that ends, so the
/// execution's own `input` is the whole of what the handover carried.
const PASS_DEFINITION: &str = r#"{"StartAt":"Done","States":{"Done":{"Type":"Pass","End":true}}}"#;

/// The emulator as it is ASSEMBLED, on one engine: the handler's own SDK call is dispatched back
/// into this same router, so the sink queue it writes is the queue this suite reads.
fn emulator() -> MultiServiceRouter {
    let engine = std::sync::Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    // The suite exercises the services, not SigV4: every call carries a credential scope so the
    // router can name the service it belongs to.
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` whose credential scope names `signing_name`, which is what routes the
/// request. The signature itself is never checked here.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// Dispatch one request as it arrives on the wire, with whatever status it answered.
async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Bytes,
) -> (u16, Bytes) {
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
    (parts.status.as_u16(), body)
}

/// One `X-Amz-Target` RPC call, as its status and parsed body.
async fn rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let (status, answered) = dispatch(
        router,
        "POST",
        &format!("{signing_name}.{REGION}.amazonaws.com"),
        "/",
        &[
            (
                "content-type",
                &format!("application/x-amz-json-{json_version}"),
            ),
            ("x-amz-target", &format!("{target_prefix}.{operation}")),
            ("authorization", &authorization(signing_name)),
        ],
        Bytes::from(body.to_string()),
    )
    .await;
    let parsed: Value = if answered.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&answered)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&answered).to_string()))
    };
    (status, parsed)
}

/// One EventBridge call (`awsJson1_1`, `AWSEvents.<Operation>`) that must answer 200.
async fn events(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(router, "events", "AWSEvents", "1.1", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from EventBridge {operation}, got {status} with {parsed}"
    );
    parsed
}

/// One Amazon SQS call (`awsJson1_0`, `AmazonSQS.<Operation>`) that must answer 200.
async fn sqs(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(router, "sqs", "AmazonSQS", "1.0", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from SQS {operation}, got {status} with {parsed}"
    );
    parsed
}

/// One Step Functions call (`awsJson1_0`, `AWSStepFunctions.<Operation>`) that must answer 200.
async fn sfn(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(
        router,
        "states",
        "AWSStepFunctions",
        "1.0",
        operation,
        body,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Step Functions {operation}, got {status} with \
         {parsed}"
    );
    parsed
}

/// One AWS Lambda call over its `restJson1` wire, with whatever status it answered.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let (status, answered) = dispatch(
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
    let parsed: Value = if answered.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&answered)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&answered).to_string()))
    };
    (status, parsed)
}

/// One awsQuery call of `signing_name` at `host`, as its status and XML body.
async fn query(
    router: &MultiServiceRouter,
    signing_name: &str,
    host: &str,
    version: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let (status, answered) = dispatch(
        router,
        "POST",
        host,
        "/",
        &[
            ("content-type", "application/x-www-form-urlencoded"),
            ("authorization", &authorization(signing_name)),
        ],
        Bytes::from(body),
    )
    .await;
    (status, String::from_utf8_lossy(&answered).to_string())
}

/// One IAM call that must answer 200.
async fn iam(router: &MultiServiceRouter, action: &str, members: &[(&str, &str)]) -> String {
    let (status, body) = query(
        router,
        "iam",
        "iam.amazonaws.com",
        "2010-05-08",
        action,
        members,
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected HTTP 200 from IAM {action}, got {status} with {body}"
    );
    body
}

/// One CloudFormation call that must answer 200.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> String {
    let (status, body) = query(
        router,
        "cloudformation",
        &format!("cloudformation.{REGION}.amazonaws.com"),
        "2010-05-15",
        action,
        members,
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected HTTP 200 from CloudFormation {action}, got {status} with {body}"
    );
    body
}

/// A role called `name`, trusted by the service principal `principal`, granting `actions` on
/// every resource; answers the ARN IAM reports for it.
async fn given_role(
    router: &MultiServiceRouter,
    name: &str,
    principal: &str,
    actions: &[&str],
) -> String {
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": principal },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string();
    let created = iam(
        router,
        "CreateRole",
        &[("RoleName", name), ("AssumeRolePolicyDocument", &trust)],
    )
    .await;
    let arn = element(&created, "Arn");
    let permissions = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": actions,
            // The row resources are created after the roles, so each grant names the ACTIONS
            // its holder performs rather than one row's own resource.
            "Resource": "*",
        }],
    })
    .to_string();
    iam(
        router,
        "PutRolePolicy",
        &[
            ("RoleName", name),
            ("PolicyName", "what-this-role-does"),
            ("PolicyDocument", &permissions),
        ],
    )
    .await;
    arn
}

/// A queue called `name`; answers its url and the ARN Amazon SQS itself reports for it.
async fn given_queue(router: &MultiServiceRouter, case: &str, name: &str) -> (String, String) {
    let created = sqs(router, case, "CreateQueue", &json!({ "QueueName": name })).await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: CreateQueue {name:?} reports no QueueUrl, got {created}")
        })
        .to_string();
    let read = sqs(
        router,
        case,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    let arn = read["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the queue {name:?} reports no QueueArn"))
        .to_string();
    (url, arn)
}

/// Put the policy EventBridge's own delivery needs on the queue `url` addresses: the
/// `sqs:SendMessage` grant to the delivering service principal.
async fn allow_the_delivery(router: &MultiServiceRouter, case: &str, url: &str, queue_arn: &str) {
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AllowEventBridge",
            "Effect": "Allow",
            "Principal": { "Service": EVENTS_PRINCIPAL },
            "Action": "sqs:SendMessage",
            "Resource": queue_arn,
        }],
    });
    sqs(
        router,
        case,
        "SetQueueAttributes",
        &json!({ "QueueUrl": url, "Attributes": { "Policy": policy.to_string() } }),
    )
    .await;
}

/// Deploy the row's function and the grant its invocation needs, as one stack whose inline code
/// CloudFormation archives into the deployment package Lambda runs; answers the function's ARN.
async fn given_function(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    function: &str,
    sink_queue: &str,
    role_arn: &str,
) -> String {
    let template = json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "Forwarder": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": function,
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    "Role": role_arn,
                    "Timeout": 30,
                    "Environment": { "Variables": { "SINK_QUEUE": sink_queue } },
                    "Code": { "ZipFile": FORWARDING_SOURCE },
                },
            },
            "InvokePermission": {
                "Type": "AWS::Lambda::Permission",
                "Properties": {
                    "FunctionName": { "Fn::GetAtt": ["Forwarder", "Arn"] },
                    "Action": INVOKE_ACTION,
                    "Principal": EVENTS_PRINCIPAL,
                },
            },
        },
    })
    .to_string();
    cloudformation(
        router,
        "CreateStack",
        &[("StackName", stack), ("TemplateBody", &template)],
    )
    .await;
    let (reached, described) = wait_for_stack_status(router, stack, CREATE_COMPLETE).await;
    assert_eq!(
        reached, CREATE_COMPLETE,
        "case {case:?}: expected the function's stack to reach {CREATE_COMPLETE}, got {described}"
    );
    let deadline = Instant::now() + BUDGET;
    loop {
        let (status, configuration) = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{function}/configuration"),
            None,
        )
        .await;
        if status == 200 && configuration["State"].as_str() == Some("Active") {
            return configuration["FunctionArn"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the function reports no ARN, got {configuration}")
                })
                .to_string();
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: Lambda never reported {function} Active, last read {status} \
             {configuration}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// A state machine called `name` whose one state ends; answers its ARN.
async fn given_state_machine(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    role_arn: &str,
) -> String {
    let created = sfn(
        router,
        case,
        "CreateStateMachine",
        &json!({ "name": name, "definition": PASS_DEFINITION, "roleArn": role_arn }),
    )
    .await;
    created["stateMachineArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateStateMachine reports no ARN, got {created}"))
        .to_string()
}

/// The ONE message the queue `url` addresses holds, as the text of its body — polled for as
/// long as [`BUDGET`].
async fn await_one_body(router: &MultiServiceRouter, case: &str, url: &str, what: &str) -> String {
    let deadline = Instant::now() + BUDGET;
    loop {
        let received = sqs(
            router,
            case,
            "ReceiveMessage",
            &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
        )
        .await;
        let messages = received["Messages"].as_array().cloned().unwrap_or_default();
        if let Some(message) = messages.first() {
            assert_eq!(
                messages.len(),
                1,
                "case {case:?}: expected exactly one {what}, got {messages:?}"
            );
            return message["Body"].as_str().unwrap_or_default().to_string();
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: no {what} ever arrived on {url}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// The `input` of the ONE execution the state machine `arn` names holds — polled for as long as
/// [`BUDGET`].
async fn await_one_execution_input(router: &MultiServiceRouter, case: &str, arn: &str) -> String {
    let deadline = Instant::now() + BUDGET;
    loop {
        let listed = sfn(
            router,
            case,
            "ListExecutions",
            &json!({ "stateMachineArn": arn }),
        )
        .await;
        let executions = listed["executions"].as_array().cloned().unwrap_or_default();
        if let Some(execution) = executions.first() {
            assert_eq!(
                executions.len(),
                1,
                "case {case:?}: expected exactly one execution of {arn}, got {executions:?}"
            );
            let execution_arn = execution["executionArn"].as_str().unwrap_or_default();
            let described = sfn(
                router,
                case,
                "DescribeExecution",
                &json!({ "executionArn": execution_arn }),
            )
            .await;
            return described["input"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the execution reports no input, got {described}")
                })
                .to_string();
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the state-machine target {arn} was never started, {listed}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Which service one target of a row names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Branch {
    /// An Amazon SQS queue, reached by `sqs:SendMessage`.
    Queue,
    /// A Lambda function, reached by `lambda:InvokeFunction`.
    Function,
    /// A Step Functions state machine, reached by `states:StartExecution`.
    StateMachine,
}

/// One row: the targets the rule carries, in the order they are written, and the `detail` the
/// envelope has to carry to each of them.
struct Row {
    case: &'static str,
    slug: &'static str,
    detail_type: &'static str,
    branches: &'static [Branch],
    detail: fn() -> Value,
}

/// The rows: the target kinds of one rule, their order and their count, over the shapes of
/// `detail` the envelope has to carry unchanged.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a queue, a function and a state machine, in that order",
            slug: "one-envelope-forward",
            detail_type: "OneEnvelopeForward",
            branches: &[Branch::Queue, Branch::Function, Branch::StateMachine],
            detail: || json!({ "order": { "id": "o-1", "items": 2 } }),
        },
        Row {
            case: "the same three targets written in the reverse order",
            slug: "one-envelope-reverse",
            detail_type: "OneEnvelopeReverse",
            branches: &[Branch::StateMachine, Branch::Function, Branch::Queue],
            detail: || {
                json!({
                    "flags": [true, false],
                    "total": 19.5,
                    "note": null,
                    "label": "quoted \"text\"",
                })
            },
        },
        Row {
            case: "two targets only: the function and the state machine",
            slug: "one-envelope-pair",
            detail_type: "OneEnvelopePair",
            branches: &[Branch::Function, Branch::StateMachine],
            detail: || json!({}),
        },
    ]
}

/// RULE: every target of a matched rule is handed one and the same envelope, whatever service it
/// names and whichever order the targets are written in.
#[tokio::test]
async fn one_event_reaches_every_target_kind_with_the_same_envelope() {
    let router = emulator();
    events(&router, "setup", "CreateEventBus", &json!({ "Name": BUS })).await;
    let target_role = given_role(
        &router,
        TARGET_ROLE,
        EVENTS_PRINCIPAL,
        &["states:StartExecution"],
    )
    .await;
    let machine_role = given_role(
        &router,
        MACHINE_ROLE,
        "states.amazonaws.com",
        &["states:*"],
    )
    .await;
    let function_role = given_role(
        &router,
        FUNCTION_ROLE,
        "lambda.amazonaws.com",
        &["sqs:GetQueueUrl", "sqs:SendMessage"],
    )
    .await;

    for row in rows() {
        let case = row.case;
        let detail = (row.detail)();

        // One resource per branch the row carries, and the sink the invoked handler forwards
        // what IT was handed to.
        let mut target_url: Option<String> = None;
        let mut sink_url: Option<String> = None;
        let mut machine_arn: Option<String> = None;
        let mut written: Vec<Value> = Vec::new();
        for (index, branch) in row.branches.iter().enumerate() {
            let id = format!("t{index}");
            match branch {
                Branch::Queue => {
                    let (url, arn) =
                        given_queue(&router, case, &format!("{}-target", row.slug)).await;
                    allow_the_delivery(&router, case, &url, &arn).await;
                    target_url = Some(url);
                    written.push(json!({ "Id": id, "Arn": arn }));
                }
                Branch::Function => {
                    let sink_name = format!("{}-sink", row.slug);
                    let (url, _) = given_queue(&router, case, &sink_name).await;
                    let function = format!("{}-forwarder", row.slug);
                    let arn = given_function(
                        &router,
                        case,
                        &format!("{}-stack", row.slug),
                        &function,
                        &sink_name,
                        &function_role,
                    )
                    .await;
                    sink_url = Some(url);
                    written.push(json!({ "Id": id, "Arn": arn }));
                }
                Branch::StateMachine => {
                    let arn = given_state_machine(
                        &router,
                        case,
                        &format!("{}-machine", row.slug),
                        &machine_role,
                    )
                    .await;
                    machine_arn = Some(arn.clone());
                    // EventBridge starts a state machine AS the role the target names, so the
                    // target carries one.
                    written.push(json!({ "Id": id, "Arn": arn, "RoleArn": target_role }));
                }
            }
        }

        let rule = format!("{}-rule", row.slug);
        events(
            &router,
            case,
            "PutRule",
            &json!({
                "Name": rule,
                "EventBusName": BUS,
                "EventPattern": json!({
                    "source": [SOURCE],
                    "detail-type": [row.detail_type],
                })
                .to_string(),
            }),
        )
        .await;
        let targeted = events(
            &router,
            case,
            "PutTargets",
            &json!({ "Rule": rule, "EventBusName": BUS, "Targets": written }),
        )
        .await;
        assert_eq!(
            targeted["FailedEntryCount"].as_i64(),
            Some(0),
            "case {case:?}: every target must be accepted, got {targeted}"
        );

        let published = events(
            &router,
            case,
            "PutEvents",
            &json!({
                "Entries": [{
                    "EventBusName": BUS,
                    "Source": SOURCE,
                    "DetailType": row.detail_type,
                    "Detail": detail.to_string(),
                }]
            }),
        )
        .await;
        assert_eq!(
            published["FailedEntryCount"].as_i64(),
            Some(0),
            "case {case:?}: the event must be accepted, got {published}"
        );
        let event_id = published["Entries"][0]["EventId"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the accepted entry reports no EventId, got {published}")
            })
            .to_string();

        // What each branch was handed, read where a client reads it.
        let mut envelopes: Vec<(&str, Value)> = Vec::new();
        if let Some(url) = &target_url {
            let text = await_one_body(&router, case, url, "message on the queue target").await;
            envelopes.push(("the queue target", parsed(case, "the queue target", &text)));
        }
        if let Some(url) = &sink_url {
            let text = await_one_body(&router, case, url, "forward from the invoked handler").await;
            envelopes.push((
                "the invoked function",
                parsed(case, "the invoked function", &text),
            ));
        }
        if let Some(arn) = &machine_arn {
            let text = await_one_execution_input(&router, case, arn).await;
            envelopes.push((
                "the state-machine target",
                parsed(case, "the state-machine target", &text),
            ));
        }
        assert_eq!(
            envelopes.len(),
            row.branches.len(),
            "case {case:?}: expected one observation per target, got {envelopes:?}"
        );

        // ONE envelope: every target saw the same document, and that document is the event as
        // it was published.
        let distinct: BTreeSet<String> = envelopes
            .iter()
            .map(|(_, envelope)| envelope.to_string())
            .collect();
        assert_eq!(
            distinct.len(),
            1,
            "case {case:?}: every target must be handed the same envelope, they were handed \
             {envelopes:?}"
        );
        for (where_from, envelope) in &envelopes {
            assert_eq!(
                envelope["id"].as_str(),
                Some(event_id.as_str()),
                "case {case:?}: {where_from} must carry the published event's own id, got \
                 {envelope}"
            );
            assert_eq!(
                envelope["source"].as_str(),
                Some(SOURCE),
                "case {case:?}: {where_from} must carry the submitted source, got {envelope}"
            );
            assert_eq!(
                envelope["detail-type"].as_str(),
                Some(row.detail_type),
                "case {case:?}: {where_from} must carry the submitted detail-type, got {envelope}"
            );
            assert_eq!(
                envelope["detail"], detail,
                "case {case:?}: {where_from} must carry the submitted detail unchanged, got \
                 {envelope}"
            );
        }
    }
}

/// `text` as the JSON document a target is handed, or a failure naming where it came from.
fn parsed(case: &str, where_from: &str, text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|error| {
        panic!("case {case:?}: {where_from} must be handed a JSON document, got {text:?} ({error})")
    })
}
