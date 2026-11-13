// SPDX-License-Identifier: CC0-1.0

//! The ERROR NAME the status-change event of an execution whose `Task` work was never
//! handed over carries.
//!
//! The rule under test: a `Task` state whose service-integration call is NOT MADE —
//! nothing is handed to the invoked service, so no work begins — ends the execution
//! `FAILED` with the Amazon States Language's `States.Runtime` and a cause saying what
//! was not run, and the "Step Functions Execution Status Change" event published onto
//! the account's default bus carries exactly that `error` and `cause` in its `detail`.
//! `States.Timeout` names the OTHER outcome: work that WAS handed over and then ran past
//! a bound the state declared for it ("TimeoutSeconds […] If the task runs longer than
//! the specified seconds, this state fails with a States.Timeout error name"). A consumer
//! that branches on the event's error code — redrive tooling, an alarm, a retry policy —
//! is therefore told which of the two happened, and a timeout is never reported where
//! nothing timed out.
//!
//! The table spans the classes a `Resource` is declined in — a service whose wire facts
//! this emulator does not carry, an operation of a REST service whose own bindings are
//! not carried, the run-a-job pattern over an integration whose job cannot be reported, an
//! integration pattern this service does not name, a bare resource ARN and a `Resource`
//! that is no ARN at all. Beside it stands the case the rule tells those FROM: an activity
//! task a worker TAKES and then never answers, which runs past the `TimeoutSeconds` its
//! state declared and whose event must carry `States.Timeout`. Each event is read out of
//! the SQS queue an EventBridge rule on the default bus targets, so an error that arrives
//! was published by Step Functions itself, and each is checked against what
//! `DescribeExecution` reports for the same execution: the event and the describe cannot
//! disagree.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::execution_status_change_events::{
    given_execution, given_machine, given_rule, read_once, Emulator, EVENT_SOURCE,
    EXECUTION_STATUS_CHANGE,
};

/// The two Amazon States Language error names this suite tells apart, spelled out here
/// rather than imported from the implementation, so a wrong constant in the service
/// cannot make a test pass.
const RUNTIME_ERROR: &str = "States.Runtime";
const TIMEOUT_ERROR: &str = "States.Timeout";

/// How long one poll of the rule's queue waits before reading again.
const POLL: Duration = Duration::from_millis(25);

/// How long the suite waits for an event a run owes before giving up — generous, because
/// what it waits for is a CONDITION (the terminal event of that execution) and not a span.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(60);

/// The `TimeoutSeconds` the contrast test's state declares for the work its worker takes
/// and never answers, which is the bound whose expiry that execution's `States.Timeout`
/// reports.
const ACTIVITY_TIMEOUT_SECONDS: u64 = 1;

/// One row: the class of `Resource` it stands for, and the `Task` state that carries it.
struct Row {
    case: &'static str,
    /// The `Task` state the row's definition is made of, as a definition writes it.
    state: Value,
}

/// A one-`Task` state machine around `state`.
fn definition(state: &Value) -> String {
    json!({ "StartAt": "Work", "States": { "Work": state } }).to_string()
}

/// A `Task` state calling `resource` with `parameters`.
fn task(resource: &str, parameters: Value) -> Value {
    json!({
        "Type": "Task",
        "Resource": resource,
        "Parameters": parameters,
        "End": true,
    })
}

/// The `detail` of the FAILED status-change event of `execution` among `seen`, with the
/// envelope of that event asserted: a Step Functions execution status change names this
/// service as its source, the status change as its detail type, and the execution it is
/// about in `resources`.
fn failed_among(seen: &[Value], execution: &str, case: &str) -> Option<Value> {
    let event = seen.iter().find(|event| {
        event["detail"]["executionArn"].as_str() == Some(execution)
            && event["detail"]["status"].as_str() == Some("FAILED")
    })?;
    assert_eq!(
        event["source"].as_str(),
        Some(EVENT_SOURCE),
        "case {case:?}: expected source {EVENT_SOURCE}, got {event}"
    );
    assert_eq!(
        event["detail-type"].as_str(),
        Some(EXECUTION_STATUS_CHANGE),
        "case {case:?}: expected detail-type {EXECUTION_STATUS_CHANGE}, got {event}"
    );
    assert_eq!(
        event["resources"],
        json!([execution]),
        "case {case:?}: expected the execution in resources, got {event}"
    );
    Some(event["detail"].clone())
}

