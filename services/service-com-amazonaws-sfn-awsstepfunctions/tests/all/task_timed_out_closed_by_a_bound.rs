// SPDX-License-Identifier: CC0-1.0

//! What a `SendTask*` call is refused with when the task its `taskToken` names was
//! closed by a BOUND the scheduling state declared rather than by anything a caller did
//! — driven over this service's own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`.
//!
//! The rule under test is the one `task_timed_out_wording` pins for the tokens a CALLER
//! closed: `TaskTimedOut` states the condition and then quotes the ACTIVITY the closed
//! task was handed out by, so the refusal names that activity and echoes no other
//! identifier of the request, of the token or of the store — and it is inert, leaving the
//! execution the token belonged to exactly as the bound left it. The suites that drive the
//! clock-closed paths (`activity_heartbeats`, `activity_timeout_late_answer`) read that
//! message from the service's own constructor, so nothing yet holds those paths to the
//! WORDING itself; this suite spells it out and asserts it for them.
//!
//! The rows differ along what the rule generalises over: WHICH bound ran out
//! (`TimeoutSeconds`, the whole task's maximum duration; `HeartbeatSeconds`, the window
//! between a worker's reports; and each of them beside a far-off instance of the other,
//! so the bound that closed the task is the one under test), how long that bound was,
//! and WHICH of the three `SendTask*` calls then arrives under the stale token.
//!
//! The harness (`Emulator`, `create_machine`, `start_execution`, `describe_execution`) is
//! the one `state_machine_lifecycle` builds, the activity helpers (`create_activity`,
//! `activity_definition`, `claim`) are `activity_callbacks`', and the refusal assertion
//! (`REPLIES`, `refused`) is `task_timed_out_wording`'s; `tests/all.rs` is this binary's
//! crate root, so each is reached through `crate::<suite>` rather than copied.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::asl::TIMEOUT_ERROR;

use crate::activity_callbacks::{activity_definition, claim, create_activity};
use crate::state_machine_lifecycle::{
    create_machine, describe_execution, start_execution, Emulator,
};
use crate::task_timed_out_wording::{refused, timed_out_message, REPLIES};

/// Generous room for the outcome the bound owes: far longer than any bound a row below
/// declares, so a loaded host still reaches it. The loop waits for the CONDITION, not for
/// this.
const PATIENCE: Duration = Duration::from_secs(60);

/// How long the poll below pauses between looks.
const POLL_PAUSE: Duration = Duration::from_millis(25);

/// One row: the bounds the scheduling `Task` state declares, and the label its resources
/// are named after.
struct Row {
    case: &'static str,
    label: &'static str,
    /// The members the `Task` state carries beside its `Resource`.
    bounds: Value,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a one-second whole-task TimeoutSeconds, with the worker silent",
            label: "whole-task",
            bounds: json!({ "TimeoutSeconds": 1 }),
        },
        Row {
            case: "a two-second whole-task TimeoutSeconds beside a far-off heartbeat window",
            label: "whole-task-with-window",
            bounds: json!({ "TimeoutSeconds": 2, "HeartbeatSeconds": 300 }),
        },
        Row {
            case: "a one-second HeartbeatSeconds window nobody beat",
            label: "window",
            bounds: json!({ "HeartbeatSeconds": 1 }),
        },
        Row {
            case: "a one-second HeartbeatSeconds window beside a far-off whole-task bound",
            label: "window-with-whole-task",
            bounds: json!({ "HeartbeatSeconds": 1, "TimeoutSeconds": 300 }),
        },
    ]
}

/// Describe `execution_arn` until the bound has closed its task, and report what it then
/// says.
async fn closed_by_the_bound(emulator: &Emulator, case: &str, execution_arn: &str) -> Value {
    let began = Instant::now();
    loop {
        let described = describe_execution(emulator, execution_arn).await;
        if described.get("status").and_then(Value::as_str) != Some("RUNNING") {
            return described;
        }
        assert!(
            began.elapsed() < PATIENCE,
            "case {case:?}: expected the bound to close the task within {PATIENCE:?}, last saw \
             {described}"
        );
        ::tokio::time::sleep(POLL_PAUSE).await;
    }
}

/// A token whose task a BOUND closed is refused by the same rule as one a caller closed:
/// every `SendTask*` answers the condition and the activity that handed the task out,
/// quoting no other identifier, and changes nothing the execution reports.
#[tokio::test]
async fn every_send_task_call_refuses_a_bound_closed_token_naming_its_activity() {
    let emulator = Emulator::new();

    for row in rows() {
        let Row {
            case,
            label,
            bounds,
        } = row;
        // One activity, machine and execution per row, so each row's task is closed by
        // its own bound alone.
        let activity = create_activity(&emulator, &format!("{label}-activity")).await;
        let machine = create_machine(
            &emulator,
            &format!("{label}-machine"),
            &activity_definition(&activity, bounds),
        )
        .await;
        let execution = start_execution(&emulator, &machine, "only", r#"{"job":"work"}"#).await;
        // A claimed task is what a bound runs against: an activity task's clock starts
        // when a poll takes it.
        let (token, _) = claim(&emulator, case, &activity).await;

        let closed = closed_by_the_bound(&emulator, case, &execution).await;
        assert_eq!(
            (
                closed.get("status").and_then(Value::as_str),
                closed.get("error").and_then(Value::as_str),
            ),
            (Some("FAILED"), Some(TIMEOUT_ERROR)),
            "case {case:?}: expected the bound to fail the execution with the language's own \
             timeout error, got {closed}"
        );

        // None of the identifiers this refusal must not leak: the state machine the task
        // was scheduled under, the execution waiting at it, and the token itself. The
        // ACTIVITY is the one identifier it does carry.
        let absent = [machine.as_str(), execution.as_str(), token.as_str()];
        let expected = timed_out_message(&activity);
        for reply in REPLIES {
            refused(&emulator, case, *reply, &token, &expected, &absent).await;
        }

        // And the refusals changed nothing: the same read answers what the bound left.
        let after = describe_execution(&emulator, &execution).await;
        assert_eq!(
            after, closed,
            "case {case:?}: expected the refusals to leave the execution exactly as the bound \
             left it, got {after} after {closed}"
        );
    }
}
