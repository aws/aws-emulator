// SPDX-License-Identifier: CC0-1.0

//! The CALLBACK round trip of a `Task` state whose work is an EVENT PUBLICATION, driven over
//! four services' own wire protocols through the production `MultiServiceRouter` with Step
//! Functions, EventBridge, Amazon SQS and Lambda mounted on ONE engine — the shape
//! `core/router/src/lib.rs` mounts them in, reached through
//! [`crate::execution_status_change_events::Emulator`] so there is one setup of these
//! services in this target.
//!
//! RULE: a `.waitForTaskToken` `Task` mints its task token as the state is entered and
//! substitutes it wherever the state's own `Parameters` quote the context object's
//! `$$.Task.Token`, makes its integration call ONCE with that rendered request, and then
//! WAITS — the call's own answer never finishes the state. What finishes it is the
//! `SendTaskSuccess` or `SendTaskFailure` quoting that token, whoever makes it and however
//! far the token travelled first: a success is the state's result, exactly the string the
//! answer carried, and a failure is reported with exactly the `error` and `cause` it named.
//! For the event-publication integration the token therefore travels in the published
//! event's own `detail`, and the work that reads it out of a delivered envelope answers the
//! task from there — so the four-hop trip (publish, match, invoke, answer) ends with the
//! execution terminal on the worker's own verdict, while a sibling rule the same publication
//! does not match receives nothing from it.
//!
//! The rows differ along what the rule generalises over: how the rendered entry addresses the
//! bus it publishes onto (the bus's bare name, and its ARN), WHERE in the rendered document
//! the context object's token is placed (a top-level member of the `Detail`, and a member of
//! an object nested inside it), and which `SendTask*` the work answers the token with (a
//! success, and a failure).
//!
//! Every expected value is computed here from what the row published — the token and the
//! event id out of the envelope EventBridge delivered, the job out of the execution's own
//! input — never read back out of the answer being checked.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::execution_status_change_events::{given_queue, read_once, Emulator};
use crate::lambda_task_control_flow::package;
use crate::state_machine_lifecycle::{string, ROLE_ARN};

/// The `source` every row's Task publishes under, and the two `detail-type`s the row's two
/// rules match on: one the publication carries and one it does not.
const SOURCE: &str = "sim.sfn";
const MATCHING_DETAIL_TYPE: &str = "task.requested";
const MISMATCHED_DETAIL_TYPE: &str = "task.requested.x";

