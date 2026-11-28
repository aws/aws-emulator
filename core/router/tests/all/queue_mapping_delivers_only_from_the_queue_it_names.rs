// SPDX-License-Identifier: CC0-1.0

//! WHICH queue an Amazon SQS event source mapping consumes on the emulator a customer
//! STARTS: the built binary, on its own port, with request authentication on, reached over
//! TLS with SigV4-signed calls.
//!
//! The RULE pinned here, in one sentence: **a mapping reporting `State` `Enabled` receives
//! from exactly the one event source its `EventSourceArn` names — "the Amazon Resource Name
//! (ARN) of the event source", of which "Amazon Simple Queue Service – The ARN of the queue"
//! is this suite's kind (`smithy_query(query="operation",
//! operation="CreateEventSourceMapping")`) — and invokes the mapped function with the records
//! of THAT queue, while the messages of every other queue the account holds stay where they
//! were sent, never delivered and still receivable by a client.**
//!
//! That is the wiring guarantee a client depends on twice over: a queue-triggered function
//! that is never invoked for its own queue is silently inert, and one invoked for a queue its
//! mapping does not name is handed records it was never wired to. Both halves are asserted
//! per row, and the positive half FIRST, because an absence measured against a pipeline that
//! delivers nothing proves nothing.
//!
//! Four crates have to agree for it to hold, so the suite lives here: Amazon SQS holds every
//! queue and decides who may receive from one, IAM answers for the execution role the poller
//! reads the source as and the handler sends as (through the policy AWS PUBLISHES for a
//! queue-triggered function, attached by ARN), CloudFormation archives the inline handler into
//! the deployment package Lambda runs, and Lambda holds the function, the mapping and the
//! poller that is this suite's subject.
//!
//! The rows differ along what the rule generalises over: how many queues exist beside the
//! mapped one (one and two), WHICH of them the mapping names (the last of two, the first of
//! three), whether a queue it does not name is standard or FIFO, and how many records one
//! batch may hold (one, and a ceiling above everything waiting).
//!
//! Every observation is one a client makes — `ReceiveMessage` and `GetQueueAttributes` on the
//! queues, and Lambda's own reads — and every wait is on a CONDITION with a generous
//! deadline, so a loaded host delays a leg instead of weakening it. The drains are LONG POLLS
//! that delete nothing, which is how a client watches a queue it does not own, and the absence
//! leg looks again over a window of several poll intervals rather than trusting one look.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::cfn_deployed_pipeline_on_a_started_emulator::{deployed, output, Started};

/// The `State` a mapping polls its event source in
/// (`EventSourceMappingConfiguration$State`).
const STATE_ENABLED: &str = "Enabled";

/// Where Lambda binds the mapping collection.
const MAPPINGS_URI: &str = "/2015-03-31/event-source-mappings";

/// The attribute counting the messages a queue can hand over right now:
/// "ApproximateNumberOfMessages — the approximate number of visible messages in a queue".
const VISIBLE_MESSAGES: &str = "ApproximateNumberOfMessages";

/// The environment variable the handler reads the queue it forwards to from.
const RESULTS_VARIABLE: &str = "RESULTS_QUEUE_URL";

/// The policy AWS publishes for the execution role of a queue-triggered function: the three
/// calls Lambda makes to consume a queue on the function's behalf, beside the log writes.
/// Attached by ARN, so the document in force is the one IAM resolves for it.
const QUEUE_EXECUTION_POLICY: &str =
    "arn:aws:iam::aws:policy/service-role/AWSLambdaSQSQueueExecutionRole";

/// How long each leg's condition is waited for, how long a drain's own long poll waits, and
/// how often this suite looks while waiting. Every wait is on a CONDITION; the spans are
/// generous, so a loaded host spends none of them.
const SETTLE_DEADLINE: Duration = Duration::from_secs(150);
/// See [`SETTLE_DEADLINE`].
const LONG_POLL_SECONDS: i64 = 5;
/// See [`SETTLE_DEADLINE`].
const LOOK_INTERVAL: Duration = Duration::from_millis(200);

/// How many long-polling looks the absence leg takes once the positive control has been
/// delivered: the delivery it forbids had every chance to be made while they ran.
const QUIET_LOOKS: usize = 2;

