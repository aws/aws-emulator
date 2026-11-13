// SPDX-License-Identifier: CC0-1.0

//! An invocation that did NOT reach its target is REPORTED: end to end over the wire
//! through the real `MultiServiceRouter`, with Amazon SQS and IAM mounted beside
//! EventBridge Scheduler on ONE engine — so a failed invocation is asserted where a
//! client observes it, in the messages the schedule's dead-letter queue holds.
//!
//! RULE: an occurrence whose invocation does not reach its target — the target's own
//! API refuses the call, or the permissions of `Target.RoleArn`, which the invocation
//! is made under, withhold it — is delivered to the queue `Target.DeadLetterConfig.Arn`
//! names, carrying the payload the target was to be handed and, beside it, the detail
//! of the failure that ended the invocation. An invocation the target accepted is not
//! reported there, and neither is one whose execution role may not write to that
//! queue.
//!
//! RULE: what the dead-letter queue is handed is the failed EVENT — the REQUEST the
//! invocation attempted against its target's own API, since a templated target names
//! one API operation of the target's service and the schedule's `Target` members are
//! that operation's request — so the undelivered payload is carried under the member
//! that request takes it in, and is the whole body only where that request is the
//! payload itself.
//!
//! RULE: the failure's reported `ERROR_CODE` is the code the service that refused the
//! call answers it with, so a call the execution role's permissions withheld is
//! reported under that service's OWN spelling of AWS's authorization refusal, and its
//! `ERROR_MESSAGE` is AWS's authorization sentence — the principal the call was made
//! by, the action and the resource — with the action named the way that same service
//! names an action in a refusal of its own.
//!
//! The rows span the class rather than one recorded case: they differ along the KIND
//! of failure (the target does not exist, the target refuses the request, the
//! permissions withhold the call), along whether the invocation reaches its target at
//! all, and along whether the execution role may write to the dead-letter queue. What
//! the retry policy admits before a failure becomes the invocation's last, over the
//! whole range of both its members, is asserted without a wait in
//! `src/state/dead_letter.rs`'s own tests.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_scheduler_awschronosservice::state::invocation;
use smithy_server_runtime::MultiServiceRouter;

use crate::schedule_invocation::{
    at_expression_body, emulator, given_execution_role_for, given_queue, peek, scheduler,
    seconds_from_now, sqs, sqs_response, POLL_INTERVAL, REGION,
};
use crate::wire_harness::account_id;

/// How long a row waits for the message it expects. The occurrence every row states
/// is seconds ahead; the rest is room for the clock's tick, the refusal and the
/// dead-letter delivery.
const PATIENCE: Duration = Duration::from_secs(80);

/// How long a row watches a queue it expects NOTHING to reach: well past the
/// occurrence its own expression names, so an invocation this emulator would have
/// performed, and a report it would have delivered, have both had their chance.
const QUIET_WINDOW: Duration = Duration::from_secs(15);

/// The error code Amazon SQS answers a call its caller is not authorized to make with.
/// A query-compatible service answers such a call with the legacy Query code AWS
/// refuses an unauthorized call under, which is what a report of a withheld invocation
/// of a QUEUE target carries.
const QUEUE_ACCESS_DENIED: &str = "AccessDenied";

/// What the row's schedule points its target at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetQueue {
    /// A standard queue of this account, which takes the invocation.
    Standard,
    /// A queue of this account that was never created, which its own API refuses a
    /// message to.
    Absent,
    /// A FIFO queue, which refuses a message carrying no message group — the row's
    /// target states none.
    Fifo,
}

/// What the row expects to observe once the occurrence has passed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// The target was invoked, and nothing was reported to the dead-letter queue.
    Delivered,
    /// The failure was reported to the dead-letter queue. `Some(code)` where the code
    /// is the service's own for that refusal; `None` where the row discovers it from
    /// the target's own API, which is what a report of it must carry.
    DeadLettered(Option<&'static str>),
    /// Neither queue was reached at all.
    Nothing,
}

/// One row: the schedule written, what its execution role may write to, and what is
/// observed.
struct Row {
    case: &'static str,
    target: TargetQueue,
    /// Whether the role's permissions allow the invocation's own call.
    may_invoke: bool,
    /// Whether they allow the dead-letter message's delivery.
    may_dead_letter: bool,
    /// How many retries the schedule's `RetryPolicy` admits, or `None` for a schedule
    /// stating no policy at all.
    retries: Option<i32>,
    expected: Expected,
}

