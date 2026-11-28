// SPDX-License-Identifier: CC0-1.0

//! What the EXECUTION ROLE of an SQS-sourced pipe with a LAMBDA target decides, driven against
//! the emulator a client STARTS: the built binary, on its own port, with request authentication
//! on, reached over TLS with SigV4-signed calls.
//!
//! RULE: **a running pipe takes each record of its Amazon SQS source and invokes its Lambda
//! target with it exactly when the policies in force on its execution role admit the calls that
//! MOVE the record; a role that withholds the source's `sqs:ReceiveMessage` leaves the record
//! WAITING on the queue — available to another consumer, nothing in flight — and the function
//! never invoked — and that holds on the emulator as it is STARTED, not only on a router a test
//! assembles in its own process.**
//!
//! [`crate::pipes_sqs_source_lambda_target_delivery`] spans what the rule generalises over (how
//! the pipe came to exist, and which of the calls its stages make the role withholds) on a
//! router built inside the test process. The emulator a client runs is a different
//! configuration of the same code: the credentials are verified, each call arrives over its own
//! TLS connection, and the pipe's own poll, its target invocation and the handler's own SDK call
//! are all made on the binary's own runtime. This suite is the one that pins the role-scoped
//! chain THERE, so a pipe that is refused in process and delivers on the started emulator — or
//! one that delivers in process and moves nothing there — is a failure a test reports rather
//! than one only a client sees. ONE deployment is driven here, because starting the binary costs
//! what starting the emulator costs; the positive pipe beside the withheld one is its control.
//!
//! Nothing here invokes the function: the only data-plane calls are the `SendMessage`s that put
//! a record on each source queue and the reads of the result queue and the sources. Every body
//! read back off the result queue was sent by the handler's own `boto3` call out of the batch the
//! pipe handed it.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::cfn_deployed_pipeline_on_a_started_emulator::{deployed, output, Started};
use crate::pipes_sqs_source_lambda_target_delivery::FORWARDER;

/// The `PipeState` a pipe whose `DesiredState` is `RUNNING` comes to rest in.
const RUNNING: &str = "RUNNING";

/// The startup states a pipe whose role withholds the source's receive may rest in: up and
/// refused at every poll, or never brought up at all.
const TERMINAL_STARTUP_STATES: [&str; 3] = [RUNNING, "START_FAILED", "CREATE_FAILED"];

/// The API action that TAKES the records from an Amazon SQS source, which a startup failure's
/// `StateReason` names as the call the role leaves the pipe without.
const RECORD_TAKING_CALL: &str = "ReceiveMessage";

/// How long the deployment, a pipe's own settling and a delivery are given. Every step is owed by
/// the CLOCK, so each wait is on the CONDITION — looked for again until it holds — and this bound
/// is generous slack whose only job is that the wait TERMINATES. Nothing below asserts on elapsed
/// time.
const SETTLE: Duration = Duration::from_secs(120);

/// How long the result queue is watched for a record the withheld pipe could not take. A pipe
/// that was going to deliver would have: it is many times the poller's own interval.
const QUIET: Duration = Duration::from_secs(3);

/// How often the stack, a pipe or a queue is looked at again while waiting.
const LOOK_AGAIN: Duration = Duration::from_millis(100);

/// The attributes that apart say whether a queue can still hand a record over: the records
/// waiting for retrieval, and the ones a consumer has taken and not deleted.
const AVAILABLE: &str = "ApproximateNumberOfMessages";
const IN_FLIGHT: &str = "ApproximateNumberOfMessagesNotVisible";

/// The function both pipes target is the one the in-process table drives
/// ([`crate::pipes_sqs_source_lambda_target_delivery::FORWARDER`]): for each record of the
/// batch it was handed it forwards that record's own marker to the queue its environment
/// names, through the AWS SDK the runtime supplies. So one message on the result queue is one
/// record a pipe delivered, and the two configurations share one definition of the chain under
/// test.
const RESULT_QUEUE_VARIABLE: &str = "RESULT_QUEUE";

