// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation-deployed queue/dead-letter-queue pair really QUARANTINES a poison
//! message: the stack's Lambda event source mapping polls the source queue, the deployment
//! package it invokes raises on every record it is handed, and the source queue's own
//! `RedrivePolicy` moves the message to the dead-letter queue the policy names once the
//! deliveries it allows are used up.
//!
//! The rule pinned here, in one sentence: **while an event source mapping on an Amazon SQS
//! queue is enabled, each pass hands a batch over through the queue's own delivery bookkeeping
//! and deletes nothing the invocation did not process, so a message no invocation ever
//! processes is delivered as many times as its queue's `RedrivePolicy` allows
//! ("maxReceiveCount — the number of times a message is delivered to the source queue before
//! being moved to the dead-letter queue") and is then moved, body for body, to that policy's
//! `deadLetterTargetArn` — leaving the source queue holding nothing.** None of that depends on
//! how the queues, the function or the mapping came to exist: a stack's `AWS::SQS::Queue`,
//! `AWS::Lambda::Function` and `AWS::Lambda::EventSourceMapping` are the same queue, function
//! and mapping a client's own `CreateQueue`, `CreateFunction` and `CreateEventSourceMapping`
//! leave behind.
//!
//! Four crates have to agree for that to hold — CloudFormation provisions both queues (with
//! the redrive document the template declares), the function and the mapping; Lambda stands
//! the poller up, invokes the package and leaves the un-processed batch where it was; Amazon
//! SQS counts the deliveries and performs the move; IAM answers for the execution role the
//! poller's reads are judged against — so the suite lives here rather than in any one crate.
//! Every observation is made where a client makes it: on the two queue URLs the stack
//! publishes and on Lambda's own `GetEventSourceMapping`, never on an emulator internal.
//!
//! The rows differ along exactly what the rule generalises over: how many deliveries the
//! policy allows (the published minimum of `maxReceiveCount` and two values above it), how
//! long the queue hides a delivered message for, how many messages one batch may hold, how
//! many are waiting, and what the bodies are (plain text, JSON, non-ASCII text and a large
//! body).
//!
//! `crate::cloudformation_provisions_lambda_event_source_mappings` is the suite for whether
//! the declared mapping exists at all and what refuses it; this one is the suite for what the
//! deployed pipeline DOES to a message nothing can process.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::cloudformation_provisions_lambda_event_source_mappings::{
    mapping_until_settled, MAPPING_STATE_ENABLED,
};
use crate::query_wire::outputs_of;
use crate::sns_lambda_subscription_permission_boundary::{
    build_router, cloudformation, iam, lambda, sqs, trust_policy, ACCOUNT, CREATE_COMPLETE,
    LOOK_INTERVAL,
};

/// The Region every request of this suite is signed for — the one `wait_for_stack_status`
/// polls in — and so the Region the stack's resources belong to.
const REGION: &str = "us-east-1";

/// The reads a mapping's poller makes on the queue it polls, which the invoked function's
/// execution role has to be granted for the mapping to be created at all. Source: the actions
/// AWS's own `AWSLambdaSQSQueueExecutionRole` grants for an Amazon SQS event source.
const QUEUE_READS: [&str; 3] = [
    "sqs:ReceiveMessage",
    "sqs:DeleteMessage",
    "sqs:GetQueueAttributes",
];

/// The attributes one answer reports a queue's whole holding as: "ApproximateNumberOfMessages
/// — the approximate number of visible messages in a queue", `…NotVisible` — "the approximate
/// number of messages that are in flight" — and `…Delayed`.
const HOLDING_ATTRIBUTES: [&str; 3] = [
    "ApproximateNumberOfMessages",
    "ApproximateNumberOfMessagesNotVisible",
    "ApproximateNumberOfMessagesDelayed",
];

/// How long a row waits for the deployed pipeline to reach what the rule owes before failing.
///
/// Every step of the chain is driven by the CLOCK (the stack settles, the function goes
/// `Active`, the mapping settles into `Enabled`, each delivery's visibility window lapses, the
/// move to the dead-letter queue falls due), so the wait is on that CONDITION — looked for
/// again until it holds — and this bound is generous slack whose only job is that the wait
/// TERMINATES. Nothing below asserts on elapsed time.
const SETTLE_BUDGET: Duration = Duration::from_secs(120);

