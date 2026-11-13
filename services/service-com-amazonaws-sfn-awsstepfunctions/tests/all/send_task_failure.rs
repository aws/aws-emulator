// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `SendTaskFailure`: the OTHER reply a worker makes to the
//! activity task its `taskToken` names, driven over this service's own wire protocol
//! (`awsJson1_0`, `POST /` with `X-Amz-Target: AWSStepFunctions.<Operation>`) through
//! the production `MultiServiceRouter`.
//!
//! The rules under test:
//!
//! 1. a reply reporting FAILURE is that task's outcome just as a result is: the state
//!    that scheduled it declares no `Retry` and no `Catch`, so the execution waiting
//!    at it fails with exactly the `error` and `cause` the worker sent — both
//!    optional, because "a caller can mark a task as fail [...] if the caller provides
//!    a null value for both error and cause fields" — and that outcome is owed to the
//!    clock exactly as the one a successful reply continues to is;
//! 2. the reply CLOSES the task and writes the two events that failure leaves in the
//!    history, after the ones the start and the claim wrote; the execution it ended is
//!    terminal, so it reports `REDRIVABLE` and a redrive runs it again;
//! 3. a `taskToken` this reply cannot be applied to is refused with exactly the error
//!    the same token gets from `SendTaskSuccess` — `TaskTimedOut` (400) for a task that
//!    is no longer outstanding, `TaskDoesNotExist` (400) while the task is outstanding
//!    but its activity has been deleted, `InvalidToken` (400) for a value that is not a
//!    token this service issued — because the two operations resolve one token by one
//!    rule.
//!
//! The harness (`Emulator`, `create_machine`, `start_execution`, `describe_execution`)
//! is the one `state_machine_lifecycle` builds and the activity-reply helpers
//! (`create_activity`, `activity_definition`, `claim`, `history`, `held_tasks`,
//! `MINTED_ID`) are `activity_callbacks`'; `tests/all.rs` is this binary's crate root,
//! so both are reached through `crate::<suite>` rather than copied.

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::{
    asl::{MAX_CAUSE_BYTES, MAX_ERROR_BYTES},
    invalid_token_message, resource_error_message, task_timed_out_message, task_token,
    ACTIVITY_DOES_NOT_EXIST,
};

use crate::activity_callbacks::{
    activity_definition, claim, create_activity, held_tasks, history, MINTED_ID,
};
use crate::state_machine_lifecycle::{
    create_machine, describe_execution, error_code, start_execution, Emulator,
    PAST_THE_EXECUTION_DELAY,
};

/// Report that the task `token` names FAILED, with whatever members `reply` carries
/// beside the token, and answer `(status, body)`.
async fn send_failure(emulator: &Emulator, token: &str, reply: &Value) -> (u16, Value) {
    let Value::Object(members) = reply else {
        panic!("expected an object of reply members, got {reply:?}")
    };
    let mut request = json!({ "taskToken": token });
    let body = request.as_object_mut().expect("the request is an object");
    for (member, value) in members {
        body.insert(member.clone(), value.clone());
    }
    emulator.call("SendTaskFailure", &request).await
}

// ---------------------------------------------------------------------------
// What the failure the worker reported does to the execution
// ---------------------------------------------------------------------------