/// The function every row runs: it forwards the body of every record it is handed to the
/// queue its environment names, with the SDK its managed runtime supplies, and reports no
/// record as unprocessed. So a body on the results queue is one a mapping really delivered to
/// a handler that ran.
const FORWARDING_HANDLER: &str = r#"import os

import boto3

QUEUES = boto3.client("sqs")
RESULTS = os.environ["RESULTS_QUEUE_URL"]


def handler(event, context):
    records = event["Records"]
    if not records:
        raise ValueError("a batch was handed over holding no record")
    for record in records:
        QUEUES.send_message(QueueUrl=RESULTS, MessageBody=record["body"])
    return {"batchItemFailures": []}
"#;

/// One queue of a row: the suffix its name is built from, whether it is FIFO, and the body
/// sent to it while the mapping polls.
struct Queue {
    label: &'static str,
    fifo: bool,
    body: &'static str,
}

/// One row: the queues that exist beside the results queue, which of them the mapping names,
/// and the `BatchSize` it reads that one under.
struct Row {
    case: &'static str,
    queues: &'static [Queue],
    /// The index in `queues` of the queue the mapping's `EventSourceArn` names.
    mapped: usize,
    /// "The maximum number of records in each batch that Lambda pulls from your stream or
    /// queue" (`CreateEventSourceMapping$BatchSize`).
    batch_size: i64,
}

/// The catalog: the mapped queue last of two and first of three, one record per batch and a
/// ceiling above everything waiting, and a FIFO queue among the ones the mapping does not
/// name.
const ROWS: [Row; 2] = [
    Row {
        case: "two standard queues, the second mapped, one record per batch",
        queues: &[
            Queue {
                label: "unwired",
                fifo: false,
                body: "sent-to-a-queue-the-mapping-does-not-name",
            },
            Queue {
                label: "wired",
                fifo: false,
                body: "sent-to-the-mapped-queue",
            },
        ],
        mapped: 1,
        batch_size: 1,
    },
    Row {
        case: "three queues, the first mapped, one of the others FIFO",
        queues: &[
            Queue {
                label: "wired",
                fifo: false,
                body: "sent-to-the-queue-the-mapping-names",
            },
            Queue {
                label: "unwired",
                fifo: false,
                body: "sent-to-another-standard-queue",
            },
            Queue {
                label: "unwired-fifo",
                fifo: true,
                body: "sent-to-a-fifo-queue-nothing-maps",
            },
        ],
        mapped: 0,
        batch_size: 10,
    },
];

/// One Amazon SQS call, which must answer 200.
fn sqs(emulator: &Started, case: &str, operation: &str, body: &Value) -> Value {
    let (status, text) = emulator.call(
        "sqs",
        "POST",
        "/",
        &[
            ("content-type", "application/x-amz-json-1.0"),
            ("x-amz-target", &format!("AmazonSQS.{operation}")),
        ],
        body.to_string().into_bytes(),
    );
    assert_eq!(
        status, 200,
        "case {case:?}: Amazon SQS {operation} must answer 200, got {status} with {text}; the \
         log:\n{}",
        emulator.log()
    );
    serde_json::from_str(&text).unwrap_or(Value::Null)
}

/// One Lambda `restJson1` call, as its status and parsed body.
fn lambda(emulator: &Started, method: &str, path: &str, body: Option<&Value>) -> (u16, Value) {
    let payload = body.map(Value::to_string).unwrap_or_default();
    let (status, text) = emulator.call(
        "lambda",
        method,
        path,
        &[("content-type", "application/json")],
        payload.into_bytes(),
    );
    let parsed = if text.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&text).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// A queue created with the attributes `attributes` names: its url and the ARN it reports for
/// itself, so a mapping is made on the ARN the queue itself published.
fn given_queue(emulator: &Started, case: &str, name: &str, attributes: &Value) -> (String, String) {
    let created = sqs(
        emulator,
        case,
        "CreateQueue",
        &json!({ "QueueName": name, "Attributes": attributes }),
    );
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the queue {name:?} reports no url: {created}"))
        .to_string();
    let arn = queue_attribute(emulator, case, &url, "QueueArn");
    (url, arn)
}