/// The handler every row's deployment package runs: every message it is handed raises, so no
/// invocation ever processes anything and the source queue's `RedrivePolicy` is the only thing
/// that decides where the message ends up.
const RAISING_HANDLER: &str = r#"
def lambda_handler(event, context):
    records = event["Records"]
    if not records:
        raise ValueError("a batch was handed over holding no record")
    raise RuntimeError("this consumer cannot process " + repr(records[0]["body"]))
"#;

/// The identity policy the execution role carries: the poller reads the source queue acting as
/// that role, so a role short of one of [`QUEUE_READS`] is a mapping Lambda refuses outright.
fn poller_policy(queue_arn: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": QUEUE_READS,
            "Resource": queue_arn,
        }],
    })
    .to_string()
}

/// Create the execution role the row's template names, where IAM holds it: trusted by Lambda,
/// and granting the poller's own reads on the queue the mapping will poll.
async fn execution_role(router: &MultiServiceRouter, role: &str, queue_arn: &str) {
    iam(
        router,
        "CreateRole",
        &[
            ("RoleName", role),
            ("AssumeRolePolicyDocument", &trust_policy()),
        ],
    )
    .await;
    iam(
        router,
        "PutRolePolicy",
        &[
            ("RoleName", role),
            ("PolicyName", "poll-the-source-queue"),
            ("PolicyDocument", &poller_policy(queue_arn)),
        ],
    )
    .await;
}

/// The ARN a queue of `name` carries in this suite's Region.
fn queue_arn(name: &str) -> String {
    format!("arn:aws:sqs:{REGION}:{ACCOUNT}:{name}")
}

/// One row: how long the source queue hides a delivered message for, how many deliveries its
/// `RedrivePolicy` allows, how many messages one batch may hold, and the bodies waiting on it.
struct Row {
    case: &'static str,
    visibility_timeout: i64,
    max_receive_count: i64,
    batch_size: i64,
    bodies: Vec<String>,
}

/// The catalog. `maxReceiveCount` spans the published minimum of its range (1, from the
/// `RedrivePolicy` queue attribute's own documentation — "valid values are from 1 to 1000") and
/// two values above it; the other columns span a queue that hides a delivered message for two
/// seconds and for three, one waiting message and two, a batch ceiling of one and of several,
/// and bodies that are plain text, JSON, non-ASCII text and large.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one plain body, the minimum of one delivery, one message per batch",
            visibility_timeout: 2,
            max_receive_count: 1,
            batch_size: 1,
            bodies: vec!["poison-plain".to_string()],
        },
        Row {
            case: "a json body, two deliveries allowed, ten per batch",
            visibility_timeout: 3,
            max_receive_count: 2,
            batch_size: 10,
            bodies: vec![json!({ "order": 17, "items": ["a", "b"] }).to_string()],
        },
        Row {
            case: "a non-ascii body and a large one, three deliveries allowed",
            visibility_timeout: 2,
            max_receive_count: 3,
            batch_size: 2,
            bodies: vec!["ünïcödé ⚑ poison".to_string(), "x".repeat(2_048)],
        },
    ]
}

/// The template one row deploys: the dead-letter queue, the source queue whose `RedrivePolicy`
/// names it, the function whose package always raises, and the event source mapping that polls
/// the source queue and invokes that function.
///
/// The function's `Timeout` is the queue's own visibility timeout: a queue hands a message to
/// one consumer for that long, so Lambda refuses a mapping onto a function that could outlast
/// the window.
fn template(stack: &str, row: &Row, role: &str) -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "DeadLetters": {
                "Type": "AWS::SQS::Queue",
                "Properties": { "QueueName": format!("{stack}-dlq") },
            },
            "Source": {
                "Type": "AWS::SQS::Queue",
                "Properties": {
                    "QueueName": format!("{stack}-source"),
                    "VisibilityTimeout": row.visibility_timeout,
                    "RedrivePolicy": {
                        "deadLetterTargetArn": { "Fn::GetAtt": ["DeadLetters", "Arn"] },
                        "maxReceiveCount": row.max_receive_count,
                    },
                },
            },
            "Consumer": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": format!("{stack}-consumer"),
                    "Role": format!("arn:aws:iam::{ACCOUNT}:role/{role}"),
                    "Runtime": "python3.12",
                    "Handler": "index.lambda_handler",
                    "Timeout": row.visibility_timeout,
                    "Code": { "ZipFile": RAISING_HANDLER },
                },
            },
            "Mapping": {
                "Type": "AWS::Lambda::EventSourceMapping",
                "Properties": {
                    "EventSourceArn": { "Fn::GetAtt": ["Source", "Arn"] },
                    "FunctionName": { "Ref": "Consumer" },
                    "BatchSize": row.batch_size,
                    "Enabled": true,
                },
            },
        },
        "Outputs": {
            "SourceUrl": { "Value": { "Ref": "Source" } },
            "DeadLetterUrl": { "Value": { "Ref": "DeadLetters" } },
            "MappingId": { "Value": { "Ref": "Mapping" } },
        },
    })
    .to_string()
}

