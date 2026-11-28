// SPDX-License-Identifier: CC0-1.0

//! What a pipe whose source is an Amazon SQS queue and whose target is a LAMBDA FUNCTION does
//! once it is running, and what its EXECUTION ROLE decides about it.
//!
//! RULE: a running pipe moves each record of its source with no client action at all, and every
//! call it makes on the way is authorized against the policies in force on its execution role
//! BEFORE it is made — so a pipe whose role admits the call that TAKES the record and the call
//! that INVOKES the target delivers that record to the function, whose own effect is what a
//! client observes, and leaves the source holding nothing; while a pipe whose role withholds the
//! source's `sqs:ReceiveMessage` takes nothing at all: the function is never invoked and the
//! record is still WAITING on the queue for another consumer
//! ("ApproximateNumberOfMessages — the approximate number of messages available for retrieval",
//! with nothing "in flight" under `ApproximateNumberOfMessagesNotVisible`), whatever startup
//! state the pipe itself comes to rest in.
//!
//! Five crates have to agree for that to hold — CloudFormation provisions the declared wiring,
//! EventBridge Pipes runs the pipe, IAM holds the role whose documents answer every call,
//! Lambda runs the function the target stage invokes, and Amazon SQS holds the source the record
//! is taken from and the queue the function's own SDK call writes to — so the suite lives here
//! rather than in any one of them.
//!
//! What is new here: the sibling suite `cloudformation_deployed_pipe_delivery` asks the same
//! question of a pipe whose TARGET is another queue, and `pipes_withheld_call_is_never_made`
//! draws the available/in-flight distinction for that target; the pipes crate's own
//! `tests/all/pipe_lambda_target.rs` drives a function target with NO IAM store mounted, so no
//! stage of it is authorized against a role. This suite is the one that pins the chain a client
//! actually ships — a queue, a function that forwards what it was handed, and a pipe between
//! them acting as a role IAM really holds — with the function's own `boto3` call as the
//! observation.
//!
//! The rows differ along what the rule generalises over: how the pipe came to exist (a
//! CloudFormation `AWS::Pipes::Pipe` declaration, the `CreatePipe` API), and what the pipe's
//! execution role admits (every call its stages make; no call at all on the source; every call
//! on the source EXCEPT the receive). A declared pipe whose role leaves it no call on its source
//! is not a row here: that pipe cannot be created at all, which is the rollback rule
//! `cloudformation_deployed_pipe_delivery` tables.
//!
//! Every wait below is on a CONDITION with a generous deadline, and nothing asserts on elapsed
//! time.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{elements, form_encode, members_of, opt_element, outputs_of};

/// The Region every call below is signed for, which is what each handler reads its Region from.
const REGION: &str = "us-east-1";

/// The `StackStatus` the one deployment here must reach.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The `CurrentState` a pipe asked to run comes to rest in, which is when it polls.
const RUNNING: &str = "RUNNING";

/// The startup states a pipe whose role withholds a source call may come to rest in: it is
/// either up and refused at every poll, or it could not be brought up at all.
const TERMINAL_STARTUP_STATES: [&str; 3] = [RUNNING, "START_FAILED", "CREATE_FAILED"];

/// The API action that TAKES the records from an Amazon SQS source, which a startup failure's
/// `StateReason` names as the call the role leaves the pipe without.
const RECORD_TAKING_CALL: &str = "ReceiveMessage";

/// How long work nothing asked for — a stack's resources coming up, a pipe settling, a batch
/// polled, the function's own send landing — is given before a row fails. Well above the
/// poller's interval, the settle span and one interpreted run of the handler, so a row fails
/// only when the work did not happen at all.
const BUDGET: Duration = Duration::from_secs(90);

/// How long a row that expects NOTHING watches for. A pipe that was going to act would have:
/// it is many times the poller's own interval and several times its retry backoff.
const QUIET: Duration = Duration::from_secs(3);

/// How often anything is looked at again while waiting.
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// The stack the emulator's own boot path gives the thread that assembles the router
/// (`core/router/src/main.rs`): this chain — CloudFormation into pipes, the pipe's poll into
/// Amazon SQS, its target stage into Lambda, the handler's own SDK call back into Amazon SQS —
/// nests more future than a default test thread's stack holds.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The attributes that together say how many records a queue HOLDS, and apart say whether it can
/// still hand one over: the records waiting for retrieval, and the ones a consumer has taken and
/// not deleted.
const AVAILABLE: &str = "ApproximateNumberOfMessages";
const IN_FLIGHT: &str = "ApproximateNumberOfMessagesNotVisible";