/// The principal EventBridge invokes a resource-policy-governed target as, and the action an
/// invocation of a function performs: "for Lambda functions, Amazon SNS topics and Amazon SQS
/// queues, EventBridge relies on resource-based policies" (`PutTargets`), so the worker admits
/// this service nothing until the grant is added.
const EVENTS_PRINCIPAL: &str = "events.amazonaws.com";
const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// Where `CreateFunction` is bound (`POST /2015-03-31/functions`).
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The `State` a function reports once it can be invoked (`FunctionConfiguration$State`:
/// "Active" once "the function is ready to be invoked").
const FUNCTION_ACTIVE: &str = "Active";

/// How long one invocation of the worker is given, as the function's own `Timeout`: well past
/// what a run needs on any host, because what is under test is the answer the worker sends and
/// not how fast an interpreter gets through it.
const WORKER_TIMEOUT_SECONDS: u64 = 60;

/// The terminal statuses a row's execution reaches, and the one it sits in while its callback
/// task is still outstanding (`ExecutionStatus`).
const SUCCEEDED: &str = "SUCCEEDED";
const FAILED: &str = "FAILED";
const RUNNING: &str = "RUNNING";

/// The `error` and `cause` the worker's `SendTaskFailure` names, which the failing row's
/// execution must carry verbatim.
const REPORTED_ERROR: &str = "WorkerRejectedTheJob";
/// See [`REPORTED_ERROR`].
const REPORTED_CAUSE: &str = "the worker could not take the job on";

/// The definition-level allowance each row's callback task is given, which no row may need:
/// the round trip ends on the worker's answer, long before it.
const TASK_TIMEOUT_SECONDS: u64 = 240;

/// How long one poll waits before looking again, and how long a row waits for work the clock
/// and four services owe: generous, so a loaded host cannot decide the outcome.
const POLL: Duration = Duration::from_millis(50);
/// See [`POLL`].
const PATIENCE: Duration = Duration::from_secs(180);

/// How long a row watches the queue of the rule the publication does NOT match.
///
/// Only entered once the row's execution has already reached its terminal status, so the
/// publication and every delivery it led to have been made: an empty queue is a rule that
/// matched nothing rather than one still in flight. The window is a second look at that, not
/// the thing that makes it true.
const SILENCE_WINDOW: Duration = Duration::from_secs(2);

/// The worker every row's matching rule invokes: it reads the task token out of the envelope
/// it was DELIVERED — at whichever of the two places the row's state machine put it — and
/// answers the paused task through the AWS SDK its runtime supplies, exactly as the callback
/// pattern is written.
///
/// The success payload is built by text formatting rather than by a JSON encoder, so the
/// string the task is answered with is byte-for-byte the one this suite predicts from the
/// delivered envelope.
const WORKER_SOURCE: &str = r#"
import boto3


def handler(event, context):
    detail = event["detail"]
    if "taskToken" in detail:
        token = detail["taskToken"]
    else:
        token = detail["callback"]["taskToken"]
    states = boto3.client("stepfunctions")
    if detail["mode"] == "fail":
        states.send_task_failure(
            taskToken=token, error=detail["error"], cause=detail["cause"]
        )
        return {"answered": "failure", "token": token}
    payload = '{"detailType":"%s","eventId":"%s","job":"%s","source":"%s"}' % (
        event["detail-type"],
        event["id"],
        detail["job"],
        event["source"],
    )
    states.send_task_success(taskToken=token, output=payload)
    return {"answered": "success", "token": token, "output": payload}
"#;

/// The payload the worker answers a success with, as this suite predicts it from the envelope
/// EventBridge delivered and the job the execution was started with.
fn expected_output(event_id: &str, job: &str) -> String {
    format!(
        "{{\"detailType\":\"{MATCHING_DETAIL_TYPE}\",\"eventId\":\"{event_id}\",\"job\":\
         \"{job}\",\"source\":\"{SOURCE}\"}}"
    )
}

/// Where in the rendered `Detail` the row puts the context object's `$$.Task.Token`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TokenAt {
    /// A top-level member of the entry's `Detail`.
    Detail,
    /// A member of an object nested inside that `Detail`.
    NestedObject,
}

impl TokenAt {
    /// The `Detail` the state renders, with the token placed where this names.
    fn detail(self, job_nonce: &str, answer: Answer) -> Value {
        let mut detail = json!({
            "job.$": "$.job",
            "nonce": job_nonce,
            "mode": answer.mode(),
            "error": REPORTED_ERROR,
            "cause": REPORTED_CAUSE,
        });
        match self {
            Self::Detail => detail["taskToken.$"] = json!("$$.Task.Token"),
            Self::NestedObject => detail["callback"] = json!({ "taskToken.$": "$$.Task.Token" }),
        }
        detail
    }

    /// The token out of a delivered envelope's `detail`, read from where this put it.
    fn token_in(self, case: &str, detail: &Value) -> String {
        let found = match self {
            Self::Detail => detail.get("taskToken"),
            Self::NestedObject => detail.pointer("/callback/taskToken"),
        };
        found
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected the substituted task token in {detail}")
            })
            .to_string()
    }
}

/// Which `SendTask*` the worker answers the token with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Answer {
    Success,
    Failure,
}

impl Answer {
    /// The `mode` the rendered `Detail` carries, which decides what the worker calls.
    fn mode(self) -> &'static str {
        match self {
            Self::Success => "answer",
            Self::Failure => "fail",
        }
    }

    /// The terminal status the answered execution must reach.
    fn status(self) -> &'static str {
        match self {
            Self::Success => SUCCEEDED,
            Self::Failure => FAILED,
        }
    }
}

/// One row: how the rendered entry addresses its bus, where the token sits in the document it
/// publishes, and how the worker answers it.
struct Row {
    case: &'static str,
    /// Whether the entry's `EventBusName` is the bus's ARN rather than its bare name — both
    /// are `EventBusNameOrArn`
    /// (`smithy_query(query="operation",operation="PutEvents",model_path="services/
    /// service-com-amazonaws-eventbridge-awsevents/smithy-model/
    /// com-amazonaws-eventbridge-awsevents.json")`).
    bus_by_arn: bool,
    token_at: TokenAt,
    answer: Answer,
}

