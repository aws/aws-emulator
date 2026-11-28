// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `StopExecution` against an execution that has ALREADY reached
//! a terminal status, driven over this service's own wire protocol (`awsJson1_0`,
//! `POST /` with `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`.
//!
//! The rules under test:
//!
//! 1. an execution whose definition reaches a terminal state reaches that state's own
//!    outcome, whatever the run cost this process on the way: a poller loop that gets
//!    to its `Succeed` ends `SUCCEEDED` carrying the data its own states computed;
//! 2. `StopExecution` against an execution that is already terminal is a REPORT of the
//!    stop that execution already carries: it answers 200 with that execution's own
//!    `stopDate` and changes nothing a later caller sees — the status, the output, the
//!    `error`, the `cause`, the `stopDate` and the history are what they were — however
//!    many times it is called and whatever `error`/`cause` the call carries;
//! 3. it is the TERMINAL STATUS that makes the call a no-op, not the record having once
//!    carried a `stopDate`: an execution running again after a redrive is moved to
//!    `ABORTED` by a stop, with the `error` and `cause` that call named.
//!
//! Rule 2's rows differ along the terminal status the execution reached and along HOW it
//! reached it — a loop that polls its way to a `Succeed`, a `Pass` that succeeds at
//! once, a `Fail` state's declared failure, a definition whose own `TimeoutSeconds` ran
//! out, and an earlier stop — which is every terminal value of `ExecutionStatus`
//! (`smithy_query(query="shape",shape="ExecutionStatus")`: `RUNNING`, `SUCCEEDED`,
//! `FAILED`, `TIMED_OUT`, `ABORTED`, `PENDING_REDRIVE`) that a start of a STANDARD state
//! machine reaches. Each row is then stopped over a table of payloads — neither field,
//! each field alone, and both — twice over, because the rule is an idempotence.
//!
//! The harness (`Emulator`, `create_machine`, `start_execution`, `describe_execution`,
//! `string`) is the one `state_machine_lifecycle` builds and the whole-history reader is
//! `execution_history`'s; `tests/all.rs` is this binary's crate root, so each is reached
//! through `crate::<suite>`.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::execution_history::history;
use crate::state_machine_lifecycle::{
    create_machine, describe_execution, start_execution, string, Emulator, HELLO_DEFINITION,
};

/// How long a poll below waits for a terminal status before giving up.
///
/// Generous on purpose: the outcome is WAITED for rather than timed, so a loaded or a
/// fast host reaches the same verdict.
const TERMINAL_DEADLINE: Duration = Duration::from_secs(120);

/// How long a poll sleeps between looks at an execution that is still running.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// The turns the poller row's loop takes before its `Choice` lets it reach the
/// `Succeed`: enough that the run really goes round, parking in its `Wait` each time.
const POLL_TURNS: i64 = 3;

/// The seconds the poller row's loop waits on each turn.
const BACKOFF_SECONDS: u64 = 1;

/// The seconds the row that declares one gives its WHOLE execution, and the seconds
/// the state it parks in would wait for — far past it, so the definition's own bound is
/// what ends that run.
const DEFINITION_TIMEOUT_SECONDS: u64 = 1;
const LONGER_THAN_THE_SUITE_SECONDS: u64 = 600;

/// The error and cause the `Fail` row's own state declares.
const FAIL_ERROR: &str = "Tenant.NotProvisioned";
const FAIL_CAUSE: &str = "the tenant was never provisioned";

/// The error and cause the stop that ABORTS a running execution carries.
const ABORT_ERROR: &str = "StoppedByOperator";
const ABORT_CAUSE: &str = "the operator stopped the run";

/// A definition whose `Task` -> `Choice` -> `Wait` loop polls its own data until the
/// `Choice` lets it through to a `Succeed`: the shape a poller workflow has, with the
/// work each turn does done by the states themselves.
fn poller_definition() -> String {
    json!({
        "StartAt": "Poll",
        "States": {
            "Poll": {
                "Type": "Pass",
                "Parameters": { "attempts.$": "States.MathAdd($.attempts, 1)" },
                "Next": "CheckReady",
            },
            "CheckReady": {
                "Type": "Choice",
                "Choices": [
                    {
                        "Variable": "$.attempts",
                        "NumericGreaterThanEquals": POLL_TURNS,
                        "Next": "Ready",
                    },
                ],
                "Default": "Backoff",
            },
            "Backoff": { "Type": "Wait", "Seconds": BACKOFF_SECONDS, "Next": "Poll" },
            "Ready": { "Type": "Succeed" },
        },
    })
    .to_string()
}