/// One attribute of the queue `url` addresses, as `GetQueueAttributes` reports it.
fn queue_attribute(emulator: &Started, case: &str, url: &str, name: &str) -> String {
    let read = sqs(
        emulator,
        case,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": [name] }),
    );
    read["Attributes"][name]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the queue reports no {name}: {read}"))
        .to_string()
}

/// Every body ONE long poll of the queue `url` addresses hands over, deleting nothing — which
/// is how a client watches a queue for what a pipeline writes to it.
fn handed_over(emulator: &Started, case: &str, url: &str) -> Vec<String> {
    let received = sqs(
        emulator,
        case,
        "ReceiveMessage",
        &json!({
            "QueueUrl": url,
            "MaxNumberOfMessages": 10,
            "WaitTimeSeconds": LONG_POLL_SECONDS,
        }),
    );
    received["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| message["Body"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Wait until the mapping `uuid` names reports `state`, which the CLOCK owes.
fn await_state(emulator: &Started, case: &str, uuid: &str, state: &str) {
    let giving_up_at = Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, read) = lambda(emulator, "GET", &format!("{MAPPINGS_URI}/{uuid}"), None);
        if status == 200 && read["State"].as_str() == Some(state) {
            return;
        }
        assert!(
            Instant::now() < giving_up_at,
            "case {case:?}: the mapping {uuid} never reported {state}, last read {status} \
             {read}; the log:\n{}",
            emulator.log()
        );
        std::thread::sleep(LOOK_INTERVAL);
    }
}

/// The stack a row deploys: the execution role Lambda reads the source queue as and the
/// handler sends as, and the forwarding function running as it.
///
/// The role carries the policy AWS PUBLISHES for a queue-triggered function, which is how
/// such a role is granted in practice, beside the ONE send the handler itself makes — on the
/// results queue alone. Nothing else: a delivery that needed a further grant would not be made
/// here.
fn wiring(results_arn: &str, results_url: &str) -> Value {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "ConsumerRole": {
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
                    "ManagedPolicyArns": [QUEUE_EXECUTION_POLICY],
                    "Policies": [{
                        "PolicyName": "forward-to-the-results-queue",
                        "PolicyDocument": {
                            "Version": "2012-10-17",
                            "Statement": [{
                                "Effect": "Allow",
                                "Action": "sqs:SendMessage",
                                "Resource": results_arn,
                            }],
                        },
                    }],
                },
            },
            "Consumer": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    "Timeout": 10,
                    "Role": { "Fn::GetAtt": ["ConsumerRole", "Arn"] },
                    "Environment": { "Variables": { RESULTS_VARIABLE: results_url } },
                    "Code": { "ZipFile": FORWARDING_HANDLER },
                },
            },
        },
        "Outputs": { "FunctionName": { "Value": { "Ref": "Consumer" } } },
    })
}

/// The attributes a row's queue is created with: what makes a FIFO queue one ("If you create a
/// FIFO queue, your queue name must end with the .fifo suffix", plus content-based
/// deduplication so a send needs no deduplication id), and nothing at all for a standard one.
fn attributes_of(queue: &Queue) -> Value {
    if queue.fifo {
        json!({ "FifoQueue": "true", "ContentBasedDeduplication": "true" })
    } else {
        json!({})
    }
}

/// The name a row's queue is created under, with the suffix a FIFO queue's name must end in.
fn queue_name(index: usize, queue: &Queue) -> String {
    let base = format!("esm-source-selection-{index}-{}", queue.label);
    if queue.fifo {
        format!("{base}.fifo")
    } else {
        base
    }
}

/// `SendMessage` of `body` to the queue `url` addresses, carrying the message group a FIFO
/// queue requires and a standard queue takes no notice of.
fn send(emulator: &Started, case: &str, url: &str, body: &str, fifo: bool) {
    let mut request = json!({ "QueueUrl": url, "MessageBody": body });
    if fifo {
        request["MessageGroupId"] = json!("one-group");
    }
    sqs(emulator, case, "SendMessage", &request);
}