/// A reply reporting failure is the task state's outcome: the execution waiting at it
/// fails with exactly the `error` and `cause` the worker sent, and the task is closed
/// — over a table whose rows differ along which of the two optional members the reply
/// carries and how large they are.
#[tokio::test]
async fn send_task_failure_fails_the_execution_with_the_reported_error() {
    let emulator = Emulator::new();

    // The bounds are the modelled `SensitiveError` (0..256) and `SensitiveCause`
    // (0..32768) maxima, which those constants' own documentation names.
    let at_the_error_bound = "e".repeat(MAX_ERROR_BYTES);
    let at_the_cause_bound = "c".repeat(MAX_CAUSE_BYTES);
    let cases: Vec<(&str, Option<String>, Option<String>)> = vec![
        (
            "both members, as a worker reporting a named failure sends them",
            Some("WorkerError".to_string()),
            Some("the worker could not process the order".to_string()),
        ),
        ("an error with no cause", Some("BareError".to_string()), None),
        (
            "a cause with no error",
            None,
            Some("something went wrong, unnamed".to_string()),
        ),
        ("neither member, which needs no KMS permission", None, None),
        (
            "an empty error and an empty cause, which the 0 minimum allows",
            Some(String::new()),
            Some(String::new()),
        ),
        (
            "characters outside ASCII, carried verbatim",
            Some("Fehler".to_string()),
            Some("wörker-日本 konnte nicht".to_string()),
        ),
        (
            "both members at their modelled maxima",
            Some(at_the_error_bound.clone()),
            Some(at_the_cause_bound.clone()),
        ),
    ];

    for (index, (case, error, cause)) in cases.into_iter().enumerate() {
        let activity = create_activity(&emulator, &format!("failing-{index}")).await;
        let definition = activity_definition(&activity, json!({}));
        let machine =
            create_machine(&emulator, &format!("failing-machine-{index}"), &definition).await;
        let execution = start_execution(&emulator, &machine, "only", r#"{"n":1}"#).await;
        let (token, _) = claim(&emulator, case, &activity).await;

        let mut reply = json!({});
        let members = reply.as_object_mut().expect("the reply is an object");
        if let Some(error) = &error {
            members.insert("error".to_string(), json!(error));
        }
        if let Some(cause) = &cause {
            members.insert("cause".to_string(), json!(cause));
        }

        let (status, body) = send_failure(&emulator, &token, &reply).await;
        assert_eq!(
            (status, &body),
            (200, &json!({})),
            "case {case:?}: expected an empty 200 from the reply, got {status} with {body:?}"
        );

        // The outcome the reply made due is the execution's, carrying what the reply
        // sent and nothing of the emulator's own.
        std::thread::sleep(PAST_THE_EXECUTION_DELAY);
        let described = describe_execution(&emulator, &execution).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("FAILED"),
            "case {case:?}: expected the execution to have failed, got {described:?}"
        );
        assert_eq!(
            described.get("error").and_then(Value::as_str),
            error.as_deref(),
            "case {case:?}: expected the error {error:?}, got {:?}",
            described.get("error")
        );
        assert_eq!(
            described.get("cause").and_then(Value::as_str),
            cause.as_deref(),
            "case {case:?}: expected the cause {cause:?}, got {:?}",
            described.get("cause")
        );
        assert!(
            described.get("stopDate").is_some_and(|stop| !stop.is_null())
                && described.get("output").is_none_or(Value::is_null),
            "case {case:?}: a failed execution has a stopDate and no output, got {described:?}"
        );

        // The reply CLOSED the task, and a repeat of it answers for a task that is
        // gone — stating the one fixed reason this error class carries.
        let held = held_tasks(&emulator, &activity);
        assert_eq!(
            held, 0,
            "case {case:?}: expected the answered task to be closed, got {held} still held"
        );
        let (status, body) = send_failure(&emulator, &token, &reply).await;
        assert_eq!(
            (
                status,
                error_code(&body),
                body.get("message").and_then(Value::as_str)
            ),
            (
                400,
                "TaskTimedOut".to_string(),
                Some(task_timed_out_message().as_str())
            ),
            "case {case:?}: expected a repeat of the reply to be refused, got {status} with \
             {body:?}"
        );
    }
}

