// SPDX-License-Identifier: CC0-1.0

//! What a `SendTask*` call is refused with when the `taskToken` it quotes names a task
//! that is no longer outstanding, driven over this service's own wire protocol
//! (`awsJson1_0`, `POST /` with `X-Amz-Target: AWSStepFunctions.<Operation>`) through the
//! production `MultiServiceRouter`.
//!
//! The rule under test: `TaskTimedOut` carries ONE fixed sentence for the whole error
//! class — the condition, then the reason quoted after it — so the refusal states why the
//! token answers nothing and echoes no identifier of the request, of the token or of the
//! store; and it is inert, leaving the execution the token once belonged to exactly as it
//! was.
//!
//! The rows differ along what the rule generalises over: WHICH `SendTask*` operation is
//! refused (all three of them, each with its own members), and WHY the token answers
//! nothing — a task already answered, a token no task was ever scheduled under, a task
//! whose execution was stopped under it, a callback (`.waitForTaskToken`) token whose
//! execution has already succeeded, and one naming an execution the store never held.
//!
//! The harness (`Emulator`, `create_machine`, `start_execution`, `describe_execution`) is
//! the one `state_machine_lifecycle` builds and the activity helpers (`create_activity`,
//! `activity_definition`, `claim`, `MINTED_ID`) are `activity_callbacks`'; `tests/all.rs`
//! is this binary's crate root, so both are reached through `crate::<suite>` rather than
//! copied.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::task_token;

use crate::activity_callbacks::{activity_definition, claim, create_activity, MINTED_ID};
use crate::state_machine_lifecycle::{
    create_machine, describe_execution, error_code, start_execution, string, Emulator,
    HELLO_DEFINITION,
};

/// The WHOLE message every `TaskTimedOut` below must carry, spelled out here rather than
/// built by the code under test.
///
/// Source: the recorded service answers a `SendTaskSuccess` quoting a token whose task has
/// already been closed with exactly this sentence. It is fixed text for the error class —
/// the shape's own documentation is "the task token has either expired or the task
/// associated with the token has already been closed", and either way the task is gone —
/// so no row below expects any part of it to vary.
const TIMED_OUT_MESSAGE: &str = "Task Timed Out: 'Provided task does not exist anymore'";

/// Generous room for an outcome the clock owes: far longer than the delay itself, so a
/// loaded host still settles inside it. The loop below waits for the CONDITION, not for
/// this.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);

/// How long the poll below pauses between looks.
const POLL_PAUSE: Duration = Duration::from_millis(20);

/// One `SendTask*` operation, with the members it carries beside the `taskToken`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reply {
    Success,
    Failure,
    Heartbeat,
}

impl Reply {
    fn operation(self) -> &'static str {
        match self {
            Self::Success => "SendTaskSuccess",
            Self::Failure => "SendTaskFailure",
            Self::Heartbeat => "SendTaskHeartbeat",
        }
    }

    /// The request this reply sends for `token`: `SendTaskSuccess` must carry an `output`
    /// and `SendTaskFailure` may name an `error` and a `cause`, while a heartbeat carries
    /// the token alone.
    fn request(self, token: &str) -> Value {
        match self {
            Self::Success => json!({ "taskToken": token, "output": r#"{"replayed":true}"# }),
            Self::Failure => json!({
                "taskToken": token,
                "error": "WorkerError",
                "cause": "the worker replayed its reply",
            }),
            Self::Heartbeat => json!({ "taskToken": token }),
        }
    }
}

/// Every `SendTask*` operation of this API, so the wording asserted below is the error
/// class's and not one operation's.
const REPLIES: &[Reply] = &[Reply::Success, Reply::Failure, Reply::Heartbeat];

/// Describe `execution_arn` until it is no longer `RUNNING`, and report what it then says.
async fn settled(emulator: &Emulator, case: &str, execution_arn: &str) -> Value {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let described = describe_execution(emulator, execution_arn).await;
        if string(&described, "status") != "RUNNING" {
            return described;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected {execution_arn} to settle, got {described}"
        );
        ::tokio::time::sleep(POLL_PAUSE).await;
    }
}

/// Send `reply` for `token` and assert the refusal: 400, `TaskTimedOut`, and exactly the
/// one sentence this error class carries — quoting none of `absent`, the identifiers this
/// refusal must not leak.
async fn refused(emulator: &Emulator, case: &str, reply: Reply, token: &str, absent: &[&str]) {
    let operation = reply.operation();
    let (status, body) = emulator.call(operation, &reply.request(token)).await;
    let code = error_code(&body);
    let message = body.get("message").and_then(Value::as_str);
    assert_eq!(
        (status, code.as_str(), message),
        (400, "TaskTimedOut", Some(TIMED_OUT_MESSAGE)),
        "case {case:?}: expected {operation} to be refused with {TIMED_OUT_MESSAGE:?}, got \
         {status} with {body}"
    );
    let message = message.expect("the assertion above holds the message");
    for leaked in absent {
        assert!(
            !message.contains(leaked),
            "case {case:?}: expected {operation}'s refusal to quote no identifier, got \
             {leaked:?} in {message:?}"
        );
    }
}