const ROWS: &[Row] = &[
    Row {
        case: "the bus by name, the token a member of the Detail, answered with a success",
        bus_by_arn: false,
        token_at: TokenAt::Detail,
        answer: Answer::Success,
    },
    Row {
        case: "the same publication addressed by the bus's ARN",
        bus_by_arn: true,
        token_at: TokenAt::Detail,
        answer: Answer::Success,
    },
    Row {
        case: "the token inside an object nested in the Detail",
        bus_by_arn: true,
        token_at: TokenAt::NestedObject,
        answer: Answer::Success,
    },
    Row {
        case: "a worker that answers the token with a failure",
        bus_by_arn: false,
        token_at: TokenAt::Detail,
        answer: Answer::Failure,
    },
];

/// The worker function, waited out to [`FUNCTION_ACTIVE`] and granted the invocation
/// EventBridge makes on it; answers its ARN.
async fn given_worker(emulator: &Emulator, name: &str) -> String {
    let (status, created) = emulator
        .lambda(
            "POST",
            FUNCTIONS_URI,
            &json!({
                "FunctionName": name,
                "Role": "arn:aws:iam::000000000000:role/LambdaExecutionRole",
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Timeout": WORKER_TIMEOUT_SECONDS,
                "Code": { "ZipFile": package(WORKER_SOURCE) },
            }),
        )
        .await;
    assert_eq!(
        status, 201,
        "the worker must be created, got {status} with {created}"
    );
    let arn = string(&created, "FunctionArn");

    let (status, granted) = emulator
        .lambda(
            "POST",
            &format!("{FUNCTIONS_URI}/{name}/policy"),
            &json!({
                "StatementId": "events-invoke",
                "Action": INVOKE_ACTION,
                "Principal": EVENTS_PRINCIPAL,
            }),
        )
        .await;
    assert_eq!(
        status, 201,
        "the worker must admit this service the invocation, got {status} with {granted}"
    );

    // The created function leaves its non-terminal state by the clock, which no fixed sleep
    // names: this looks until it reports itself invocable.
    let began = Instant::now();
    loop {
        let (status, configuration) = emulator
            .lambda(
                "GET",
                &format!("{FUNCTIONS_URI}/{name}/configuration"),
                &Value::Null,
            )
            .await;
        assert_eq!(
            status, 200,
            "the created worker must be readable, got {status} with {configuration}"
        );
        if configuration.get("State").and_then(Value::as_str) == Some(FUNCTION_ACTIVE) {
            return arn;
        }
        assert!(
            began.elapsed() < PATIENCE,
            "the worker must reach {FUNCTION_ACTIVE} within {PATIENCE:?}, last saw \
             {configuration}"
        );
        ::tokio::time::sleep(POLL).await;
    }
}

/// A custom event bus called `name`; answers the ARN EventBridge minted for it.
async fn given_bus(emulator: &Emulator, name: &str) -> String {
    let created = emulator.events("CreateEventBus", &json!({ "Name": name })).await;
    string(&created, "EventBusArn")
}

/// A rule called `name` on the bus `bus`, matching `detail_type` and `nonce`, with `targets`
/// wired to it.
async fn given_rule_on(
    emulator: &Emulator,
    case: &str,
    bus: &str,
    name: &str,
    detail_type: &str,
    nonce: &str,
    targets: Value,
) {
    emulator
        .events(
            "PutRule",
            &json!({
                "Name": name,
                "EventBusName": bus,
                "State": "ENABLED",
                "EventPattern": json!({
                    "source": [SOURCE],
                    "detail-type": [detail_type],
                    "detail": { "nonce": [nonce] },
                })
                .to_string(),
            }),
        )
        .await;
    let wired = emulator
        .events(
            "PutTargets",
            &json!({ "Rule": name, "EventBusName": bus, "Targets": targets }),
        )
        .await;
    assert_eq!(
        wired.get("FailedEntryCount").and_then(Value::as_i64),
        Some(0),
        "case {case:?}: every target of {name} must be accepted, got {wired}"
    );
}