/// Read the rule's queue until the FAILED status-change event of `execution` has been
/// delivered, keeping every other event read along the way in `seen` — a message is left
/// out of the queue once read, so the events of the rows around this one accumulate
/// there rather than being lost.
///
/// The wait is for the CONDITION: the queue is looked at again until the event is there
/// or a generous deadline passes.
async fn failed_detail(
    emulator: &Emulator,
    url: &str,
    execution: &str,
    case: &str,
    seen: &mut Vec<Value>,
) -> Value {
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    loop {
        if let Some(detail) = failed_among(seen, execution, case) {
            return detail;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: no FAILED status-change event for {execution} was delivered; the \
             bus delivered {:?}",
            delivered_pairs(seen)
        );
        seen.extend(read_once(emulator, url, case).await);
        ::tokio::time::sleep(POLL).await;
    }
}

/// The `(status, error)` of every event read so far, which is what a failure report has
/// to name: the pair a consumer selecting on the error code would have seen.
fn delivered_pairs(seen: &[Value]) -> Vec<(String, String)> {
    seen.iter()
        .map(|event| {
            let detail = &event["detail"];
            (
                detail["status"].as_str().unwrap_or("<none>").to_string(),
                detail["error"].as_str().unwrap_or("<none>").to_string(),
            )
        })
        .collect()
}

/// The error a declined integration is reported under, over every class of `Resource`
/// the service makes no call for.
#[tokio::test]
async fn the_event_of_an_unmade_task_integration_carries_the_runtime_error() {
    let emulator = Emulator::new();
    let url = given_rule(
        &emulator,
        "sfn-terminal-status",
        &json!({ "source": [EVENT_SOURCE], "detail-type": [EXECUTION_STATUS_CHANGE] }),
    )
    .await;

    let rows = [
        Row {
            case: "a service this emulator carries no wire facts for",
            state: task(
                "arn:aws:states:::codebuild:startBuild",
                json!({ "ProjectName": "nightly" }),
            ),
        },
        Row {
            // AWS Backup is reached over a REST protocol, which spreads an operation's
            // input over the path, the query, the headers and a body separately: where
            // each member of `DeleteBackupVault` goes is that operation's own fact, and
            // this emulator carries the bindings of other Backup operations only.
            case: "an operation of a REST service whose own bindings are not carried",
            state: task(
                "arn:aws:states:::backup:deleteBackupVault",
                json!({ "BackupVaultName": "nightly" }),
            ),
        },
        Row {
            case: "the run-a-job pattern over an integration whose job cannot be reported",
            state: task(
                "arn:aws:states:::ecs:runTask.sync",
                json!({ "Cluster": "default", "TaskDefinition": "batch:1" }),
            ),
        },
        Row {
            case: "an integration pattern this service does not name",
            state: task(
                "arn:aws:states:::sqs:sendMessage.waitForever",
                json!({ "QueueUrl": "https://example.invalid/q", "MessageBody": "hi" }),
            ),
        },
        Row {
            // A bare resource ARN of a service whose resources are not work in
            // themselves, so it names no call to make.
            case: "a bare resource ARN",
            state: task(
                "arn:aws:sqs:us-west-2:000000000000:orders",
                json!({ "MessageBody": "hi" }),
            ),
        },
        Row {
            case: "a Resource that is no ARN at all",
            state: task("sendMessage", json!({ "MessageBody": "hi" })),
        },
    ];

    let mut seen: Vec<Value> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let Row { case, state } = row;
        let machine = given_machine(
            &emulator,
            &format!("unmade-{index}"),
            &definition(state),
            "STANDARD",
        )
        .await;
        let execution = given_execution(&emulator, &machine, &format!("run-{index}"), "{}").await;

        let detail = failed_detail(&emulator, &url, &execution, case, &mut seen).await;
        assert_eq!(
            detail["error"].as_str(),
            Some(RUNTIME_ERROR),
            "case {case:?}: expected the event to carry {RUNTIME_ERROR}, got {detail}"
        );
        let cause = detail["cause"].as_str().unwrap_or_default();
        assert!(
            !cause.is_empty(),
            "case {case:?}: a failed execution's event must say what happened, got {detail}"
        );
        assert!(
            cause.contains("nothing was run for it"),
            "case {case:?}: the cause must say nothing ran, got {cause:?}"
        );

        // The event and a describe of the same execution are one record read twice.
        let described = emulator
            .sfn("DescribeExecution", &json!({ "executionArn": &execution }))
            .await;
        assert_eq!(
            described["status"].as_str(),
            Some("FAILED"),
            "case {case:?}: expected the execution to have failed, got {described}"
        );
        assert_eq!(
            (described["error"].as_str(), described["cause"].as_str()),
            (detail["error"].as_str(), detail["cause"].as_str()),
            "case {case:?}: the event and the describe must agree, got {described} and {detail}"
        );
    }
}