/// The one stack this suite deploys: the result queue and the forwarding function, one source
/// queue per pipe, the two execution roles the pipes act as, and the two pipes themselves.
///
/// The withheld role is granted every call on its source EXCEPT the one that takes the record, so
/// its pipe can be brought up and is refused at every poll — a role that leaves a pipe NO call on
/// its source is a pipe that cannot be created, which rolls the stack back
/// ([`crate::cloudformation_deployed_pipe_delivery`]).
fn template(prefix: &str) -> String {
    let arn_of = |logical: &str| json!({ "Fn::GetAtt": [logical, "Arn"] });
    let pipe_role = |statements: Value| {
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
    };
    let pipe = |name: String, source: &str, role: &str| {
        json!({
            "Type": "AWS::Pipes::Pipe",
            "Properties": {
                "Name": name,
                "Source": arn_of(source),
                "Target": arn_of("Forwarder"),
                "RoleArn": arn_of(role),
                "SourceParameters": { "SqsQueueParameters": { "BatchSize": 1 } },
                "TargetParameters": {
                    "LambdaFunctionParameters": { "InvocationType": "REQUEST_RESPONSE" },
                },
            },
        })
    };
    let queue = |name: String| {
        json!({ "Type": "AWS::SQS::Queue", "Properties": { "QueueName": name } })
    };
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "ResultQueue": queue(format!("{prefix}-result")),
            "OpenSource": queue(format!("{prefix}-open-source")),
            "WithheldSource": queue(format!("{prefix}-withheld-source")),
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
            "OpenRole": pipe_role(json!([
                {
                    "Effect": "Allow",
                    "Action": ["sqs:ReceiveMessage", "sqs:DeleteMessage", "sqs:GetQueueAttributes"],
                    "Resource": arn_of("OpenSource"),
                },
                {
                    "Effect": "Allow",
                    "Action": "lambda:InvokeFunction",
                    "Resource": arn_of("Forwarder"),
                },
            ])),
            "WithheldRole": pipe_role(json!([
                {
                    "Effect": "Allow",
                    "Action": ["sqs:DeleteMessage", "sqs:GetQueueAttributes"],
                    "Resource": arn_of("WithheldSource"),
                },
                {
                    "Effect": "Allow",
                    "Action": "lambda:InvokeFunction",
                    "Resource": arn_of("Forwarder"),
                },
            ])),
            "OpenPipe": pipe(format!("{prefix}-open-pipe"), "OpenSource", "OpenRole"),
            "WithheldPipe": pipe(
                format!("{prefix}-withheld-pipe"),
                "WithheldSource",
                "WithheldRole",
            ),
        },
        "Outputs": {
            "ResultUrl": { "Value": { "Ref": "ResultQueue" } },
            "OpenSourceUrl": { "Value": { "Ref": "OpenSource" } },
            "WithheldSourceUrl": { "Value": { "Ref": "WithheldSource" } },
        },
    })
    .to_string()
}

/// One Amazon SQS call over its `awsJson1.0` wire, as its status and parsed body.
fn sqs(emulator: &Started, operation: &str, body: &Value) -> (u16, Value) {
    let (status, text) = emulator.rpc(
        "sqs",
        &format!("AmazonSQS.{operation}"),
        "application/x-amz-json-1.0",
        body,
    );
    let parsed: Value = serde_json::from_str(&text).unwrap_or(Value::String(text));
    (status, parsed)
}

/// Every message body on the queue at `url`, left where it is: `VisibilityTimeout: 0` takes no
/// checkout, so a later look sees them again and nothing this suite reads is hidden from the next
/// assertion.
fn visible_bodies(emulator: &Started, case: &str, url: &str) -> Vec<String> {
    let (status, answered) = sqs(
        emulator,
        "ReceiveMessage",
        &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10, "VisibilityTimeout": 0 }),
    );
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