/// The one envelope `url` is delivered, looked for until the publication the row's Task makes
/// has been routed to it.
async fn delivered_envelope(emulator: &Emulator, url: &str, case: &str) -> Value {
    let began = Instant::now();
    let mut held: Vec<Value> = Vec::new();
    loop {
        held.extend(read_once(emulator, url, case).await);
        if let [only] = held.as_slice() {
            return only.clone();
        }
        assert!(
            held.is_empty(),
            "case {case:?}: one publication must reach the matching rule once, got {held:?}"
        );
        assert!(
            began.elapsed() < PATIENCE,
            "case {case:?}: the Task's publication must reach the matching rule within \
             {PATIENCE:?}, nothing arrived"
        );
        ::tokio::time::sleep(POLL).await;
    }
}

/// `execution` once it has left [`RUNNING`].
async fn settled(emulator: &Emulator, execution: &str, case: &str) -> Value {
    let began = Instant::now();
    loop {
        let described = emulator
            .sfn("DescribeExecution", &json!({ "executionArn": execution }))
            .await;
        if described.get("status").and_then(Value::as_str) != Some(RUNNING) {
            return described;
        }
        assert!(
            began.elapsed() < PATIENCE,
            "case {case:?}: the answered callback task must take the execution out of \
             {RUNNING} within {PATIENCE:?}, last saw {described}"
        );
        ::tokio::time::sleep(POLL).await;
    }
}

/// Everything `url` was delivered over [`SILENCE_WINDOW`] — empty for a rule the row's
/// publication did not match.
async fn nothing_delivered(emulator: &Emulator, url: &str, case: &str) -> Vec<Value> {
    let deadline = Instant::now() + SILENCE_WINDOW;
    let mut held: Vec<Value> = Vec::new();
    while Instant::now() < deadline {
        held.extend(read_once(emulator, url, case).await);
        ::tokio::time::sleep(POLL).await;
    }
    held
}

/// The one-state definition a row runs: a callback `Task` over the event-publication
/// integration, whose rendered entry carries the row's own document.
fn definition(row: &Row, bus: &str, nonce: &str) -> Value {
    json!({
        "StartAt": "RequestWork",
        "States": {
            "RequestWork": {
                "Type": "Task",
                "Resource": "arn:aws:states:::events:putEvents.waitForTaskToken",
                "Parameters": {
                    "Entries": [{
                        "EventBusName": bus,
                        "Source": SOURCE,
                        "DetailType": MATCHING_DETAIL_TYPE,
                        "Detail": row.token_at.detail(nonce, row.answer),
                    }],
                },
                "TimeoutSeconds": TASK_TIMEOUT_SECONDS,
                "End": true,
            },
        },
    })
}