/// The row the rule above tells those from: work that WAS handed over and then outlasted
/// the bound its own state declared for it is the one outcome `States.Timeout` names, and
/// the event of that execution carries that name rather than the runtime error.
///
/// An activity is that work: it is scheduled for a worker that polls for it, and the
/// worker below takes the task and then goes quiet — so the work was handed over, and the
/// bound the state declared for it is what closes it.
#[tokio::test]
async fn the_event_of_work_that_outlasted_its_declared_bound_carries_the_timeout_error() {
    let case = "an activity task nobody answers inside the TimeoutSeconds it declared";
    let emulator = Emulator::new();
    let url = given_rule(
        &emulator,
        "sfn-timed-out-status",
        &json!({ "source": [EVENT_SOURCE], "detail-type": [EXECUTION_STATUS_CHANGE] }),
    )
    .await;
    let activity = emulator
        .sfn("CreateActivity", &json!({ "name": "nobody-polls" }))
        .await;
    let activity_arn = activity["activityArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no activityArn in {activity}"))
        .to_string();
    let state = json!({
        "Type": "Task",
        "Resource": activity_arn,
        "TimeoutSeconds": ACTIVITY_TIMEOUT_SECONDS,
        "End": true,
    });
    let machine = given_machine(&emulator, "timed-out", &definition(&state), "STANDARD").await;
    let execution = given_execution(&emulator, &machine, "run", "{}").await;

    // A worker TAKES the task — an activity task's own clock starts when a poll takes it
    // — and then answers nothing, which is the work outlasting the bound its state
    // declared. Looked for again until the task is there, which is the condition and not a
    // span.
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    loop {
        let answered = emulator
            .sfn(
                "GetActivityTask",
                &json!({ "activityArn": &activity_arn, "workerName": "quiet" }),
            )
            .await;
        let token = answered["taskToken"].as_str().unwrap_or_default();
        if !token.is_empty() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the scheduled task was never handed out, last saw {answered}"
        );
        ::tokio::time::sleep(POLL).await;
    }

    // Wait for the CONDITION — the execution having settled — and not for a span.
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    let described = loop {
        let described = emulator
            .sfn("DescribeExecution", &json!({ "executionArn": &execution }))
            .await;
        if described["status"].as_str() != Some("RUNNING") {
            break described;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the execution never settled, last read {described}"
        );
        ::tokio::time::sleep(POLL).await;
    };
    assert_eq!(
        (described["status"].as_str(), described["error"].as_str()),
        (Some("FAILED"), Some(TIMEOUT_ERROR)),
        "case {case:?}: expected FAILED with {TIMEOUT_ERROR}, got {described}"
    );

    let mut seen: Vec<Value> = Vec::new();
    let detail = failed_detail(&emulator, &url, &execution, case, &mut seen).await;
    assert_eq!(
        detail["error"].as_str(),
        Some(TIMEOUT_ERROR),
        "case {case:?}: expected the event to carry {TIMEOUT_ERROR}, got {detail}"
    );
    assert_eq!(
        (described["error"].as_str(), described["cause"].as_str()),
        (detail["error"].as_str(), detail["cause"].as_str()),
        "case {case:?}: the event and the describe must agree, got {described} and {detail}"
    );
}