/// `SendMessage` of `body` to the queue `url` addresses.
async fn send_message(router: &MultiServiceRouter, url: &str, body: &str) {
    sqs(
        router,
        "SendMessage",
        &json!({ "QueueUrl": url, "MessageBody": body }),
    )
    .await;
}

/// What the queue `url` addresses holds right now, as (visible, in flight, delayed), from ONE
/// answer: a message a poller is holding in flight at this instant is one the queue still has,
/// and a total assembled from counts read in SEPARATE answers would count it twice or not at
/// all.
async fn holding(router: &MultiServiceRouter, url: &str) -> (u32, u32, u32) {
    let answered = sqs(
        router,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": HOLDING_ATTRIBUTES }),
    )
    .await;
    let count = |name: &str| -> u32 {
        answered["Attributes"][name]
            .as_str()
            .and_then(|raw| raw.parse::<u32>().ok())
            .unwrap_or(0)
    };
    (
        count(HOLDING_ATTRIBUTES[0]),
        count(HOLDING_ATTRIBUTES[1]),
        count(HOLDING_ATTRIBUTES[2]),
    )
}

/// How many messages a queue holds in all, from one reading of [`holding`].
fn held(counts: (u32, u32, u32)) -> u32 {
    counts.0 + counts.1 + counts.2
}