/// Every row: the token the state minted travels out in the event the Task publishes, the
/// matching rule's targets receive that very envelope, the work answers the token out of it,
/// and the paused execution ends on that answer — while the sibling rule the publication does
/// not match receives nothing from it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_event_publication_callback_task_is_finished_by_the_answer_to_its_token() {
    let emulator = Emulator::new();
    let worker = given_worker(&emulator, "callback-worker").await;

    // One token answers one task, so no two rows may ever see the same one.
    let mut tokens: BTreeSet<String> = BTreeSet::new();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let nonce = format!("nonce-{index}");
        let job = format!("job-{nonce}-0");

        let bus_arn = given_bus(&emulator, &format!("callback-bus-{index}")).await;
        let bus_name = format!("callback-bus-{index}");
        let (matched_url, matched_queue) =
            given_queue(&emulator, &format!("callback-matched-{index}")).await;
        let (missed_url, missed_queue) =
            given_queue(&emulator, &format!("callback-missed-{index}")).await;

        given_rule_on(
            &emulator,
            case,
            &bus_name,
            &format!("callback-mismatch-{index}"),
            MISMATCHED_DETAIL_TYPE,
            &nonce,
            json!([{ "Id": "missed-queue", "Arn": missed_queue }]),
        )
        .await;
        given_rule_on(
            &emulator,
            case,
            &bus_name,
            &format!("callback-match-{index}"),
            MATCHING_DETAIL_TYPE,
            &nonce,
            json!([
                { "Id": "matched-queue", "Arn": matched_queue },
                { "Id": "worker", "Arn": worker },
            ]),
        )
        .await;

        let addressed = if row.bus_by_arn {
            bus_arn.clone()
        } else {
            bus_name.clone()
        };
        let created = emulator
            .sfn(
                "CreateStateMachine",
                &json!({
                    "name": format!("callback-events-{index}"),
                    "definition": definition(row, &addressed, &nonce).to_string(),
                    "roleArn": ROLE_ARN,
                }),
            )
            .await;
        let machine = string(&created, "stateMachineArn");
        let started = emulator
            .sfn(
                "StartExecution",
                &json!({
                    "stateMachineArn": machine,
                    "name": "only",
                    "input": json!({ "job": &job }).to_string(),
                }),
            )
            .await;
        let execution = string(&started, "executionArn");

        // The entry the state rendered reached the matching rule, carrying the document the
        // definition built: the job out of the execution's input and the token out of the
        // context object, at the place this row put it.
        let envelope = delivered_envelope(&emulator, &matched_url, case).await;
        assert_eq!(
            envelope.get("source").and_then(Value::as_str),
            Some(SOURCE),
            "case {case:?}: the envelope names the Task's own source, got {envelope}"
        );
        assert_eq!(
            envelope.get("detail-type").and_then(Value::as_str),
            Some(MATCHING_DETAIL_TYPE),
            "case {case:?}: the envelope names the Task's own detail type, got {envelope}"
        );
        let detail = envelope
            .get("detail")
            .unwrap_or_else(|| panic!("case {case:?}: expected a detail in {envelope}"))
            .clone();
        assert_eq!(
            detail.get("job").and_then(Value::as_str),
            Some(job.as_str()),
            "case {case:?}: expected the job the execution was started with, got {detail}"
        );
        assert_eq!(
            detail.get("nonce").and_then(Value::as_str),
            Some(nonce.as_str()),
            "case {case:?}: expected the constant the definition carried, got {detail}"
        );
        let token = row.token_at.token_in(case, &detail);
        assert!(
            !token.is_empty() && token != "$$.Task.Token",
            "case {case:?}: expected the context object's token to have been substituted, \
             got {token:?}"
        );
        assert!(
            tokens.insert(token.clone()),
            "case {case:?}: one token answers one task, so {token:?} must not have served \
             an earlier row"
        );
        let event_id = envelope
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: expected an event id in {envelope}"))
            .to_string();
        assert!(
            !event_id.is_empty(),
            "case {case:?}: expected a non-empty event id, got {envelope}"
        );

        // Nothing but the answer to that token finishes the task, and what the answer
        // reported is the state's own outcome.
        let described = settled(&emulator, &execution, case).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some(row.answer.status()),
            "case {case:?}: expected {}, got {described}",
            row.answer.status()
        );
        match row.answer {
            Answer::Success => {
                assert_eq!(
                    (described.get("error"), described.get("cause")),
                    (None, None),
                    "case {case:?}: an answered success reports neither error nor cause, got \
                     {described}"
                );
                let expected = expected_output(&event_id, &job);
                assert_eq!(
                    described.get("output").and_then(Value::as_str),
                    Some(expected.as_str()),
                    "case {case:?}: expected the execution's output to be exactly the string \
                     the worker answered the token with ({expected:?}), got {described}"
                );
            }
            Answer::Failure => {
                assert_eq!(
                    described.get("error").and_then(Value::as_str),
                    Some(REPORTED_ERROR),
                    "case {case:?}: expected the error the answer named, got {described}"
                );
                assert_eq!(
                    described.get("cause").and_then(Value::as_str),
                    Some(REPORTED_CAUSE),
                    "case {case:?}: expected the cause the answer named, got {described}"
                );
                assert_eq!(
                    described.get("output"),
                    None,
                    "case {case:?}: a failed execution reports no output, got {described}"
                );
            }
        }

        // The same publication matched the sibling rule on no field, so its queue took
        // nothing from it.
        let missed = nothing_delivered(&emulator, &missed_url, case).await;
        assert!(
            missed.is_empty(),
            "case {case:?}: the rule whose detail type the publication does not carry must \
             receive nothing from it, got {missed:?}"
        );
    }
}
