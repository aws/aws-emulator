// SPDX-License-Identifier: CC0-1.0

//! The optimized Amazon SNS publish a Step Functions `Task` state names, driven across
//! the four services the question spans — Step Functions runs the state, IAM holds the
//! documents the execution role is judged against, Amazon SNS serves the publish and
//! fans it out, and Amazon SQS holds what arrived — mounted on ONE engine exactly as
//! `core/router/src/lib.rs` mounts them.
//!
//! RULE: Step Functions makes a `Task` state's integration call ACTING AS the state
//! machine's `roleArn`, so the identity-based documents in force on that role decide
//! whether the call happens: documents that allow the invoked operation's published
//! action on the resource the request names make the call, and what the task reports is
//! the invoked service's own answer (for `arn:aws:states:::sns:publish`, the `MessageId`
//! of a notification its subscribers really receive); documents that withhold it make no
//! call at all, and the task fails with that service's own refusal of an unauthorized
//! call — worded with the action as THAT service publishes it, which for Amazon SNS is
//! `SNS:<ApiName>` rather than the ARN namespace — so the state's `Catch` can route on
//! it and the target is untouched.
//!
//! The rows differ along what the rule generalises over: whether the documents grant the
//! published action, whether the grant reaches the resource the request names (its own
//! ARN, a wildcard, another topic's ARN), whether they grant another action of the same
//! service instead, whether an explicit `Deny` sits beside a grant, and the case the
//! statement spells the action in (IAM folds an action's case, so the decision does not
//! change while the refusal's own wording does not follow the document).
//!
//! Four crates have to agree for that to hold, so the suite lives here rather than in
//! any one of them, and it borrows the role, the queue and the response readers
//! `all/sfn_task_target_authorization.rs` already owns: `tests/all.rs` is one crate root.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, form_encode};
use crate::sfn_task_target_authorization::{
    allow, logs_only, queue_handles, ran_with_input, role, session_principal, sfn, sqs, string,
    ACCOUNT, REGION,
};

/// The history events a task's two outcomes write
/// (`smithy_query(query="shape",shape="HistoryEventType")`).
const TASK_SUCCEEDED: &str = "TaskSucceeded";
const TASK_FAILED: &str = "TaskFailed";

/// The error name a `Task` state reports for an unauthorized publish: Amazon SNS models
/// that refusal as `AuthorizationErrorException`, and Step Functions names a failed
/// integration call `<service>.<exception class>`.
const AUTHORIZATION_ERROR: &str = "SNS.AuthorizationErrorException";

/// The IAM action Amazon SNS publishes for `Publish`, spelled the way that service
/// publishes its own actions — `SNS:<ApiName>` ("Actions defined by Amazon Simple
/// Notification Service", IAM Service Authorization Reference) — which is therefore the
/// action its refusal of an unauthorized publish names.
const PUBLISH_ACTION: &str = "SNS:Publish";

/// The metadata Amazon SNS's own SDK client appends to the refusal, with the id of the
/// refused request normalized away ([`normalized`]): the client the call was made with,
/// the status `AuthorizationErrorException` carries and the query code it is named by.
const REFUSAL_METADATA: &str = " (Service: AmazonSNS; Status Code: 403; Error Code: \
                                 AuthorizationError; Request ID: <id>; Proxy: null)";

/// The four services this question spans, over ONE shared engine.
fn emulator() -> MultiServiceRouter {
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
    router.register_service(
        service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        engine,
    ));
    router
}

/// One Amazon SNS awsQuery call (API version 2010-03-31) that must answer 200, as its
/// XML body.
async fn sns(router: &MultiServiceRouter, action: &str, members: &[(&str, &str)]) -> String {
    let mut body = format!("Action={action}&Version=2010-03-31");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sns.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/sns/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(Bytes::from(body))
        .expect("a well-formed awsQuery request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "expected HTTP 200 from Amazon SNS {action}, got {status} with {text}"
    );
    text
}