const ROWS: &[Row] = &[
    Row {
        case: "a target queue that does not exist",
        target: TargetQueue::Absent,
        may_invoke: true,
        may_dead_letter: true,
        // A refusal of the request itself is the invocation's last attempt whatever
        // the policy would still have admitted, so no retry is made of it.
        retries: Some(2),
        expected: Expected::DeadLettered(None),
    },
    Row {
        case: "a FIFO target the schedule names no message group for",
        target: TargetQueue::Fifo,
        may_invoke: true,
        may_dead_letter: true,
        retries: None,
        expected: Expected::DeadLettered(None),
    },
    Row {
        case: "an execution role that may not write to the target queue",
        target: TargetQueue::Standard,
        may_invoke: false,
        may_dead_letter: true,
        retries: None,
        expected: Expected::DeadLettered(Some(QUEUE_ACCESS_DENIED)),
    },
    Row {
        case: "an invocation the target accepts",
        target: TargetQueue::Standard,
        may_invoke: true,
        may_dead_letter: true,
        retries: Some(0),
        expected: Expected::Delivered,
    },
    Row {
        case: "an execution role that may write to neither queue",
        target: TargetQueue::Standard,
        may_invoke: false,
        may_dead_letter: false,
        retries: None,
        expected: Expected::Nothing,
    },
];

/// Every row: a failed invocation is reported to the schedule's dead-letter queue
/// with the failure's own detail, and nothing else is.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_invocation_is_reported_to_the_dead_letter_queue() {
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let emulator = emulator();
        let (dead_letter_url, dead_letter_arn) =
            given_queue(&emulator, case, &format!("dlq-{index}")).await;
        let (target_url, target_arn) = given_target(&emulator, case, index, row.target).await;

        let mut granted: Vec<&str> = Vec::new();
        if row.may_invoke {
            granted.push(&target_arn);
        }
        if row.may_dead_letter {
            granted.push(&dead_letter_arn);
        }
        let role =
            given_execution_role_for(&emulator, case, &format!("role-{index}"), &granted).await;

        let marker = format!("{{\"marker\":\"{index}\"}}");
        let occurrence = seconds_from_now(4);
        let mut target = json!({
            "Arn": target_arn,
            "RoleArn": role,
            "Input": marker,
            "DeadLetterConfig": { "Arn": dead_letter_arn },
        });
        if let Some(retries) = row.retries {
            target["RetryPolicy"] = json!({ "MaximumRetryAttempts": retries });
        }
        let name = format!("schedule-{index}");
        let (status, created) = scheduler(
            &emulator,
            "POST",
            &format!("/schedules/{name}"),
            Some(json!({
                "ScheduleExpression": format!("at({})", at_expression_body(occurrence)),
                "FlexibleTimeWindow": { "Mode": "OFF" },
                "Target": target,
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateSchedule answered {status} {created}"
        );
        let schedule_arn = created["ScheduleArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no ScheduleArn in {created}"))
            .to_string();

        match row.expected {
            Expected::Delivered => {
                let delivered = messages_once(&emulator, &target_url, 1, PATIENCE).await;
                let body = delivered
                    .first()
                    .map(|message| message["Body"].as_str().unwrap_or_default().to_string())
                    .unwrap_or_else(|| panic!("case {case:?}: the target was never invoked"));
                assert_eq!(
                    body, marker,
                    "case {case:?}: expected the schedule's own Input {marker:?}, got {body:?}"
                );
                stays_empty(&emulator, case, &dead_letter_url, "the dead-letter queue").await;
            }
            Expected::Nothing => {
                stays_empty(&emulator, case, &dead_letter_url, "the dead-letter queue").await;
                stays_empty(&emulator, case, &target_url, "the target queue").await;
            }
            Expected::DeadLettered(code) => {
                // The code the report must carry is the one the target's own API
                // answers that call with, so a row that does not state one asks the
                // target itself rather than restating its vocabulary here.
                let expected_code = match code {
                    Some(code) => code.to_string(),
                    None => refusal_code(&emulator, case, &target_url).await,
                };
                let reported = messages_once(&emulator, &dead_letter_url, 1, PATIENCE).await;
                let message = reported
                    .first()
                    .unwrap_or_else(|| panic!("case {case:?}: nothing was dead-lettered"));
                // What is reported is the failed EVENT: the `SendMessage` request the
                // invocation attempted, which addresses the target queue and carries
                // the undelivered payload as its `MessageBody`. The table below spans
                // the other target kinds.
                let expected_body = json!({ "QueueUrl": target_url, "MessageBody": marker });
                assert_eq!(
                    body_document(message),
                    expected_body,
                    "case {case:?}: expected the failed event {expected_body}, got {message}"
                );
                let attribute = |name: &str| {
                    message["MessageAttributes"][name]["StringValue"]
                        .as_str()
                        .map(ToString::to_string)
                };
                for (name, expected) in [
                    ("ERROR_CODE", expected_code.clone()),
                    ("SCHEDULE_ARN", schedule_arn.clone()),
                    ("TARGET_ARN", target_arn.clone()),
                    (
                        "SCHEDULED_TIME",
                        invocation::scheduled_time_text(occurrence),
                    ),
                    // A refusal of the request is never retried, so the report of one
                    // counts no retry even where the policy admitted two.
                    ("RETRY_ATTEMPTS", "0".to_string()),
                ] {
                    assert_eq!(
                        attribute(name),
                        Some(expected.clone()),
                        "case {case:?}: expected {name} to carry {expected:?}, got {message}"
                    );
                }
                let detail = attribute("ERROR_MESSAGE").unwrap_or_default();
                assert!(
                    !detail.is_empty(),
                    "case {case:?}: expected the failure's message to be reported, got {message}"
                );
                if code == Some(QUEUE_ACCESS_DENIED) {
                    // AWS's own authorization refusal: the call that was withheld, named
                    // the way Amazon SQS names an action in a refusal of its own (case-
                    // folded throughout), and the resource it was on.
                    for expected in ["is not authorized to perform: sqs:sendmessage", &target_arn] {
                        assert!(
                            detail.contains(expected),
                            "case {case:?}: expected the refusal to name {expected:?}, got \
                             {detail:?}"
                        );
                    }
                }
                let execution = attribute("EXECUTION_ID").unwrap_or_default();
                // "8-4-4-4-12": the form EventBridge Scheduler names an invocation in.
                let groups: Vec<usize> = execution.split('-').map(str::len).collect();
                assert_eq!(
                    groups,
                    vec![8, 4, 4, 4, 12],
                    "case {case:?}: expected an execution id in the published form, got \
                     {execution:?}"
                );
                assert_eq!(
                    message["MessageAttributes"]["RETRY_ATTEMPTS"]["DataType"].as_str(),
                    Some("Number"),
                    "case {case:?}: expected the retry count reported as a figure, got {message}"
                );
                assert_eq!(
                    attribute("EXHAUSTED_RETRY_CONDITION"),
                    None,
                    "case {case:?}: expected no exhausted-retry condition for a refusal no \
                     retry was made of, got {message}"
                );
                // Nothing reached the target: the report is of an invocation that did
                // not arrive, never of one that did.
                if row.target == TargetQueue::Standard {
                    let held = peek(&emulator, &target_url).await;
                    assert!(
                        held.is_empty(),
                        "case {case:?}: expected the target to hold nothing, got {held:?}"
                    );
                }
            }
        }
    }
}