/// One `ReceiveMessage` on the queue `url` addresses: the `(body, ApproximateReceiveCount)` of
/// every message it handed over. The hold is long enough that a message counted here is not
/// handed to the same reader again while it waits for the rest.
async fn receive(router: &MultiServiceRouter, url: &str) -> Vec<(String, u32)> {
    let answered = sqs(
        router,
        "ReceiveMessage",
        &json!({
            "QueueUrl": url,
            "MaxNumberOfMessages": 10,
            "VisibilityTimeout": 300,
            "MessageSystemAttributeNames": ["ApproximateReceiveCount"],
        }),
    )
    .await;
    answered["Messages"]
        .as_array()
        .map(|messages| {
            messages
                .iter()
                .map(|message| {
                    let body = message["Body"].as_str().unwrap_or_default().to_string();
                    let count = message["Attributes"]["ApproximateReceiveCount"]
                        .as_str()
                        .and_then(|raw| raw.parse::<u32>().ok())
                        .unwrap_or(0);
                    (body, count)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Poll `GetFunctionConfiguration` until Lambda reports `name` `Active` — "the function is
/// ready to be invoked" (`FunctionConfiguration$State`) — so a batch the mapping hands over is
/// refused by the handler rather than by the function's own lifecycle.
async fn wait_until_active(router: &MultiServiceRouter, case: &str, name: &str) {
    let deadline = Instant::now() + SETTLE_BUDGET;
    let mut last = Value::Null;
    while Instant::now() < deadline {
        let (_, reported) = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{name}/configuration"),
            None,
        )
        .await;
        if reported.get("State").and_then(Value::as_str) == Some("Active") {
            return;
        }
        last = reported;
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    panic!("case {case:?}: Lambda never reported {name} Active, last saw {last}");
}

/// RULE: a message no invocation of a deployed mapping's function ever processes is delivered
/// as many times as the source queue's `RedrivePolicy` allows and is then moved to the
/// dead-letter queue that policy names, body for body, leaving the source queue empty.
#[tokio::test]
async fn a_stack_deployed_redrive_policy_moves_a_poison_message_to_its_dead_letter_queue() {
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let router = build_router();
        let stack = format!("sqs-redrive-deploy-{index}");
        let role = format!("{stack}-exec");
        let source_name = format!("{stack}-source");

        // The execution role the template names, created where IAM holds it and granted
        // exactly the reads the poller makes on the queue the mapping will poll.
        execution_role(&router, &role, &queue_arn(&source_name)).await;

        cloudformation(
            &router,
            "CreateStack",
            &[
                ("StackName", &stack),
                ("TemplateBody", &template(&stack, row, &role)),
            ],
        )
        .await;
        let (status, described) = wait_for_stack_status(&router, &stack, CREATE_COMPLETE).await;
        assert_eq!(
            status, CREATE_COMPLETE,
            "case {case:?}: the stack must deploy the queue pair, the consumer and the \
             mapping, got {status} with {described}"
        );
        let outputs = outputs_of(&described);
        let published = |name: &str| {
            outputs.get(name).cloned().unwrap_or_else(|| {
                panic!("case {case:?}: the stack must publish {name}, got {described}")
            })
        };
        let source_url = published("SourceUrl");
        let dead_letter_url = published("DeadLetterUrl");
        let mapping_id = published("MappingId");

        // The mapping the stack published is one Lambda resolves and really polls the declared
        // source: the round trip through that service's own read, before anything is sent.
        wait_until_active(&router, case, &format!("{stack}-consumer")).await;
        let settled = mapping_until_settled(&router, case, &mapping_id).await;
        assert_eq!(
            (
                settled.get("State").and_then(Value::as_str),
                settled.get("EventSourceArn").and_then(Value::as_str),
            ),
            (
                Some(MAPPING_STATE_ENABLED),
                Some(queue_arn(&source_name).as_str()),
            ),
            "case {case:?}: the deployed mapping must settle into {MAPPING_STATE_ENABLED} \
             polling the declared source queue, got {settled}"
        );

        for body in &row.bodies {
            send_message(&router, &source_url, body).await;
        }

        // The whole chain, waited for at its END: the poller delivers each message, the
        // handler raises so nothing is deleted, the visibility window lapses, the delivery is
        // made again, and once the policy's count is used up the message is moved to the
        // dead-letter queue and the source queue drains.
        let mut seen: BTreeMap<String, u32> = BTreeMap::new();
        let deadline = Instant::now() + SETTLE_BUDGET;
        let mut drained: Option<(u32, u32, u32)> = None;
        while Instant::now() < deadline {
            for (body, count) in receive(&router, &dead_letter_url).await {
                seen.entry(body).or_insert(count);
            }
            let counts = holding(&router, &source_url).await;
            if seen.len() == row.bodies.len() && held(counts) == 0 {
                drained = Some(counts);
                break;
            }
            tokio::time::sleep(LOOK_INTERVAL).await;
        }

        // Every body that was sent is on the dead-letter queue, byte for byte: the message is
        // MOVED, not summarised.
        let quarantined: Vec<String> = seen.keys().cloned().collect();
        let mut expected = row.bodies.clone();
        expected.sort();
        assert_eq!(
            quarantined, expected,
            "case {case:?}: the dead-letter queue must hold exactly the bodies that were sent, \
             got {quarantined:?}"
        );
        // "The number of times a message is delivered to the source queue before being moved
        // to the dead-letter queue": the source queue made exactly that many deliveries, and
        // the read above is one more — `ApproximateReceiveCount` is "the number of times a
        // message has been received across all queues but not deleted".
        let expected_count = u32::try_from(row.max_receive_count).unwrap_or(1) + 1;
        for (body, count) in &seen {
            assert_eq!(
                *count, expected_count,
                "case {case:?}: the message {body:.32?} must have been delivered {} times \
                 before the move, got a receive count of {count} on the dead-letter queue",
                row.max_receive_count
            );
        }
        // And the source queue is empty: the messages LEFT it as they reached the dead-letter
        // one, so an operator polling the source sees nothing waiting.
        assert_eq!(
            drained.map(held),
            Some(0),
            "case {case:?}: the source queue must report no messages left after the redrive, \
             got {drained:?} as (visible, in flight, delayed)"
        );
    }
}