/// A failure reply appends its own two events to the ones the start and the claim
/// left, and the execution it ended is terminal — so it reports `REDRIVABLE`, and a
/// redrive runs it again and queues a task of its own.
#[tokio::test]
async fn a_failure_reply_appends_its_events_and_leaves_the_execution_redrivable() {
    let emulator = Emulator::new();
    let activity = create_activity(&emulator, "redriven").await;
    let definition = activity_definition(&activity, json!({}));
    let machine = create_machine(&emulator, "redriven-machine", &definition).await;
    let execution = start_execution(&emulator, &machine, "only", r#"{"n":1}"#).await;
    let (token, _) = claim(&emulator, "the claim", &activity).await;

    let (status, body) = send_failure(
        &emulator,
        &token,
        &json!({ "error": "WorkerError", "cause": "the worker gave up" }),
    )
    .await;
    assert_eq!(
        (status, &body),
        (200, &json!({})),
        "expected an empty 200 from the reply, got {status} with {body:?}"
    );
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    // The describe is what materialises whatever the clock had made due, so the
    // history is read after it.
    let described = describe_execution(&emulator, &execution).await;
    assert_eq!(
        (
            described.get("status").and_then(Value::as_str),
            described.get("redriveStatus").and_then(Value::as_str)
        ),
        (Some("FAILED"), Some("REDRIVABLE")),
        "expected a failed, redrivable execution, got {described:?}"
    );

    let events = history(&emulator, &execution);
    assert_eq!(
        events,
        vec![
            "ExecutionStarted",
            "TaskStateEntered",
            "ActivityScheduled",
            "ActivityStarted",
            "ActivityFailed",
            "ExecutionFailed",
        ],
        "expected the failure's events appended once, got {events:?}"
    );

    emulator
        .ok("RedriveExecution", &json!({ "executionArn": execution }))
        .await;
    let redriven = describe_execution(&emulator, &execution).await;
    assert_eq!(
        (
            redriven.get("status").and_then(Value::as_str),
            redriven.get("redriveCount").and_then(Value::as_i64)
        ),
        (Some("RUNNING"), Some(1)),
        "expected the redrive to run the execution again, got {redriven:?}"
    );
    let held = held_tasks(&emulator, &activity);
    assert_eq!(
        held, 1,
        "expected the re-run to queue one task, got {held} held"
    );
    let (fresh, _) = claim(&emulator, "after the redrive", &activity).await;
    assert_ne!(
        fresh, token,
        "expected the re-run's task to carry a token of its own, got {token:?} again"
    );
}

// ---------------------------------------------------------------------------
// Which tokens this reply cannot be applied to
// ---------------------------------------------------------------------------

/// A failure reply is refused for exactly the tokens a result reply is, with the same
/// modelled error and the same message: the two operations resolve one `taskToken` by
/// one rule.
#[tokio::test]
async fn a_failure_reply_is_refused_for_the_tokens_a_result_reply_is() {
    let emulator = Emulator::new();

    // A token minted for an execution that was then STOPPED: the task went with it.
    let stopped_activity = create_activity(&emulator, "stopped").await;
    let stopped_machine = create_machine(
        &emulator,
        "stopped-machine",
        &activity_definition(&stopped_activity, json!({})),
    )
    .await;
    let stopped = start_execution(&emulator, &stopped_machine, "only", "{}").await;
    let (stopped_token, _) = claim(&emulator, "the stopped execution", &stopped_activity).await;
    emulator
        .ok("StopExecution", &json!({ "executionArn": stopped }))
        .await;

    // A token of a task whose activity was deleted under it.
    let deleted_activity = create_activity(&emulator, "deleted").await;
    let deleted_machine = create_machine(
        &emulator,
        "deleted-machine",
        &activity_definition(&deleted_activity, json!({})),
    )
    .await;
    let orphaned = start_execution(&emulator, &deleted_machine, "only", "{}").await;
    let (orphaned_token, _) = claim(&emulator, "the orphaned task", &deleted_activity).await;
    emulator
        .ok("DeleteActivity", &json!({ "activityArn": deleted_activity }))
        .await;

    // Each row: the token, the modelled error it is refused with, and the message that
    // error carries.
    let cases: Vec<(&str, String, &str, String)> = vec![
        (
            "a token of a stopped execution, whose task can never be answered",
            stopped_token,
            "TaskTimedOut",
            task_timed_out_message(),
        ),
        (
            "a well-formed token no task was ever scheduled under",
            task_token(MINTED_ID, &stopped_activity),
            "TaskTimedOut",
            task_timed_out_message(),
        ),
        (
            "a token whose activity was deleted while the task was outstanding",
            orphaned_token,
            "TaskDoesNotExist",
            resource_error_message(ACTIVITY_DOES_NOT_EXIST, &deleted_activity),
        ),
        (
            "text that is no token at all",
            "not-a-token".to_string(),
            "InvalidToken",
            invalid_token_message(),
        ),
        (
            "a token whose id is not one this service mints",
            task_token("not-an-id", &stopped_activity),
            "InvalidToken",
            invalid_token_message(),
        ),
        (
            "a token naming a state machine rather than an activity",
            task_token(MINTED_ID, &stopped_machine),
            "InvalidToken",
            invalid_token_message(),
        ),
    ];
    for (case, token, expected_code, expected_message) in cases {
        let (status, body) = send_failure(&emulator, &token, &json!({ "error": "Nope" })).await;
        assert_eq!(
            (
                status,
                error_code(&body),
                body.get("message").and_then(Value::as_str)
            ),
            (
                400,
                expected_code.to_string(),
                Some(expected_message.as_str())
            ),
            "case {case:?}: expected a 400 {expected_code} saying {expected_message:?}, got \
             {status} with {body:?}"
        );
    }

    // The orphaned execution is untouched by the refusal: it is still waiting.
    let described = describe_execution(&emulator, &orphaned).await;
    assert_eq!(
        described.get("status").and_then(Value::as_str),
        Some("RUNNING"),
        "expected the orphaned execution to still be waiting, got {described:?}"
    );
}