/// The `marker` a body carries, for a body that is JSON holding one.
fn marker_of(body: &str) -> Option<String> {
    let parsed: Value = serde_json::from_str(body).ok()?;
    Some(parsed["marker"].as_str()?.to_string())
}

/// The markers the result queue carries now.
fn forwarded_markers(emulator: &Started, case: &str, url: &str) -> Vec<String> {
    visible_bodies(emulator, case, url)
        .iter()
        .filter_map(|body| marker_of(body))
        .collect()
}

/// What the queue at `url` reports for `attribute`, or -1 for an answer carrying none.
fn counted(emulator: &Started, case: &str, url: &str, attribute: &str) -> i64 {
    let (status, answered) = sqs(
        emulator,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": [attribute] }),
    );
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

/// Put one record carrying `marker` on the queue at `url`.
fn send(emulator: &Started, case: &str, url: &str, marker: &str) {
    let (status, sent) = sqs(
        emulator,
        "SendMessage",
        &json!({
            "QueueUrl": url,
            "MessageBody": json!({ "marker": marker }).to_string(),
        }),
    );
    assert_eq!(
        status, 200,
        "case {case:?}: SendMessage({url:?}) answered {status} {sent}"
    );
}

/// The `CurrentState` and `StateReason` the pipe `name` reports now.
fn pipe_state(emulator: &Started, case: &str, name: &str) -> (String, String) {
    let (status, body) = emulator.call("pipes", "GET", &format!("/v1/pipes/{name}"), &[], Vec::new());
    assert_eq!(
        status, 200,
        "case {case:?}: DescribePipe({name:?}) answered {status} {body}"
    );
    let described: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    (
        described["CurrentState"].as_str().unwrap_or_default().to_string(),
        described["StateReason"].as_str().unwrap_or_default().to_string(),
    )
}

/// Wait until the pipe `name` rests in one of `wanted` and answer what it then reports.
fn settled_pipe(emulator: &Started, case: &str, name: &str, wanted: &[&str]) -> (String, String) {
    let giving_up_at = Instant::now() + SETTLE;
    loop {
        let reported = pipe_state(emulator, case, name);
        if wanted.contains(&reported.0.as_str()) || Instant::now() >= giving_up_at {
            return reported;
        }
        std::thread::sleep(LOOK_AGAIN);
    }
}

/// RULE: on the emulator a client starts, the declared pipe whose role admits the calls that move
/// the record invokes its function target with it and consumes it, while the pipe beside it whose
/// role withholds the source's receive invokes nothing and leaves its record waiting on its
/// source.
#[test]
fn a_started_emulators_pipe_delivers_to_its_lambda_target_only_as_its_role_admits() {
    let case = "an SQS-sourced pipe with a Lambda target on a started emulator";
    let emulator = Started::start();
    let prefix = "pipes-role-scoped-lambda";
    let stack = format!("{prefix}-stack");

    // The deploy-and-settle walk is the shared harness's, which states the `CAPABILITY_IAM`
    // this template's roles need and waits for a terminal `StackStatus`.
    let outputs = deployed(&emulator, case, &stack, &template(prefix));
    let result_url = output(&outputs, case, "ResultUrl");
    let open_url = output(&outputs, case, "OpenSourceUrl");
    let withheld_url = output(&outputs, case, "WithheldSourceUrl");

    // The pipe whose role admits every call is up; the one whose role withholds the receive
    // rests in a startup state a client can read.
    let open_pipe = format!("{prefix}-open-pipe");
    let (open_state, open_reason) = settled_pipe(&emulator, case, &open_pipe, &[RUNNING]);
    assert_eq!(
        open_state, RUNNING,
        "case {case:?}: expected the open pipe {RUNNING}, it reports {open_state:?} \
         ({open_reason:?}); the log:\n{}",
        emulator.log()
    );
    let withheld_pipe = format!("{prefix}-withheld-pipe");
    let (withheld_state, withheld_reason) =
        settled_pipe(&emulator, case, &withheld_pipe, &TERMINAL_STARTUP_STATES);
    assert!(
        TERMINAL_STARTUP_STATES.contains(&withheld_state.as_str()),
        "case {case:?}: expected the withheld pipe to rest in one of \
         {TERMINAL_STARTUP_STATES:?}, it reports {withheld_state:?} ({withheld_reason:?})"
    );
    // A pipe that could not be brought UP says which call it may not make: the `StateReason`
    // of a startup failure names the call that takes the records, which is the one this role
    // withholds. A pipe that is up carries no such reason, and is refused at each poll instead.
    assert!(
        withheld_state == RUNNING || withheld_reason.contains(RECORD_TAKING_CALL),
        "case {case:?}: expected a pipe that could not start to report why — a StateReason \
         naming {RECORD_TAKING_CALL:?} — got {withheld_state:?} ({withheld_reason:?})"
    );

    let open_marker = format!("{prefix}-open-marker");
    let withheld_marker = format!("{prefix}-withheld-marker");
    send(&emulator, case, &open_url, &open_marker);
    send(&emulator, case, &withheld_url, &withheld_marker);

    // The open pipe's record reaches the function, which forwards its marker.
    let giving_up_at = Instant::now() + SETTLE;
    let mut forwarded: Vec<String> = Vec::new();
    while Instant::now() < giving_up_at {
        forwarded = forwarded_markers(&emulator, case, &result_url);
        if forwarded.contains(&open_marker) {
            break;
        }
        std::thread::sleep(LOOK_AGAIN);
    }
    assert!(
        forwarded.contains(&open_marker),
        "case {case:?}: expected the open pipe to invoke the function with its record, which \
         forwards {open_marker:?}; the result queue carries {forwarded:?}; the log:\n{}",
        emulator.log()
    );

    // And the record it delivered is gone from its source.
    let giving_up_at = Instant::now() + SETTLE;
    let mut held = -1;
    while Instant::now() < giving_up_at {
        held = counted(&emulator, case, &open_url, AVAILABLE)
            + counted(&emulator, case, &open_url, IN_FLIGHT);
        if held == 0 {
            break;
        }
        std::thread::sleep(LOOK_AGAIN);
    }
    assert_eq!(
        held, 0,
        "case {case:?}: expected the delivered record deleted from its source, the queue still \
         holds {held}"
    );

    // The withheld pipe invokes nothing: its marker is never forwarded, however long the result
    // queue is watched.
    let watching_until = Instant::now() + QUIET;
    let mut seen: Vec<String> = Vec::new();
    while Instant::now() < watching_until {
        seen.extend(forwarded_markers(&emulator, case, &result_url));
        std::thread::sleep(LOOK_AGAIN);
    }
    assert!(
        !seen.contains(&withheld_marker),
        "case {case:?}: expected no invocation for the pipe that may not poll its source, the \
         result queue carries {withheld_marker:?} among {seen:?}"
    );

    // Its record was never TAKEN: it is still available to another consumer, with nothing in
    // flight, and that consumer's own receive is handed it.
    let available = counted(&emulator, case, &withheld_url, AVAILABLE);
    let in_flight = counted(&emulator, case, &withheld_url, IN_FLIGHT);
    assert_eq!(
        (available, in_flight),
        (1, 0),
        "case {case:?}: expected the record still available on the withheld source with nothing \
         in flight, got {AVAILABLE}={available} {IN_FLIGHT}={in_flight}"
    );
    let waiting = visible_bodies(&emulator, case, &withheld_url)
        .iter()
        .filter_map(|body| marker_of(body))
        .collect::<Vec<String>>();
    assert!(
        waiting.contains(&withheld_marker),
        "case {case:?}: expected the source to hand {withheld_marker:?} to another consumer, its \
         receive answered {waiting:?}"
    );
}