/// The function every row's pipe targets: for each record of the batch it was handed it forwards
/// that record's own body to the queue its environment names, through the AWS SDK the runtime
/// supplies. So one message on the result queue is one record a pipe delivered, and what it
/// carries is the record the pipe took.
pub(crate) const FORWARDER: &str = r#"import json
import os

import boto3


def handler(event, context):
    client = boto3.client("sqs")
    forwarded = []
    for record in event:
        body = json.loads(record["body"])
        client.send_message(
            QueueUrl=os.environ["RESULT_QUEUE"],
            MessageBody=json.dumps({"marker": body["marker"], "id": record["messageId"]}),
        )
        forwarded.append(record["messageId"])
    return {"forwarded": forwarded}
"#;

/// The environment variable the handler reads the result queue's url from. Not read by the
/// emulator: it is the handler's own, passed to it by the declaration.
const RESULT_QUEUE_VARIABLE: &str = "RESULT_QUEUE";

/// Run `body` to completion on a thread sized like the emulator's own boot path, on the
/// current-thread runtime the pipe's own tasks then run on.
fn driven<Body>(name: &'static str, body: Body)
where
    Body: FnOnce() -> ::std::pin::Pin<Box<dyn ::std::future::Future<Output = ()>>> + Send + 'static,
{
    ::std::thread::Builder::new()
        .name(name.to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(move || {
            let runtime = ::tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build a current-thread runtime");
            runtime.block_on(body());
        })
        .expect("spawn the sized thread")
        .join()
        .expect("the suite thread runs to completion");
}

/// The emulator as it is ASSEMBLED, on one engine: the pipe's own calls are dispatched back into
/// this same router, so the queue this suite reads is the queue the handler wrote.
fn build_router() -> MultiServiceRouter {
    let engine: std::sync::Arc<engine::InMemoryEngine> =
        std::sync::Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in [`REGION`], which is
/// what routes the request. The signature is never checked: authentication is off here.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One CloudFormation call over its awsQuery wire, as its status and the XML it answered.
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
    let response = Box::pin(router.dispatch(request)).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One Amazon SQS call over its `awsJson1.0` wire.
async fn sqs(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sqs.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .header("authorization", authorization("sqs"))
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = Box::pin(router.dispatch(request)).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&payload).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One EventBridge Pipes call over its own `restJson1` wire, on the URI the operation's `@http`
/// binding declares (`POST /v1/pipes/{Name}`, `GET /v1/pipes/{Name}`).
async fn pipes(
    router: &MultiServiceRouter,
    method: &str,
    name: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = body.map(|value| value.to_string().into_bytes()).unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(format!("/v1/pipes/{name}"))
        .header("host", format!("pipes.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("pipes"))
        .body(Bytes::from(payload))
        .expect("a well-formed request");
    let response = Box::pin(router.dispatch(request)).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// Every resource the stack reports a FAILED status for, with the reason it gave — quoted when a
/// deployment did not complete, which is where a resource that could not be created says why.
async fn reported_failures(router: &MultiServiceRouter, stack: &str) -> Vec<String> {
    let (status, body) =
        cloudformation(router, "DescribeStackEvents", &[("StackName", stack)]).await;
    if status != 200 {
        return vec![format!("DescribeStackEvents answered {status}: {body}")];
    }
    members_of(&body)
        .into_iter()
        .filter_map(|member| {
            let resource_status = elements(member, "ResourceStatus").into_iter().next()?;
            if !resource_status.contains("FAILED") {
                return None;
            }
            let logical = elements(member, "LogicalResourceId").into_iter().next()?;
            let reason = elements(member, "ResourceStatusReason")
                .into_iter()
                .next()
                .unwrap_or_default();
            Some(format!("{logical} {resource_status}: {reason}"))
        })
        .collect()
}

/// Create `stack` from `template`, wait until it is no longer in progress, and answer the outputs
/// it published.
async fn deployed(
    router: &MultiServiceRouter,
    stack: &str,
    template: &str,
) -> BTreeMap<String, String> {
    let (status, body) = cloudformation(
        router,
        "CreateStack",
        &[
            ("StackName", stack),
            ("TemplateBody", template),
            ("Capabilities.member.1", "CAPABILITY_IAM"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateStack({stack:?}) must be accepted, got {status} with {body}"
    );
    let deadline = Instant::now() + BUDGET;
    let mut described = String::new();
    let mut reported = String::new();
    while Instant::now() < deadline {
        let (status, body) = cloudformation(router, "DescribeStacks", &[("StackName", stack)]).await;
        assert_eq!(
            status, 200,
            "setup: DescribeStacks({stack:?}) must answer 200, got {status} with {body}"
        );
        reported = opt_element(&body, "StackStatus").unwrap_or_default();
        described = body;
        if !reported.ends_with("_IN_PROGRESS") {
            break;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
    assert_eq!(
        reported, CREATE_COMPLETE,
        "setup: the stack must reach {CREATE_COMPLETE}, got {reported:?}; the resource failures \
         it reported: {:?}",
        reported_failures(router, stack).await
    );
    outputs_of(&described)
}

/// The value `name` the stack published.
fn published(outputs: &BTreeMap<String, String>, name: &str) -> String {
    outputs
        .get(name)
        .cloned()
        .unwrap_or_else(|| panic!("setup: the stack must publish {name:?}, it published {outputs:?}"))
}

/// A role declaration trusting EventBridge Pipes, carrying `statements` as its one inline policy:
/// a role a pipe acts as has to admit that service principal, or the pipe cannot be created at
/// all.
fn pipe_role(statements: Value) -> Value {
    json!({
        "Type": "AWS::IAM::Role",
        "Properties": {
            "AssumeRolePolicyDocument": {
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "Service": "pipes.amazonaws.com" },
                    "Action": "sts:AssumeRole",
                }],
            },
            "Policies": [{
                "PolicyName": "move-the-records",
                "PolicyDocument": { "Version": "2012-10-17", "Statement": statements },
            }],
        },
    })
}

/// A source queue declaration, named after the row it serves.
fn queue(name: &str) -> Value {
    json!({
        "Type": "AWS::SQS::Queue",
        "Properties": { "QueueName": name },
    })
}

/// The one template this suite deploys: one result queue and one forwarding function, a source
/// queue per row, the three execution roles the rows act as, and the pipe the declared row is.
fn template(prefix: &str) -> String {
    let source_arn = |logical: &str| json!({ "Fn::GetAtt": [logical, "Arn"] });
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "ResultQueue": queue(&format!("{prefix}-result")),
            "DeclaredSource": queue(&format!("{prefix}-declared-source")),
            "ApiSource": queue(&format!("{prefix}-api-source")),
            "NoSourceCallSource": queue(&format!("{prefix}-no-source-call-source")),
            "NoReceiveSource": queue(&format!("{prefix}-no-receive-source")),
            "ForwarderRole": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "AssumeRolePolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": { "Service": "lambda.amazonaws.com" },
                            "Action": "sts:AssumeRole",
                        }],
                    },
                    "Policies": [{
                        "PolicyName": "forward-the-records",
                        "PolicyDocument": {
                            "Version": "2012-10-17",
                            "Statement": [{
                                "Effect": "Allow",
                                "Action": ["sqs:SendMessage", "logs:*"],
                                "Resource": "*",
                            }],
                        },
                    }],
                },
            },
            "Forwarder": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": format!("{prefix}-forwarder"),
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    "Role": { "Fn::GetAtt": ["ForwarderRole", "Arn"] },
                    "Environment": {
                        "Variables": { RESULT_QUEUE_VARIABLE: { "Ref": "ResultQueue" } },
                    },
                    "Code": { "ZipFile": FORWARDER },
                },
            },
            // Every call an Amazon SQS source's stage makes on the source, and the call a
            // function target's stage makes on the target.
            "OpenRole": pipe_role(json!([
                {
                    "Effect": "Allow",
                    "Action": ["sqs:ReceiveMessage", "sqs:DeleteMessage", "sqs:GetQueueAttributes"],
                    "Resource": [source_arn("DeclaredSource"), source_arn("ApiSource")],
                },
                {
                    "Effect": "Allow",
                    "Action": "lambda:InvokeFunction",
                    "Resource": { "Fn::GetAtt": ["Forwarder", "Arn"] },
                },
            ])),
            // No call at all on the source: the target's invoke and nothing else.
            "NoSourceCallRole": pipe_role(json!([{
                "Effect": "Allow",
                "Action": "lambda:InvokeFunction",
                "Resource": { "Fn::GetAtt": ["Forwarder", "Arn"] },
            }])),
            // Every call on the source except the one that TAKES the record.
            "NoReceiveRole": pipe_role(json!([
                {
                    "Effect": "Allow",
                    "Action": ["sqs:DeleteMessage", "sqs:GetQueueAttributes"],
                    "Resource": source_arn("NoReceiveSource"),
                },
                {
                    "Effect": "Allow",
                    "Action": "lambda:InvokeFunction",
                    "Resource": { "Fn::GetAtt": ["Forwarder", "Arn"] },
                },
            ])),
            "DeclaredPipe": {
                "Type": "AWS::Pipes::Pipe",
                "Properties": {
                    "Name": format!("{prefix}-declared-pipe"),
                    "Source": source_arn("DeclaredSource"),
                    "Target": { "Fn::GetAtt": ["Forwarder", "Arn"] },
                    "RoleArn": { "Fn::GetAtt": ["OpenRole", "Arn"] },
                    "SourceParameters": { "SqsQueueParameters": { "BatchSize": 1 } },
                    "TargetParameters": {
                        "LambdaFunctionParameters": { "InvocationType": "REQUEST_RESPONSE" },
                    },
                },
            },
        },
        "Outputs": {
            "ResultUrl": { "Value": { "Ref": "ResultQueue" } },
            "FunctionArn": { "Value": { "Fn::GetAtt": ["Forwarder", "Arn"] } },
            "DeclaredSourceUrl": { "Value": { "Ref": "DeclaredSource" } },
            "ApiSourceUrl": { "Value": { "Ref": "ApiSource" } },
            "ApiSourceArn": { "Value": source_arn("ApiSource") },
            "NoSourceCallSourceUrl": { "Value": { "Ref": "NoSourceCallSource" } },
            "NoSourceCallSourceArn": { "Value": source_arn("NoSourceCallSource") },
            "NoReceiveSourceUrl": { "Value": { "Ref": "NoReceiveSource" } },
            "NoReceiveSourceArn": { "Value": source_arn("NoReceiveSource") },
            "OpenRoleArn": { "Value": { "Fn::GetAtt": ["OpenRole", "Arn"] } },
            "NoSourceCallRoleArn": { "Value": { "Fn::GetAtt": ["NoSourceCallRole", "Arn"] } },
            "NoReceiveRoleArn": { "Value": { "Fn::GetAtt": ["NoReceiveRole", "Arn"] } },
        },
    })
    .to_string()
}

