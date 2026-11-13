// SPDX-License-Identifier: CC0-1.0

//! End-to-end test for the WHOLE late-answer hand-off of an activity-backed `Task`
//! state: the task the state schedules is delivered to a long-polling worker, the
//! state's own bound closes it when that worker goes quiet, and the answer the worker
//! sends afterwards is refused without touching the execution it was too late for —
//! driven over each service's own wire protocol through the production
//! `MultiServiceRouter` with Step Functions and CloudWatch mounted on ONE engine, so
//! the series are read back through CloudWatch's own `GetMetricStatistics` exactly as a
//! client charting them does.
//!
//! The rule under test: when the bound an activity-backed `Task` state declares runs
//! out before the worker that claimed the task answers, the service closes that task,
//! fails the execution with the language's own timeout error and no output, refuses
//! EVERY later call on that token with `TaskTimedOut` (400) without changing anything
//! the execution reports, and counts the close in the timed-out series of the ACTIVITY
//! and the failed series of the STATE MACHINE — never in either's succeeded series and
//! never in the execution-level timed-out series, which belongs to a state machine's
//! own bound; a worker that answers while the bound still holds moves the succeeded
//! series instead.
//!
//! The rows differ along what the rule generalises over: which of the three calls a
//! token-holder can make arrives too late, how long the bound it missed was, and
//! whether the worker answered inside the bound at all.
//!
//! The harness (`Metered`, `activity_dimensions`, `machine_dimensions`) is the one
//! `states_metric_families` builds and the definition builder is `activity_tasks`';
//! `tests/all.rs` is this binary's crate root, so both are reached through
//! `crate::<module>`.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::{
    asl::TIMEOUT_ERROR, observability, task_timed_out_message,
};

use crate::activity_tasks::activity_definition;
use crate::state_machine_lifecycle::{error_code, string};
use crate::states_metric_families::{activity_dimensions, machine_dimensions, Metered};

/// How long a poll for the scheduled task waits between looks.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// How long a row waits for the task to be delivered: generous, so a loaded host cannot
/// fail a row whose own declared bound is the only tight thing in it.
const PATIENCE: Duration = Duration::from_secs(60);

/// The name the rows' workers poll under, which the service only logs.
const WORKER: &str = "late-answer-worker";

/// The document the worker that answers in time reports as its task's result.
const IN_TIME_OUTPUT: &str = r#"{"ok":1}"#;

/// What the worker that answers too late would have reported.
const LATE_OUTPUT: &str = r#"{"late":true}"#;

/// The error a worker's own failure report carries, for the row whose failure arrives
/// too late to be recorded.
const LATE_ERROR: &str = "WorkerFailed";

/// When the worker that claimed the task answers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    /// It answers while the bound the state declared still holds.
    InTime,
    /// It goes quiet until the bound has run out, and only then makes this call.
    Late(&'static str),
}

/// One row: the bound the scheduling state declares, when its worker answers, and what
/// the execution and the series must then hold.
struct Row {
    case: &'static str,
    /// The bound the `Task` state declares on its task, in seconds.
    bound_seconds: u32,
    answer: Answer,
    /// The terminal status the execution reaches.
    status: &'static str,
    /// The `error` it reports, and `None` for one that reports none.
    error: Option<&'static str>,
    /// The `output` it reports, and `None` for one that carries no output member at all.
    output: Option<&'static str>,
    /// What the six series must hold: the activity's timed-out, succeeded and failed
    /// counts, then the state machine's failed, succeeded and timed-out ones.
    counted: [f64; 6],
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a success report that arrives past a one-second bound",
            bound_seconds: 1,
            answer: Answer::Late("SendTaskSuccess"),
            status: "FAILED",
            error: Some(TIMEOUT_ERROR),
            output: None,
            counted: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        },
        Row {
            case: "a failure report that arrives past a one-second bound",
            bound_seconds: 1,
            answer: Answer::Late("SendTaskFailure"),
            status: "FAILED",
            error: Some(TIMEOUT_ERROR),
            output: None,
            counted: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        },
        Row {
            case: "a heartbeat that arrives past a two-second bound",
            bound_seconds: 2,
            answer: Answer::Late("SendTaskHeartbeat"),
            status: "FAILED",
            error: Some(TIMEOUT_ERROR),
            output: None,
            counted: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        },
        Row {
            case: "a success report well inside a generous bound",
            bound_seconds: 300,
            answer: Answer::InTime,
            status: "SUCCEEDED",
            error: None,
            output: Some(IN_TIME_OUTPUT),
            counted: [0.0, 1.0, 0.0, 0.0, 1.0, 0.0],
        },
    ]
}