/// RULE: on the emulator a client STARTS, a queue mapping reporting `Enabled` consumes the one
/// queue its `EventSourceArn` names and no other — its own queue's message reaches the running
/// handler, and the message of every queue it does not name is neither delivered nor taken off
/// that queue.
#[test]
fn a_queue_mapping_consumes_only_the_queue_its_event_source_arn_names() {
    let emulator = Started::start();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        // The queue the handler forwards to, which no mapping names: where the records that
        // reached the function's own run are read back.
        let (results_url, results_arn) = given_queue(
            &emulator,
            case,
            &format!("esm-source-selection-{index}-results"),
            &json!({}),
        );
        let queues: Vec<(String, String)> = row
            .queues
            .iter()
            .map(|queue| {
                given_queue(
                    &emulator,
                    case,
                    &queue_name(index, queue),
                    &attributes_of(queue),
                )
            })
            .collect();
        let (mapped_url, mapped_arn) = queues[row.mapped].clone();

        let template = wiring(&results_arn, &results_url).to_string();
        let outputs = deployed(
            &emulator,
            case,
            &format!("esm-source-selection-{index}"),
            &template,
        );
        let function = output(&outputs, case, "FunctionName");

        let (status, created) = lambda(
            &emulator,
            "POST",
            MAPPINGS_URI,
            Some(&json!({
                "FunctionName": function,
                "EventSourceArn": mapped_arn,
                "BatchSize": row.batch_size,
                "Enabled": true,
            })),
        );
        assert_eq!(
            status, 202,
            "case {case:?}: CreateEventSourceMapping must answer 202, got {status} with \
             {created}; the log:\n{}",
            emulator.log()
        );
        let uuid = created["UUID"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the created mapping reports no UUID: {created}")
            })
            .to_string();
        await_state(&emulator, case, &uuid, STATE_ENABLED);

        // What is waiting, on every queue of the row: the queue the mapping names last, so
        // that a message on a queue it does not name has been waiting for longer than the one
        // it must deliver.
        for (position, (queue, (url, _))) in row.queues.iter().zip(&queues).enumerate() {
            if position == row.mapped {
                continue;
            }
            send(&emulator, case, url, queue.body, queue.fifo);
        }
        let mapped_body = row.queues[row.mapped].body;
        send(
            &emulator,
            case,
            &mapped_url,
            mapped_body,
            row.queues[row.mapped].fifo,
        );

        // The positive control, which comes first: the mapped queue's message reaches the
        // running handler, so the body it forwards appears on the results queue.
        let giving_up_at = Instant::now() + SETTLE_DEADLINE;
        let mut forwarded: BTreeSet<String> = BTreeSet::new();
        loop {
            forwarded.extend(handed_over(&emulator, case, &results_url));
            if forwarded.contains(mapped_body) {
                break;
            }
            assert!(
                Instant::now() < giving_up_at,
                "case {case:?}: the mapping must hand its function the message its own queue \
                 was sent, so {mapped_body:?} must be forwarded; the handler forwarded \
                 {forwarded:?} and the mapping reads {:?}; the log:\n{}",
                lambda(&emulator, "GET", &format!("{MAPPINGS_URI}/{uuid}"), None).1,
                emulator.log()
            );
            std::thread::sleep(LOOK_INTERVAL);
        }

        // And the absence, measured against that same pipeline: no body of a queue the
        // mapping does not name is ever forwarded, however long this looks.
        let unwired: BTreeSet<String> = row
            .queues
            .iter()
            .enumerate()
            .filter(|(position, _)| *position != row.mapped)
            .map(|(_, queue)| queue.body.to_string())
            .collect();
        for _ in 0..QUIET_LOOKS {
            forwarded.extend(handed_over(&emulator, case, &results_url));
            let wrongly: BTreeSet<&String> = forwarded.intersection(&unwired).collect();
            assert!(
                wrongly.is_empty(),
                "case {case:?}: a mapping reads only the queue it names, so no body of another \
                 queue may be forwarded, got {wrongly:?}; the log:\n{}",
                emulator.log()
            );
        }

        // The messages nothing polled are still their own queues' to hand over, which is what
        // makes the absence a withholding rather than a loss.
        for (position, (queue, (url, _))) in row.queues.iter().zip(&queues).enumerate() {
            if position == row.mapped {
                continue;
            }
            let visible = queue_attribute(&emulator, case, url, VISIBLE_MESSAGES);
            assert_eq!(
                visible, "1",
                "case {case:?}: the queue {:?} is not the mapping's event source, so its \
                 message must still be waiting on it, got {visible:?}",
                queue.label
            );
        }
    }
}