/// A token whose ACTIVITY task was already answered, one no task was ever scheduled under,
/// and one whose execution was stopped under it: every `SendTask*` refuses each of them
/// with the one sentence of this error class.
#[tokio::test]
async fn every_send_task_call_refuses_a_closed_activity_token_with_the_one_sentence() {
    let emulator = Emulator::new();

    // Row 1: the task was ANSWERED, which closes it — so the token answers nothing more.
    let answered_activity = create_activity(&emulator, "answered").await;
    let answered_machine = create_machine(
        &emulator,
        "answered-machine",
        &activity_definition(&answered_activity, json!({})),
    )
    .await;
    let answered_execution =
        start_execution(&emulator, &answered_machine, "only", r#"{"n":1}"#).await;
    let (answered_token, _) = claim(&emulator, "the answered task", &answered_activity).await;
    emulator
        .ok(
            "SendTaskSuccess",
            &json!({ "taskToken": answered_token, "output": r#"{"done":true}"# }),
        )
        .await;

    // Row 3: the EXECUTION waiting at the task was stopped, so nobody answers the task.
    let stopped_activity = create_activity(&emulator, "stopped").await;
    let stopped_machine = create_machine(
        &emulator,
        "stopped-machine",
        &activity_definition(&stopped_activity, json!({})),
    )
    .await;
    let stopped_execution = start_execution(&emulator, &stopped_machine, "only", "{}").await;
    let (stopped_token, _) = claim(&emulator, "the stopped task", &stopped_activity).await;
    emulator
        .ok(
            "StopExecution",
            &json!({ "executionArn": stopped_execution }),
        )
        .await;

    let rows: Vec<(&str, String, Vec<&str>)> = vec![
        (
            "a token whose task a reply already answered",
            answered_token,
            vec![answered_activity.as_str(), answered_execution.as_str()],
        ),
        (
            "a well-formed token no task was ever scheduled under",
            task_token(MINTED_ID, &answered_activity),
            vec![answered_activity.as_str(), MINTED_ID],
        ),
        (
            "a token whose execution was stopped under its task",
            stopped_token,
            vec![stopped_activity.as_str(), stopped_execution.as_str()],
        ),
    ];

    for (case, token, absent) in &rows {
        for reply in REPLIES {
            refused(&emulator, case, *reply, token, absent).await;
        }
    }
}

/// A CALLBACK (`.waitForTaskToken`) token — one that names the execution that was waiting
/// at it rather than an activity — is refused by the same rule once that execution has
/// finished, and the refusal is INERT: the execution still reports what it settled with.
#[tokio::test]
async fn a_replayed_callback_token_is_refused_without_touching_the_execution() {
    let emulator = Emulator::new();
    let machine = create_machine(&emulator, "callback-machine", HELLO_DEFINITION).await;
    let execution = start_execution(&emulator, &machine, "only", r#"{"from":"the start"}"#).await;
    let case = "a callback token of an execution that has already succeeded";
    let succeeded = settled(&emulator, case, &execution).await;
    assert_eq!(
        succeeded.get("status").and_then(Value::as_str),
        Some("SUCCEEDED"),
        "case {case:?}: expected the execution to have succeeded, got {succeeded}"
    );
    let output = string(&succeeded, "output");

    let rows: Vec<(&str, String, Vec<&str>)> = vec![
        (
            case,
            task_token(MINTED_ID, &execution),
            vec![execution.as_str(), machine.as_str(), MINTED_ID],
        ),
        (
            "a callback token naming an execution the store never held",
            task_token(MINTED_ID, &format!("{execution}-never-started")),
            vec![execution.as_str(), MINTED_ID],
        ),
    ];

    for (case, token, absent) in &rows {
        for reply in REPLIES {
            refused(&emulator, case, *reply, token, absent).await;
        }
    }

    // The refusals changed nothing: the execution reports the status and the output it
    // settled with before them.
    let after = describe_execution(&emulator, &execution).await;
    assert_eq!(
        (
            after.get("status").and_then(Value::as_str),
            string(&after, "output")
        ),
        (Some("SUCCEEDED"), output.clone()),
        "case {case:?}: expected the refusals to leave the execution at {output:?}, got {after}"
    );
}