/// The request one of the three calls a token-holder can make carries.
fn reply_request(operation: &str, token: &str) -> Value {
    match operation {
        "SendTaskSuccess" => json!({ "taskToken": token, "output": LATE_OUTPUT }),
        "SendTaskFailure" => json!({ "taskToken": token, "error": LATE_ERROR }),
        "SendTaskHeartbeat" => json!({ "taskToken": token }),
        other => panic!("no reply of this suite is made with {other}"),
    }
}

/// Create an activity that must succeed, and answer its ARN.
///
/// `pub(crate)`, not private: `tests/all.rs` is this binary's crate root, so the suite
/// that drives SEVERAL executions through ONE activity's queue
/// (`crate::bounded_activity_task_outcomes`) reaches these three helpers rather than
/// carrying a second copy of them.
pub(crate) async fn create_activity(metered: &Metered, case: &str, name: &str) -> String {
    let created = metered
        .ok(case, "CreateActivity", &json!({ "name": name }))
        .await;
    string(&created, "activityArn")
}

/// Start one execution on `input`, and answer its ARN.
pub(crate) async fn start_execution(
    metered: &Metered,
    case: &str,
    machine: &str,
    input: &str,
) -> String {
    let started = metered
        .ok(
            case,
            "StartExecution",
            &json!({ "stateMachineArn": machine, "name": "run", "input": input }),
        )
        .await;
    string(&started, "executionArn")
}