/// A definition whose one `Fail` state ends the execution with the error and cause it
/// declares.
fn failing_definition() -> String {
    json!({
        "StartAt": "Bang",
        "States": {
            "Bang": { "Type": "Fail", "Error": FAIL_ERROR, "Cause": FAIL_CAUSE },
        },
    })
    .to_string()
}

/// A definition that would wait far longer than this suite runs, so an execution of it
/// is still `RUNNING` when it is stopped.
fn waiting_definition() -> String {
    json!({
        "StartAt": "Park",
        "States": {
            "Park": {
                "Type": "Wait",
                "Seconds": LONGER_THAN_THE_SUITE_SECONDS,
                "Next": "Ready",
            },
            "Ready": { "Type": "Succeed" },
        },
    })
    .to_string()
}

/// The same definition under an execution-level `TimeoutSeconds` the parked state goes
/// far past: "the maximum number of seconds an execution of the state machine can run".
fn timing_out_definition() -> String {
    let mut written: Value =
        serde_json::from_str(&waiting_definition()).expect("the definition is JSON");
    written["TimeoutSeconds"] = json!(DEFINITION_TIMEOUT_SECONDS);
    written.to_string()
}

/// The stops every terminal row below takes, differing along the fields the call may
/// carry: "a caller can stop an execution without using any KMS permissions [...] if
/// the caller provides a null value for both error and cause fields", so neither field,
/// each one alone and both are all requests this operation serves.
fn stop_payloads() -> Vec<(&'static str, Value)> {
    vec![
        ("neither error nor cause", json!({})),
        ("an error alone", json!({ "error": "Cancelled" })),
        ("a cause alone", json!({ "cause": "a second operator asked" })),
        (
            "both an error and a cause",
            json!({ "error": "Cancelled", "cause": "a second operator asked" }),
        ),
    ]
}

/// One row of the terminal table: the definition it runs, the input it is started
/// with, whether it is stopped while still running, and the outcome it is terminal at.
struct Row {
    case: &'static str,
    definition: String,
    input: &'static str,
    /// The stop that ABORTS this row's execution while it is still running, for the row
    /// whose terminal status is the one a stop produces.
    aborted_by: Option<Value>,
    status: &'static str,
    /// The output the row's own states computed, parsed, for a row that reaches one.
    output: Option<Value>,
    error: Option<&'static str>,
    cause: Option<&'static str>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a poller loop that reaches its Succeed",
            definition: poller_definition(),
            input: r#"{"attempts":0}"#,
            aborted_by: None,
            status: "SUCCEEDED",
            output: Some(json!({ "attempts": POLL_TURNS })),
            error: None,
            cause: None,
        },
        Row {
            case: "a Pass state that succeeds at once",
            definition: HELLO_DEFINITION.to_string(),
            input: r#"{"n":1}"#,
            aborted_by: None,
            status: "SUCCEEDED",
            output: Some(json!({ "msg": "hi" })),
            error: None,
            cause: None,
        },
        Row {
            case: "a Fail state that declares an error and a cause",
            definition: failing_definition(),
            input: "{}",
            aborted_by: None,
            status: "FAILED",
            output: None,
            error: Some(FAIL_ERROR),
            cause: Some(FAIL_CAUSE),
        },
        Row {
            case: "a definition whose own TimeoutSeconds ran out",
            definition: timing_out_definition(),
            input: "{}",
            aborted_by: None,
            status: "TIMED_OUT",
            output: None,
            error: None,
            cause: None,
        },
        Row {
            case: "an execution a caller had already stopped",
            definition: waiting_definition(),
            input: "{}",
            aborted_by: Some(json!({ "error": ABORT_ERROR, "cause": ABORT_CAUSE })),
            status: "ABORTED",
            output: None,
            error: Some(ABORT_ERROR),
            cause: Some(ABORT_CAUSE),
        },
    ]
}