/// The queue the row's target names, and the url Amazon SQS addresses it by.
///
/// An ABSENT queue is named by the ARN a queue of that name would carry and never
/// created, so the url is the one the invocation itself resolves that ARN to.
async fn given_target(
    router: &MultiServiceRouter,
    case: &str,
    index: usize,
    target: TargetQueue,
) -> (String, String) {
    match target {
        TargetQueue::Standard => given_queue(router, case, &format!("target-{index}")).await,
        TargetQueue::Absent => {
            let name = format!("absent-{index}");
            (
                format!(
                    "https://sqs.{REGION}.amazonaws.com/{}/{name}",
                    account_id()
                ),
                format!("arn:aws:sqs:{REGION}:{}:{name}", account_id()),
            )
        }
        TargetQueue::Fifo => {
            let name = format!("target-{index}.fifo");
            let (status, created) = sqs(
                router,
                "CreateQueue",
                &json!({ "QueueName": name, "Attributes": { "FifoQueue": "true" } }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: CreateQueue answered {status} {created}"
            );
            let url = created["QueueUrl"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no QueueUrl in {created}"))
                .to_string();
            (url, format!("arn:aws:sqs:{REGION}:{}:{name}", account_id()))
        }
    }
}

/// The error code a client sees when Amazon SQS refuses a plain `SendMessage` to
/// `url`.
///
/// The probe is refused for the same reason the invocation is, so this is the code
/// the service reports for that failure rather than one this suite states. It is read
/// the way a client reads it: Amazon SQS's protocol is query-compatible, so a refusal
/// carries the Query code in a header of its own beside the modelled name in the
/// body, and the Query code is the one an SDK surfaces.
async fn refusal_code(router: &MultiServiceRouter, case: &str, url: &str) -> String {
    let answered = sqs_response(
        router,
        "SendMessage",
        &json!({ "QueueUrl": url, "MessageBody": "a probe" }),
        true,
    )
    .await;
    let status = answered.status().as_u16();
    let query_code = answered
        .headers()
        .get("x-amzn-query-error")
        .and_then(|value| value.to_str().ok())
        // `<code>;<fault>`: the code is the first field.
        .and_then(|value| value.split(';').next())
        .map(ToString::to_string);
    let body = String::from_utf8_lossy(answered.body()).to_string();
    assert!(
        status >= 400,
        "case {case:?}: expected SendMessage to {url} to be refused, got {status} {body}"
    );
    if let Some(code) = query_code.filter(|code| !code.is_empty()) {
        return code;
    }
    let parsed: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let kind = parsed["__type"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no error code in {body}"))
        .to_string();
    // An awsJson error type may be qualified by its namespace; the code is the name.
    kind.rsplit('#').next().unwrap_or(&kind).to_string()
}

/// The messages the queue at `url` holds once it holds at least `wanted`, or whatever
/// it holds after `patience` — left on the queue, with every message attribute they
/// carry.
async fn messages_once(
    router: &MultiServiceRouter,
    url: &str,
    wanted: usize,
    patience: Duration,
) -> Vec<Value> {
    let deadline = Instant::now() + patience;
    loop {
        let held = received(router, url).await;
        if held.len() >= wanted || Instant::now() >= deadline {
            return held;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Assert the queue at `url` stays empty for as long as a row watches it, looking
/// again until the window closes.
async fn stays_empty(router: &MultiServiceRouter, case: &str, url: &str, what: &str) {
    let deadline = Instant::now() + QUIET_WINDOW;
    while Instant::now() < deadline {
        let held = received(router, url).await;
        assert!(
            held.is_empty(),
            "case {case:?}: expected {what} to hold nothing, got {held:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Every message waiting on the queue at `url`, with its message attributes, left on
/// the queue: a report is read for its attributes, and a row that looks again must
/// still see it.
async fn received(router: &MultiServiceRouter, url: &str) -> Vec<Value> {
    let (status, answered) = sqs(
        router,
        "ReceiveMessage",
        &json!({
            "QueueUrl": url,
            "MaxNumberOfMessages": 10,
            "VisibilityTimeout": 0,
            "MessageAttributeNames": ["All"],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "receive {url}: ReceiveMessage answered {status} {answered}"
    );
    answered["Messages"].as_array().cloned().unwrap_or_default()
}

/// The document a dead-letter message's body IS: a failed event is the request the
/// invocation attempted, so the body is read as that document rather than as text.
fn body_document(message: &Value) -> Value {
    let body = message["Body"].as_str().unwrap_or_default();
    serde_json::from_str(body).unwrap_or_else(|_| Value::String(body.to_string()))
}

/// The `QueueUrl` Amazon SQS addresses the queue an ARN names by, which is how the
/// `SendMessage` request a queue target's invocation makes names it:
/// `<endpoint>/<accountId>/<QueueName>`. Pinned against the url the service itself
/// reports for a queue in the table below.
fn queue_url_of(queue_arn: &str) -> String {
    let name = queue_arn.rsplit(':').next().unwrap_or_default();
    format!("https://sqs.{REGION}.amazonaws.com/{}/{name}", account_id())
}

/// `MessageGroupId`, as Amazon SQS's own `SendMessage` request names it — the member a
/// queue target's `Target.SqsParameters` states.
const MESSAGE_GROUP_ID_MEMBER: &str = "MessageGroupId";

/// One row of the failed-event table: what the schedule's target names, what it states
/// for it, and the members of the request that target's API takes.
struct EventRow {
    case: &'static str,
    /// `Target.Arn`, with `{region}` and `{account}` where the row's own emulator
    /// fills them in.
    target: &'static str,
    /// The member of the target API's request that ADDRESSES the target, or `None` for
    /// a universal target, whose `Input` already IS the request.
    addressed_under: Option<&'static str>,
    /// Whether that member addresses the target by the url Amazon SQS reports for it
    /// rather than by its ARN.
    by_queue_url: bool,
    /// The member of that request the undelivered payload is carried under.
    carried_under: Option<&'static str>,
    /// The FIFO message group `Target.SqsParameters` states, if any.
    message_group_id: Option<&'static str>,
    /// The error code the target's own service answers a call its caller is not
    /// authorized to make with, which is what the report of this row's withheld
    /// invocation carries.
    refused_with: &'static str,
    /// The action as the refusal NAMES it: the IAM action the call is authorized by,
    /// worded the way the service that answered the refusal words one.
    reported_action: &'static str,
}

const EVENT_ROWS: &[EventRow] = &[
    EventRow {
        // "The templated target type for the Amazon SQS SendMessage API operation"
        // (`Target$SqsParameters`), whose request takes `QueueUrl` and `MessageBody`.
        case: "a queue target",
        target: "arn:aws:sqs:{region}:{account}:failed-event-target",
        addressed_under: Some("QueueUrl"),
        by_queue_url: true,
        carried_under: Some("MessageBody"),
        message_group_id: None,
        // Amazon SQS is query-compatible, so its answer carries the legacy Query code,
        // and that service names the action case-folded throughout.
        refused_with: QUEUE_ACCESS_DENIED,
        reported_action: "sqs:sendmessage",
    },
    EventRow {
        // The same operation, with the one parameter that target kind states: the
        // group travels with the invocation, so it is part of the request that failed.
        case: "a FIFO queue target the schedule states a message group for",
        target: "arn:aws:sqs:{region}:{account}:failed-event-target.fifo",
        addressed_under: Some("QueueUrl"),
        by_queue_url: true,
        carried_under: Some("MessageBody"),
        message_group_id: Some("orders"),
        refused_with: QUEUE_ACCESS_DENIED,
        reported_action: "sqs:sendmessage",
    },
    EventRow {
        // Amazon SNS's `Publish`, which takes `TopicArn` and `Message`.
        case: "a topic target",
        target: "arn:aws:sns:{region}:{account}:failed-event-topic",
        addressed_under: Some("TopicArn"),
        by_queue_url: false,
        carried_under: Some("Message"),
        message_group_id: None,
        // Amazon SNS MODELS its authorization refusal
        // (`AuthorizationErrorException`, Query code `AuthorizationError`).
        refused_with: "AuthorizationError",
        // An SNS action is `SNS:<OperationName>`, which is how that service names the
        // action a refusal of its own withheld.
        reported_action: "SNS:Publish",
    },
    EventRow {
        // Step Functions' `StartExecution`, which takes `StateMachineArn` and `Input`.
        case: "a state machine target",
        target: "arn:aws:states:{region}:{account}:stateMachine:failed-event-machine",
        addressed_under: Some("StateMachineArn"),
        by_queue_url: false,
        carried_under: Some("Input"),
        message_group_id: None,
        refused_with: "AccessDeniedException",
        // AWS's authorization layer answers for this service, naming the action as the
        // service documents it.
        reported_action: "states:StartExecution",
    },
    EventRow {
        // AWS Lambda's `Invoke`, which takes `FunctionName` and `Payload`.
        case: "a function target",
        target: "arn:aws:lambda:{region}:{account}:function:failed-event-function",
        addressed_under: Some("FunctionName"),
        by_queue_url: false,
        carried_under: Some("Payload"),
        message_group_id: None,
        refused_with: "AccessDeniedException",
        reported_action: "lambda:InvokeFunction",
    },
    EventRow {
        // A universal target's `Input` already IS the request the invocation makes, so
        // there is nothing to build around it. The call it names is a queue's, so the
        // refusal is the one Amazon SQS answers.
        case: "a universal target",
        target: "arn:aws:scheduler:::aws-sdk:sqs:sendMessage",
        addressed_under: None,
        by_queue_url: false,
        carried_under: None,
        message_group_id: None,
        refused_with: QUEUE_ACCESS_DENIED,
        reported_action: "sqs:sendmessage",
    },
];

/// Every row: what the dead-letter queue is handed is the REQUEST the invocation
/// attempted against its target's own API — the target it addresses, the undelivered
/// payload under the member that request carries it in, and the parameters the target
/// states — and a universal target's own input as it stands.
///
/// Every row's execution role may write to the dead-letter queue alone, so every row's
/// invocation is withheld and every row's failed event is reported.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dead_lettered_event_is_the_request_the_invocation_attempted() {
    for (index, row) in EVENT_ROWS.iter().enumerate() {
        let case = row.case;
        let emulator = emulator();
        let (dead_letter_url, dead_letter_arn) =
            given_queue(&emulator, case, &format!("event-dlq-{index}")).await;
        // The row's own queue: what a universal target's request addresses, and a
        // queue the execution role may not write to either.
        let (queue_url, queue_arn) =
            given_queue(&emulator, case, &format!("event-queue-{index}")).await;
        // The url the request carries for a queue is the one Amazon SQS itself reports
        // for it, so the form this suite derives below is pinned against that service.
        assert_eq!(
            queue_url_of(&queue_arn),
            queue_url,
            "case {case:?}: expected the queue url Amazon SQS reports for {queue_arn}"
        );
        let role = given_execution_role_for(
            &emulator,
            case,
            &format!("event-role-{index}"),
            &[&dead_letter_arn],
        )
        .await;

        let target_arn = row
            .target
            .replace("{region}", REGION)
            .replace("{account}", account_id());
        let marker = format!("{{\"marker\":\"{index}\"}}");
        let payload = match row.addressed_under {
            // A universal target's input is the call's own request, which this row
            // makes of the queue it created.
            None => json!({ "QueueUrl": queue_url, "MessageBody": marker }),
            Some(_) => json!({ "marker": format!("{index}") }),
        };
        let input = payload.to_string();
        let mut target = json!({
            "Arn": target_arn,
            "RoleArn": role,
            "Input": input,
            "RetryPolicy": { "MaximumRetryAttempts": 0 },
            "DeadLetterConfig": { "Arn": dead_letter_arn },
        });
        if let Some(group) = row.message_group_id {
            target["SqsParameters"] = json!({ "MessageGroupId": group });
        }
        let name = format!("failed-event-{index}");
        let (status, created) = scheduler(
            &emulator,
            "POST",
            &format!("/schedules/{name}"),
            Some(json!({
                "ScheduleExpression": format!("at({})", at_expression_body(seconds_from_now(4))),
                "FlexibleTimeWindow": { "Mode": "OFF" },
                "State": "ENABLED",
                "Target": target,
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateSchedule answered {status} {created}"
        );

        let expected = match (row.addressed_under, row.carried_under) {
            (Some(address), Some(member)) => {
                let mut document = serde_json::Map::new();
                let addressed = match row.by_queue_url {
                    true => queue_url_of(&target_arn),
                    false => target_arn.clone(),
                };
                document.insert(address.to_string(), Value::String(addressed));
                document.insert(member.to_string(), Value::String(input.clone()));
                if let Some(group) = row.message_group_id {
                    document.insert(
                        MESSAGE_GROUP_ID_MEMBER.to_string(),
                        Value::String(group.to_string()),
                    );
                }
                Value::Object(document)
            }
            _ => payload.clone(),
        };
        let reported = messages_once(&emulator, &dead_letter_url, 1, PATIENCE).await;
        let message = reported
            .first()
            .unwrap_or_else(|| panic!("case {case:?}: nothing was dead-lettered"));
        assert_eq!(
            body_document(message),
            expected,
            "case {case:?}: expected the dead-lettered event {expected}, got {message}"
        );
        // The report is of a call the invocation's own permissions withheld, under the
        // code the target's own service answers such a call with.
        assert_eq!(
            message["MessageAttributes"]["ERROR_CODE"]["StringValue"].as_str(),
            Some(row.refused_with),
            "case {case:?}: expected {:?} to be reported, got {message}",
            row.refused_with
        );
        // And worded the way the service that refused the call words such a refusal: the
        // principal the call was made by — the session this invocation assumed the
        // execution role under, named by a token of the invocation's own execution id —
        // the action as that service names it, and the resource.
        let execution = message["MessageAttributes"]["EXECUTION_ID"]["StringValue"]
            .as_str()
            .unwrap_or_default()
            .replace('-', "");
        let action = row.reported_action;
        let refused_on = match row.addressed_under {
            Some(_) => target_arn.clone(),
            // The universal row's call is made against the queue it created.
            None => queue_arn.clone(),
        };
        let expected_message = format!(
            "User: arn:aws:sts::{account}:assumed-role/event-role-{index}/{execution} is not \
             authorized to perform: {action} on resource: {refused_on} because no \
             identity-based policy allows the {action} action",
            account = account_id(),
        );
        assert_eq!(
            message["MessageAttributes"]["ERROR_MESSAGE"]["StringValue"].as_str(),
            Some(expected_message.as_str()),
            "case {case:?}: expected the refusal {expected_message:?}, got {message}"
        );
    }
}