/// The `(taskToken, input)` the activity's queue hands a long-polling worker, looked for
/// again until one arrives or [`PATIENCE`] has passed.
///
/// The poll is a LONG poll, so the first call already waits for the task the running
/// execution scheduled; the loop is what makes a row that needs a second look wait for
/// the CONDITION rather than for a span of time. A task no worker holds is bounded by
/// nothing — an activity task's own clock starts when a poll takes it — so however long
/// this takes, the task a row claims is still there to be claimed.
pub(crate) async fn claimed(metered: &Metered, case: &str, activity_arn: &str) -> (String, Value) {
    let began = Instant::now();
    loop {
        let answered = metered
            .ok(
                case,
                "GetActivityTask",
                &json!({ "activityArn": activity_arn, "workerName": WORKER }),
            )
            .await;
        if let Some(token) = answered.get("taskToken").and_then(Value::as_str) {
            let delivered = answered
                .get("input")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: a delivered task carries its input"));
            let parsed: Value = serde_json::from_str(delivered).unwrap_or_else(|error| {
                panic!("case {case:?}: expected the state's input as JSON, got {error} for {delivered:?}")
            });
            return (token.to_string(), parsed);
        }
        assert!(
            began.elapsed() < PATIENCE,
            "case {case:?}: expected the scheduled task to be handed out within {PATIENCE:?}, \
             last saw {answered}"
        );
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// A `Task` state whose `Resource` is an activity ARN hands its task to a polling
/// worker; the bound it declares closes a task its worker left unanswered, and the
/// answer that worker sends afterwards is refused with `TaskTimedOut` and changes
/// nothing — with the close counted under the activity's own dimension and the
/// execution's failure under its state machine's.
#[tokio::test]
async fn a_late_answer_is_refused_and_only_the_timed_out_series_move() {
    let metered = Metered::new();

    for (index, row) in rows().into_iter().enumerate() {
        let Row {
            case,
            bound_seconds,
            answer,
            status,
            error,
            output,
            counted,
        } = row;
        // One activity and one state machine per row, so each row's series are its own.
        let activity = create_activity(&metered, case, &format!("late-answer-{index}")).await;
        let definition =
            activity_definition(&activity, json!({ "TimeoutSeconds": bound_seconds }));
        let machine = metered
            .machine(case, &format!("late-answer-{index}"), &definition)
            .await;
        let input = json!({ "case": index });
        let execution = start_execution(&metered, case, &machine, &input.to_string()).await;

        // The running execution queued a task on the activity, so the long poll is
        // handed a token and the input the state computed — the execution's own input
        // here, the state applying no data flow to it.
        let (token, delivered) = claimed(&metered, case, &activity).await;
        assert!(
            !token.is_empty(),
            "case {case:?}: expected a non-empty taskToken, got {token:?}"
        );
        assert_eq!(
            delivered, input,
            "case {case:?}: expected the input the state scheduled, got {delivered}"
        );

        let described = match answer {
            // The worker answers while the bound still holds: its report is recorded,
            // and the execution ends on it.
            Answer::InTime => {
                metered
                    .ok(
                        case,
                        "SendTaskSuccess",
                        &json!({ "taskToken": token, "output": IN_TIME_OUTPUT }),
                    )
                    .await;
                metered.settled(case, &execution).await
            }
            // The worker goes quiet: the bound the state declared is what closes the
            // task, and the call it makes afterwards is too late for the execution that
            // has already failed on it.
            Answer::Late(operation) => {
                let settled = metered.settled(case, &execution).await;
                let (refused_status, refusal) = metered
                    .sfn(operation, &reply_request(operation, &token))
                    .await;
                let code = error_code(&refusal);
                assert_eq!(
                    (refused_status, code.as_str()),
                    (400, "TaskTimedOut"),
                    "case {case:?}: expected {operation} on the stale token to be refused 400 \
                     TaskTimedOut, got {refused_status} {code} with {refusal}"
                );
                // The refusal's wording: the one construction every `SendTask*` answers
                // this condition with, which states the condition and carries nothing of
                // the request, the token or the store.
                let message = task_timed_out_message();
                assert_eq!(
                    refusal.get("message").and_then(Value::as_str),
                    Some(message.as_str()),
                    "case {case:?}: expected the refusal {message:?}, got {refusal}"
                );
                // And it changed NOTHING the execution reports: the same read answers
                // the same thing after it as before it.
                let again = metered
                    .ok(
                        case,
                        "DescribeExecution",
                        &json!({ "executionArn": execution }),
                    )
                    .await;
                assert_eq!(
                    again, settled,
                    "case {case:?}: expected the refused answer to leave the execution exactly \
                     as it was, got {again} after {settled}"
                );
                again
            }
        };

        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some(status),
            "case {case:?}: expected the execution to end {status}, got {described}"
        );
        assert_eq!(
            described.get("error").and_then(Value::as_str),
            error,
            "case {case:?}: expected the error {error:?}, got {described}"
        );
        // A failed execution carries no output member at all, and a succeeded one
        // carries exactly what its task reported.
        assert_eq!(
            described.get("output").and_then(Value::as_str),
            output,
            "case {case:?}: expected the output {output:?}, got {described}"
        );

        // The series, each read under the dimension set its own family is written with:
        // the activity's by activity ARN, the execution's by state-machine ARN.
        let of_activity = activity_dimensions(&activity);
        let of_machine = machine_dimensions(&machine);
        for (metric, dimensions, expected) in [
            (
                observability::ACTIVITIES_TIMED_OUT_METRIC,
                &of_activity,
                counted[0],
            ),
            (
                observability::ACTIVITIES_SUCCEEDED_METRIC,
                &of_activity,
                counted[1],
            ),
            (
                observability::ACTIVITIES_FAILED_METRIC,
                &of_activity,
                counted[2],
            ),
            (
                observability::EXECUTIONS_FAILED_METRIC,
                &of_machine,
                counted[3],
            ),
            (
                observability::EXECUTIONS_SUCCEEDED_METRIC,
                &of_machine,
                counted[4],
            ),
            (
                observability::EXECUTIONS_TIMED_OUT_METRIC,
                &of_machine,
                counted[5],
            ),
        ] {
            let summed = metered.summed(case, metric, dimensions).await;
            assert_eq!(
                summed, expected,
                "case {case:?}: expected {metric} to count {expected}, got {summed}"
            );
        }
        // And neither family is written under the other's dimension set, so a client
        // asking for exactly one of them reads only what belongs to it.
        for (metric, dimensions) in [
            (observability::ACTIVITIES_TIMED_OUT_METRIC, &of_machine),
            (observability::EXECUTIONS_FAILED_METRIC, &of_activity),
        ] {
            let crossed = metered.summed(case, metric, dimensions).await;
            assert_eq!(
                crossed, 0.0,
                "case {case:?}: {metric} belongs to one dimension set alone, got {crossed}"
            );
        }
    }
}