/// A queue that admits `sns.amazonaws.com`, and a topic it is subscribed to; answers the
/// queue's url and the topic's ARN.
async fn topic_with_subscriber(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let created = sqs(router, "CreateQueue", &json!({ "QueueName": name })).await;
    let (queue_url, queue_arn) = queue_handles(router, &created).await;
    let topic_arn = element(&sns(router, "CreateTopic", &[("Name", name)]).await, "TopicArn");
    let queue_policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "sns.amazonaws.com" },
            "Action": "sqs:SendMessage",
            "Resource": queue_arn,
            "Condition": { "ArnEquals": { "aws:SourceArn": topic_arn } },
        }],
    });
    sqs(
        router,
        "SetQueueAttributes",
        &json!({
            "QueueUrl": queue_url,
            "Attributes": { "Policy": queue_policy.to_string() },
        }),
    )
    .await;
    sns(
        router,
        "Subscribe",
        &[
            ("TopicArn", topic_arn.as_str()),
            ("Protocol", "sqs"),
            ("Endpoint", queue_arn.as_str()),
            ("ReturnSubscriptionArn", "true"),
        ],
    )
    .await;
    (queue_url, topic_arn)
}

/// The definition every row runs: one `Task` state publishing the execution input's
/// `message` to `topic_arn`, with a `Catch` on `States.ALL` routing a failure to a state
/// that ends the execution — the shape the rule's refusal arm has to be routable by.
fn definition(topic_arn: &str) -> String {
    json!({
        "StartAt": "PublishToSNS",
        "States": {
            "PublishToSNS": {
                "Type": "Task",
                "Resource": "arn:aws:states:::sns:publish",
                "Parameters": { "TopicArn": topic_arn, "Message.$": "$.message" },
                "Catch": [{
                    "ErrorEquals": ["States.ALL"],
                    "Next": "Denied",
                    "ResultPath": "$.errorInfo",
                }],
                "End": true,
            },
            "Denied": { "Type": "Pass", "End": true },
        },
    })
    .to_string()
}

/// Start one execution of `machine_arn` with `message` as its input and report it once it
/// has left `RUNNING`, as its ARN and the `DescribeExecution` that settled it.
///
/// The `StartExecution` and the poll to a terminal status are the harness's own
/// ([`ran_with_input`]); only the document the execution carries is this suite's.
async fn ran(
    router: &MultiServiceRouter,
    machine_arn: &str,
    message: &str,
    case: &str,
) -> (String, Value) {
    let input = json!({ "message": message }).to_string();
    ran_with_input(router, machine_arn, Some(&input), case).await
}

/// The history events of one execution.
async fn history(router: &MultiServiceRouter, execution_arn: &str) -> Vec<Value> {
    let answered = sfn(
        router,
        "GetExecutionHistory",
        &json!({ "executionArn": execution_arn, "maxResults": 1000 }),
    )
    .await;
    answered
        .get("events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Every event of `events` whose `type` is `named`.
fn of_type<'a>(events: &'a [Value], named: &str) -> Vec<&'a Value> {
    events
        .iter()
        .filter(|event| event.get("type").and_then(Value::as_str) == Some(named))
        .collect()
}