/// `CreatePipe` for a pipe named `name` wiring `source` to the function `target` under `role`,
/// the same configuration the declared row carries.
async fn create_pipe(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    source: &str,
    target: &str,
    role: &str,
) {
    let (status, created) = pipes(
        router,
        "POST",
        name,
        Some(&json!({
            "Source": source,
            "Target": target,
            "RoleArn": role,
            "DesiredState": RUNNING,
            "SourceParameters": { "SqsQueueParameters": { "BatchSize": 1 } },
            "TargetParameters": {
                "LambdaFunctionParameters": { "InvocationType": "REQUEST_RESPONSE" },
            },
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreatePipe({name:?}) answered {status} {created}"
    );
}

/// The `CurrentState` the pipe `name` reports now, with the `StateReason` it carries.
async fn pipe_state(router: &MultiServiceRouter, case: &str, name: &str) -> (String, String) {
    let (status, described) = pipes(router, "GET", name, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribePipe({name:?}) answered {status} {described}"
    );
    (
        described["CurrentState"].as_str().unwrap_or_default().to_string(),
        described["StateReason"].as_str().unwrap_or_default().to_string(),
    )
}

/// Wait until the pipe `name` rests in one of `wanted`, and answer the state and reason it then
/// reports.
async fn settled_pipe(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    wanted: &[&str],
) -> (String, String) {
    let deadline = Instant::now() + BUDGET;
    loop {
        let (state, reason) = pipe_state(router, case, name).await;
        if wanted.contains(&state.as_str()) || Instant::now() >= deadline {
            return (state, reason);
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Put one record carrying `marker` on the queue at `url`, as a producer does.
async fn send(router: &MultiServiceRouter, case: &str, url: &str, marker: &str) {
    let (status, sent) = sqs(
        router,
        "SendMessage",
        &json!({
            "QueueUrl": url,
            "MessageBody": json!({ "marker": marker }).to_string(),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: SendMessage({url:?}) answered {status} {sent}"
    );
}

/// Every message body on the queue at `url`, left where it is: `VisibilityTimeout: 0` takes no
/// checkout, so a later look sees them again and nothing this suite reads is hidden from the
/// next assertion.
async fn visible_bodies(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<String> {
    let (status, answered) = sqs(
        router,
        "ReceiveMessage",
        &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10, "VisibilityTimeout": 0 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ReceiveMessage({url:?}) answered {status} {answered}"
    );
    answered["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| message["Body"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The `marker` a forwarded message carries, for a body that is one of this handler's reports.
fn forwarded_marker(body: &str) -> Option<String> {
    let parsed: Value = serde_json::from_str(body).ok()?;
    Some(parsed["marker"].as_str()?.to_string())
}

/// What the queue at `url` reports for `attribute`, or -1 for an answer that carries none.
async fn counted(router: &MultiServiceRouter, case: &str, url: &str, attribute: &str) -> i64 {
    let (status, answered) = sqs(
        router,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": [attribute] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetQueueAttributes({url:?}) answered {status} {answered}"
    );
    answered["Attributes"][attribute]
        .as_str()
        .unwrap_or_default()
        .parse()
        .unwrap_or(-1)
}

/// How the pipe of one row came to exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Created {
    /// A CloudFormation `AWS::Pipes::Pipe` the stack declared.
    Declared,
    /// A `CreatePipe` call.
    Api,
}

/// One row: the pipe, the source its records are put on, the role it acts as, and whether the
/// rule says that role lets it move the record.
struct Row {
    case: &'static str,
    created: Created,
    /// The stack output naming the source queue's url, and — for a row whose pipe this suite
    /// creates — its ARN and the role's.
    source_url: &'static str,
    source_arn: &'static str,
    role_arn: &'static str,
    /// Whether the role admits the call that TAKES the record from the source.
    moves_the_record: bool,
}

/// RULE: a running pipe takes each record of its Amazon SQS source and invokes its Lambda target
/// with it — so the function's own effect is observed and the source is left holding nothing —
/// exactly when the policies in force on its execution role admit the calls that move it; a role
/// that withholds the source's receive leaves the record WAITING on the queue and the function
/// never invoked.
#[test]
fn an_sqs_sourced_pipe_delivers_to_its_lambda_target_only_as_its_role_admits() {
    driven("pipes-sqs-source-lambda-target", || {
        Box::pin(async {
            let rows = [
                Row {
                    case: "a declared pipe whose role admits every call",
                    created: Created::Declared,
                    source_url: "DeclaredSourceUrl",
                    source_arn: "",
                    role_arn: "",
                    moves_the_record: true,
                },
                Row {
                    case: "an API-created pipe whose role admits every call",
                    created: Created::Api,
                    source_url: "ApiSourceUrl",
                    source_arn: "ApiSourceArn",
                    role_arn: "OpenRoleArn",
                    moves_the_record: true,
                },
                Row {
                    case: "a pipe whose role may make no call at all on its source",
                    created: Created::Api,
                    source_url: "NoSourceCallSourceUrl",
                    source_arn: "NoSourceCallSourceArn",
                    role_arn: "NoSourceCallRoleArn",
                    moves_the_record: false,
                },
                Row {
                    case: "a pipe whose role withholds only the source's receive",
                    created: Created::Api,
                    source_url: "NoReceiveSourceUrl",
                    source_arn: "NoReceiveSourceArn",
                    role_arn: "NoReceiveRoleArn",
                    moves_the_record: false,
                },
            ];

            let router = build_router();
            let prefix = "pipes-lambda-target";
            let outputs = deployed(&router, &format!("{prefix}-stack"), &template(prefix)).await;
            let result_url = published(&outputs, "ResultUrl");
            let function_arn = published(&outputs, "FunctionArn");

            for (index, row) in rows.iter().enumerate() {
                let case = row.case;
                let source_url = published(&outputs, row.source_url);
                let name = match row.created {
                    Created::Declared => format!("{prefix}-declared-pipe"),
                    Created::Api => {
                        let name = format!("{prefix}-api-pipe-{index}");
                        create_pipe(
                            &router,
                            case,
                            &name,
                            &published(&outputs, row.source_arn),
                            &function_arn,
                            &published(&outputs, row.role_arn),
                        )
                        .await;
                        name
                    }
                };

                // Whatever the role admits, the pipe settles in a startup state a client can
                // read: up and polling, or one it could not be brought up from.
                let wanted: &[&str] = if row.moves_the_record {
                    &[RUNNING]
                } else {
                    &TERMINAL_STARTUP_STATES
                };
                let (state, reason) = settled_pipe(&router, case, &name, wanted).await;
                assert!(
                    wanted.contains(&state.as_str()),
                    "case {case:?}: expected the pipe to settle in one of {wanted:?}, it reports \
                     {state:?} ({reason:?})"
                );
                // A pipe that could not be brought UP says which call it may not make: the
                // `StateReason` of a startup failure names the call that takes the records,
                // which is the one these roles withhold. A pipe that IS up carries no such
                // reason and is refused at each poll instead.
                assert!(
                    row.moves_the_record || state == RUNNING || reason.contains(RECORD_TAKING_CALL),
                    "case {case:?}: expected a pipe that could not start to report why — a \
                     StateReason naming {RECORD_TAKING_CALL:?} — got {state:?} ({reason:?})"
                );

                let marker = format!("{prefix}-{index}-marker");
                send(&router, case, &source_url, &marker).await;

                if row.moves_the_record {
                    // The record reaches the function, which forwards it: one report carrying
                    // this row's marker.
                    let deadline = Instant::now() + BUDGET;
                    let mut forwarded: Vec<String> = Vec::new();
                    while Instant::now() < deadline {
                        forwarded = visible_bodies(&router, case, &result_url)
                            .await
                            .iter()
                            .filter_map(|body| forwarded_marker(body))
                            .collect();
                        if forwarded.contains(&marker) {
                            break;
                        }
                        tokio::time::sleep(LOOK_AGAIN).await;
                    }
                    assert!(
                        forwarded.contains(&marker),
                        "case {case:?}: expected the function to be invoked with the record and \
                         to forward {marker:?}, the result queue carries {forwarded:?}; the pipe \
                         reports {state:?} ({reason:?})"
                    );
                    // And the record it took is gone from the source: a batch an invocation
                    // took is deleted once it has it.
                    let deadline = Instant::now() + BUDGET;
                    let mut held = -1;
                    while Instant::now() < deadline {
                        let available = counted(&router, case, &source_url, AVAILABLE).await;
                        let in_flight = counted(&router, case, &source_url, IN_FLIGHT).await;
                        held = available + in_flight;
                        if held == 0 {
                            break;
                        }
                        tokio::time::sleep(LOOK_AGAIN).await;
                    }
                    assert_eq!(
                        held, 0,
                        "case {case:?}: expected the delivered record deleted from its source, \
                         the queue still holds {held}"
                    );
                } else {
                    // Nothing is invoked, so nothing carrying this row's marker is ever
                    // forwarded.
                    let watching_until = Instant::now() + QUIET;
                    let mut forwarded: Vec<String> = Vec::new();
                    while Instant::now() < watching_until {
                        forwarded.extend(
                            visible_bodies(&router, case, &result_url)
                                .await
                                .iter()
                                .filter_map(|body| forwarded_marker(body)),
                        );
                        tokio::time::sleep(LOOK_AGAIN).await;
                    }
                    assert!(
                        !forwarded.contains(&marker),
                        "case {case:?}: expected no invocation at all, the result queue carries \
                         {marker:?} among {forwarded:?}"
                    );
                    // The record was never TAKEN: it is still waiting for a consumer, with
                    // nothing in flight, and another consumer's own receive is handed it.
                    let available = counted(&router, case, &source_url, AVAILABLE).await;
                    let in_flight = counted(&router, case, &source_url, IN_FLIGHT).await;
                    assert_eq!(
                        (available, in_flight),
                        (1, 0),
                        "case {case:?}: expected the record still available on its source and \
                         nothing in flight, got {AVAILABLE}={available} {IN_FLIGHT}={in_flight}"
                    );
                    let waiting: Vec<Option<String>> = visible_bodies(&router, case, &source_url)
                        .await
                        .iter()
                        .map(|body| forwarded_marker(body))
                        .collect();
                    assert!(
                        waiting.contains(&Some(marker.clone())),
                        "case {case:?}: expected the queue to hand {marker:?} to another \
                         consumer, its receive answered {waiting:?}"
                    );
                }
            }
        })
    });
}