/// Poll an execution until it reports a status other than `RUNNING`, and answer that
/// record. Fails with `case` when [`TERMINAL_DEADLINE`] passes first.
async fn poll_until_terminal(emulator: &Emulator, case: &str, execution: &str) -> Value {
    let deadline = Instant::now() + TERMINAL_DEADLINE;
    loop {
        let described = describe_execution(emulator, execution).await;
        if string(&described, "status") != "RUNNING" {
            return described;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the execution to reach a terminal status, got {described:?}"
        );
        // Awaited, not blocked: what carries a run past a `Wait` state is work of this
        // emulator's own ([`crate::state::wait_clock`]), so a poll that held this thread
        // would be waiting for work it is keeping from running.
        ::tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One `StopExecution` request: the execution, plus the members the payload carries.
fn stop_request(execution: &str, payload: &Value) -> Value {
    let mut request = json!({ "executionArn": execution });
    let members = request.as_object_mut().expect("the request is an object");
    for (member, value) in payload.as_object().expect("the payload is an object") {
        members.insert(member.clone(), value.clone());
    }
    request
}

/// The `output` of a description, parsed, for a record that reports one.
fn output_of(described: &Value, case: &str) -> Option<Value> {
    let output = described.get("output").filter(|output| !output.is_null())?;
    let text = output
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected output as a string, got {output:?}"));
    Some(
        serde_json::from_str(text)
            .unwrap_or_else(|error| panic!("case {case:?}: expected JSON output, got {error}")),
    )
}

// ---------------------------------------------------------------------------
// Rules 1 and 2: a terminal execution's record, and the stop that reports it
// ---------------------------------------------------------------------------

/// Every row reaches the terminal outcome its own definition decides, and a stop of the
/// terminal execution answers that execution's own `stopDate` and changes nothing —
/// over and over, whatever fields the stop carries.
#[tokio::test]
async fn a_stop_of_a_terminal_execution_reports_it_and_changes_nothing() {
    let emulator = Emulator::new();
    let table = rows();

    // Every row is started first, so the waits the rows ask for overlap rather than
    // being paid one after another.
    let mut started = Vec::with_capacity(table.len());
    for (index, row) in table.iter().enumerate() {
        let machine = create_machine(&emulator, &format!("terminal-{index}"), &row.definition).await;
        let execution = start_execution(&emulator, &machine, "only", row.input).await;
        if let Some(payload) = &row.aborted_by {
            // The row whose terminal status a stop produces: this one call is the stop
            // that MOVES the record, and every stop below it is the no-op under test.
            let request = stop_request(&execution, payload);
            emulator.ok("StopExecution", &request).await;
        }
        started.push(execution);
    }

    for (row, execution) in table.iter().zip(started) {
        let case = row.case;
        let described = poll_until_terminal(&emulator, case, &execution).await;

        // Rule 1: the terminal outcome is the definition's own.
        assert_eq!(
            (
                described.get("status").and_then(Value::as_str),
                described.get("error").and_then(Value::as_str),
                described.get("cause").and_then(Value::as_str),
            ),
            (Some(row.status), row.error, row.cause),
            "case {case:?}: expected the definition's own outcome {} {:?} {:?}, got {described:?}",
            row.status,
            row.error,
            row.cause
        );
        assert_eq!(
            output_of(&described, case),
            row.output,
            "case {case:?}: expected the output the run's own states computed {:?}, got \
             {described:?}",
            row.output
        );
        let stop_date = described
            .get("stopDate")
            .cloned()
            .filter(|stop_date| !stop_date.is_null())
            .unwrap_or_else(|| {
                panic!("case {case:?}: a terminal execution carries a stopDate, got {described:?}")
            });
        let events = history(&emulator, &execution, json!({})).await;

        // Rule 2: every stop of it reports that stopDate and leaves the record and the
        // history exactly as they are. The payload table is walked TWICE: the rule is
        // an idempotence, so a stop that follows another stop must answer the same.
        for round in 1..=2 {
            for (payload_case, payload) in stop_payloads() {
                let label = format!("{case} stopped in round {round} with {payload_case}");
                let answered = emulator
                    .ok("StopExecution", &stop_request(&execution, &payload))
                    .await;
                assert_eq!(
                    answered.get("stopDate"),
                    Some(&stop_date),
                    "case {label:?}: expected the stopDate the execution already carries \
                     ({stop_date:?}), got {answered:?}"
                );
                let after = describe_execution(&emulator, &execution).await;
                assert_eq!(
                    after, described,
                    "case {label:?}: expected the record left exactly as it was \
                     ({described:?}), got {after:?}"
                );
                let after_events = history(&emulator, &execution, json!({})).await;
                assert_eq!(
                    after_events, events,
                    "case {label:?}: expected no new history event, got {after_events:?} after \
                     {events:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 3: it is the terminal status that makes the stop a no-op
// ---------------------------------------------------------------------------

/// An execution running AGAIN after a redrive is stopped as any other running execution
/// is — so the no-op above keys on the execution being terminal now, not on its record
/// having carried a `stopDate` before.
#[tokio::test]
async fn a_stop_of_an_execution_running_again_after_a_redrive_aborts_it() {
    let emulator = Emulator::new();
    let machine = create_machine(&emulator, "redriven-then-stopped", &waiting_definition()).await;
    let execution = start_execution(&emulator, &machine, "only", "{}").await;

    // The first stop moves the record: the execution is parked in its `Wait`.
    let first = emulator
        .ok(
            "StopExecution",
            &stop_request(
                &execution,
                &json!({ "error": ABORT_ERROR, "cause": ABORT_CAUSE }),
            ),
        )
        .await;
    let first_stop_date = first
        .get("stopDate")
        .cloned()
        .unwrap_or_else(|| panic!("case \"the first stop\": expected a stopDate in {first:?}"));

    // Redriven, the execution runs again, so it carries no stopDate at all.
    emulator
        .ok(
            "RedriveExecution",
            &json!({ "executionArn": execution, "clientToken": "redrive-1" }),
        )
        .await;
    let running = describe_execution(&emulator, &execution).await;
    assert_eq!(
        running.get("status").and_then(Value::as_str),
        Some("RUNNING"),
        "case \"after the redrive\": a redriven execution runs again, got {running:?}"
    );

    // So a stop of it is a stop, not a report: the record moves to the error and cause
    // THIS call named, and the instant it reports is its own.
    let second_error = "StoppedAgain";
    let second_cause = "the operator stopped the re-run";
    let second = emulator
        .ok(
            "StopExecution",
            &stop_request(
                &execution,
                &json!({ "error": second_error, "cause": second_cause }),
            ),
        )
        .await;
    let second_stop_date = second
        .get("stopDate")
        .cloned()
        .unwrap_or_else(|| panic!("case \"the second stop\": expected a stopDate in {second:?}"));
    let after = describe_execution(&emulator, &execution).await;
    assert_eq!(
        (
            after.get("status").and_then(Value::as_str),
            after.get("error").and_then(Value::as_str),
            after.get("cause").and_then(Value::as_str),
            after.get("stopDate"),
        ),
        (
            Some("ABORTED"),
            Some(second_error),
            Some(second_cause),
            Some(&second_stop_date),
        ),
        "case \"after the second stop\": expected the re-run stopped with this call's own \
         error and cause, got {after:?}"
    );
    assert_ne!(
        Some(&second_stop_date),
        Some(&first_stop_date),
        "case \"after the second stop\": the stop of the re-run reports its own instant, not \
         the instant of the attempt that was redriven"
    );

    // And now that it is terminal again, a further stop is the no-op once more.
    let repeated = emulator
        .ok(
            "StopExecution",
            &stop_request(&execution, &json!({ "error": "Ignored" })),
        )
        .await;
    assert_eq!(
        repeated.get("stopDate"),
        Some(&second_stop_date),
        "case \"a third stop\": expected the stopDate of the stop that landed, got {repeated:?}"
    );
    let unchanged = describe_execution(&emulator, &execution).await;
    assert_eq!(
        unchanged, after,
        "case \"a third stop\": expected the record left exactly as it was ({after:?}), got \
         {unchanged:?}"
    );
}