/// The notification bodies waiting on the queue at `url`, drained.
async fn delivered(router: &MultiServiceRouter, url: &str) -> Vec<String> {
    let received = sqs(
        router,
        "ReceiveMessage",
        &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
    )
    .await;
    received["Messages"]
        .as_array()
        .map(|messages| {
            messages
                .iter()
                .map(|message| message["Body"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// `message` with every request id it reports replaced by `<id>`, asserting on the way
/// that each one carries the 8-4-4-4-12 form an AWS request id has: the id identifies the
/// refused call, so it differs per refusal and is no part of the sentence.
fn normalized(case: &str, message: &str) -> String {
    const MARKER: &str = "Request ID: ";

    let mut rest = message;
    let mut out = String::with_capacity(message.len());
    while let Some(at) = rest.find(MARKER) {
        let (before, after) = rest.split_at(at + MARKER.len());
        out.push_str(before);
        let end = after.find(';').unwrap_or(after.len());
        let (id, tail) = after.split_at(end);
        let widths: Vec<usize> = id.split('-').map(str::len).collect();
        assert_eq!(
            widths,
            vec![8usize, 4, 4, 4, 12],
            "case {case:?}: expected the refused request's id in the 8-4-4-4-12 form, got {id:?}"
        );
        assert!(
            id.chars().all(|digit| digit.is_ascii_hexdigit() || digit == '-'),
            "case {case:?}: expected a hexadecimal request id, got {id:?}"
        );
        out.push_str("<id>");
        rest = tail;
    }
    out.push_str(rest);
    out
}

/// What a row expects of the task: the publish happened, or it was refused with the
/// clause naming WHY the documents withheld it.
enum Outcome {
    /// The call was made: the subscriber receives the message and the task reports the
    /// `MessageId` Amazon SNS minted for it.
    Published,
    /// The call was not made: the task fails with Amazon SNS's own refusal, which ends
    /// with this clause.
    Refused(&'static str),
}

/// One row: the case, the statements in force on the execution role beside the log writes
/// every state machine makes, and what the task must do.
struct Row {
    case: &'static str,
    /// Written as a closure over the topic ARNs because each row's documents are about
    /// the topics this run minted.
    statements: fn(&str, &str) -> Vec<Value>,
    outcome: Outcome,
}

/// RULE: the optimized `sns:publish` integration is executed as the state machine's
/// execution role — published when that role's documents allow `SNS:Publish` on the topic
/// the request names, and refused with Amazon SNS's own `AuthorizationError` when they do
/// not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_optimized_sns_publish_is_made_as_the_execution_role() {
    let rows = [
        Row {
            case: "a grant of the published action on the topic's own ARN",
            statements: |topic, _other| vec![allow("sns:Publish", topic)],
            outcome: Outcome::Published,
        },
        Row {
            // The same decision reached through a wildcard resource, which is how a
            // least-privilege document is usually first written.
            case: "a grant of the published action over every resource",
            statements: |_topic, _other| vec![allow("sns:Publish", "*")],
            outcome: Outcome::Published,
        },
        Row {
            // IAM compares an action's name case-insensitively, so a document written in
            // the service's own spelling grants the very same call.
            case: "a grant spelled in the service's own action case",
            statements: |topic, _other| vec![allow(PUBLISH_ACTION, topic)],
            outcome: Outcome::Published,
        },
        Row {
            case: "no statement about any call of the service",
            statements: |_topic, _other| Vec::new(),
            outcome: Outcome::Refused("because no identity-based policy allows the SNS:Publish action"),
        },
        Row {
            // Another action of the SAME service: the access request is over the action
            // the operation publishes, so a grant of a different one withholds it.
            case: "a grant of another action of the same service",
            statements: |topic, _other| vec![allow("sns:Subscribe", topic)],
            outcome: Outcome::Refused("because no identity-based policy allows the SNS:Publish action"),
        },
        Row {
            // The right action on the wrong resource: the request names the topic it
            // publishes to, so a grant that does not reach it withholds the call.
            case: "a grant of the published action on another topic",
            statements: |_topic, other| vec![allow("sns:Publish", other)],
            outcome: Outcome::Refused("because no identity-based policy allows the SNS:Publish action"),
        },
        Row {
            // The other way documents withhold a call: an explicit `Deny` wins over every
            // grant, and the refusal says so rather than naming a missing allow.
            case: "an explicit deny beside a grant of the same call",
            statements: |topic, _other| {
                vec![
                    allow("sns:Publish", topic),
                    json!({ "Effect": "Deny", "Action": "sns:Publish", "Resource": topic }),
                ]
            },
            outcome: Outcome::Refused("with an explicit deny in an identity-based policy"),
        },
    ];

    let router = emulator();
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let (queue_url, topic_arn) =
            topic_with_subscriber(&router, &format!("sfn-sns-publish-{index}")).await;
        let (other_url, other_topic) =
            topic_with_subscriber(&router, &format!("sfn-sns-elsewhere-{index}")).await;
        let mut statements = vec![logs_only()];
        statements.extend((row.statements)(&topic_arn, &other_topic));
        let role_arn = role(
            &router,
            &format!("SnsPublishRole{index}"),
            &json!({ "Version": "2012-10-17", "Statement": statements }),
        )
        .await;

        let machine_name = format!("sfn-sns-publish-{index}");
        let created = sfn(
            &router,
            "CreateStateMachine",
            &json!({
                "name": machine_name,
                "definition": definition(&topic_arn),
                "roleArn": role_arn,
            }),
        )
        .await;
        let machine_arn = string(&created, "stateMachineArn");

        let message = format!("published by {case}");
        let (execution_arn, described) = ran(&router, &machine_arn, &message, case).await;
        // Both arms end SUCCEEDED — the refusal arm through the state's own `Catch` — so
        // the status is never what tells an authorized publish from a denied one.
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("SUCCEEDED"),
            "case {case:?}: expected the execution to reach its end state, got {described:?}"
        );
        let events = history(&router, &execution_arn).await;
        let succeeded = of_type(&events, TASK_SUCCEEDED);
        let failed = of_type(&events, TASK_FAILED);
        let bodies = delivered(&router, &queue_url).await;

        match &row.outcome {
            Outcome::Published => {
                assert_eq!(
                    (succeeded.len(), failed.len()),
                    (1, 0),
                    "case {case:?}: expected exactly one {TASK_SUCCEEDED} and no \
                     {TASK_FAILED}, got {events:?}"
                );
                // What the task reports is Amazon SNS's own answer to `Publish`.
                let output = succeeded[0]["taskSucceededEventDetails"]["output"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let result: Value = serde_json::from_str(&output).unwrap_or_else(|error| {
                    panic!("case {case:?}: expected the publish result, got {output:?} ({error})")
                });
                let message_id = result["MessageId"].as_str().unwrap_or_default().to_string();
                assert!(
                    !message_id.is_empty(),
                    "case {case:?}: expected the MessageId Amazon SNS minted, got {result:?}"
                );
                // And the far end: the topic's subscriber really received the message.
                assert_eq!(
                    bodies.len(),
                    1,
                    "case {case:?}: expected one notification at the subscriber, got {bodies:?}"
                );
                let envelope: Value = serde_json::from_str(&bodies[0]).unwrap_or_else(|error| {
                    panic!(
                        "case {case:?}: the subscriber receives a notification document, got \
                         {:?} ({error})",
                        bodies[0]
                    )
                });
                assert_eq!(
                    envelope["Message"].as_str(),
                    Some(message.as_str()),
                    "case {case:?}: expected the published message, got {envelope:?}"
                );
                assert_eq!(
                    envelope["MessageId"].as_str(),
                    Some(message_id.as_str()),
                    "case {case:?}: expected the notification of the very message the task \
                     published, got {envelope:?}"
                );
            }
            Outcome::Refused(because) => {
                assert_eq!(
                    (succeeded.len(), failed.len()),
                    (0, 1),
                    "case {case:?}: expected exactly one {TASK_FAILED} and no \
                     {TASK_SUCCEEDED}, got {events:?}"
                );
                let details = &failed[0]["taskFailedEventDetails"];
                assert_eq!(
                    details.get("error").and_then(Value::as_str),
                    Some(AUTHORIZATION_ERROR),
                    "case {case:?}: expected the error Amazon SNS models for an \
                     unauthorized call, got {details:?}"
                );
                // The whole refusal, as the invoked service words it: the session of the
                // role the call was made as, the action that service publishes, the topic
                // the request named, why its documents withheld it, and the metadata its
                // own client appends.
                let principal = session_principal(&role_arn, &machine_name);
                let expected = format!(
                    "User: {principal} is not authorized to perform: {PUBLISH_ACTION} on \
                     resource: {topic_arn} {because}{REFUSAL_METADATA}"
                );
                let cause = string(details, "cause");
                assert_eq!(
                    normalized(case, &cause),
                    expected,
                    "case {case:?}: expected the refusal {expected:?}, got {cause:?}"
                );
                // Nothing was published: the call the documents withheld was not made.
                assert!(
                    bodies.is_empty(),
                    "case {case:?}: expected the subscriber to receive nothing, got {bodies:?}"
                );
            }
        }
        // The other topic of this row is never published to, whichever arm ran: it is
        // only there to be named by a grant that does not reach the topic under test.
        assert!(
            delivered(&router, &other_url).await.is_empty(),
            "case {case:?}: expected nothing published to {other_topic}"
        );
    }
}

/// The topic a `Task`'s `TopicArn` parameter names is addressed in the account and Region
/// the call is made in, which is what makes the ARN the refusal above is refused ON the
/// same string the request carried: Amazon SNS composes a created topic's ARN from the
/// account that owns it and the Region it lives in, not from anything the caller supplies.
#[tokio::test]
async fn a_created_topic_is_addressed_in_the_emulated_account_and_region() {
    let router = emulator();
    let (_url, topic_arn) = topic_with_subscriber(&router, "sfn-sns-account").await;
    assert_eq!(
        topic_arn,
        format!("arn:aws:sns:{REGION}:{ACCOUNT}:sfn-sns-account"),
        "expected the topic ARN this account and Region address, got {topic_arn:?}"
    );
}
