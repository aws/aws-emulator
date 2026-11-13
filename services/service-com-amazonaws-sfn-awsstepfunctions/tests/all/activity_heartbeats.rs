// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `SendTaskHeartbeat`: how a worker reports that the activity
//! task its `taskToken` names is still making progress, driven over this service's own
//! wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`.
//!
//! The rules under test:
//!
//! 1. a heartbeat RESETS the heartbeat clock of the task its token names: the window
//!    the scheduling state's `HeartbeatSeconds` opens runs again from the heartbeat, so
//!    a task heartbeaten inside each window stays outstanding and its execution stays
//!    `RUNNING` well past the point an unheartbeaten one would have been closed;
//! 2. that is the ONLY thing it changes: the task's whole `TimeoutSeconds` is its
//!    maximum allowed duration regardless of how many heartbeats were received, the
//!    execution is not continued, and the call writes no history event — so the same
//!    token still answers the task afterwards;
//! 3. a heartbeat is judged by the token before the store is consulted: a value that is
//!    not a token this service issued is `InvalidToken` (400), a token whose task is no
//!    longer outstanding — closed by the clock, already answered, or withdrawn with its
//!    execution — is `TaskTimedOut` (400), and one whose activity was deleted while the
//!    task was outstanding is `TaskDoesNotExist` (400).
//!
//! The harness (`Emulator`, `string`, `create_machine`, `start_execution`,
//! `describe_execution`) is the one `state_machine_lifecycle` builds; `tests/all.rs` is
//! this binary's crate root, so it is reached through
//! `crate::state_machine_lifecycle`.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::{
    asl::TIMEOUT_ERROR, task_timed_out_message, task_token,
};

use crate::state_machine_lifecycle::{
    create_machine, describe_execution, error_code, start_execution, string, Emulator, ACCOUNT,
    PAST_THE_EXECUTION_DELAY, REGION,
};

/// An id of the form this service mints its own ids in, for a token no task was ever
/// scheduled under.
const MINTED_ID: &str = "3c4d5e6f-7a8b-4c9d-8e1f-2a3b4c5d6e7f";

/// Create an activity that must succeed, and return its ARN.
async fn create_activity(emulator: &Emulator, name: &str) -> String {
    let created = emulator.ok("CreateActivity", &json!({ "name": name })).await;
    string(&created, "activityArn")
}

/// A one-`Task` definition whose state polls `activity_arn`, plus whatever other
/// members `extra` declares for that state.
fn activity_definition(activity_arn: &str, extra: Value) -> String {
    let Value::Object(extra) = extra else {
        panic!("expected an object of state members, got {extra:?}")
    };
    let mut state = json!({ "Type": "Task", "Resource": activity_arn, "End": true });
    let members = state.as_object_mut().expect("the state is an object");
    for (member, value) in extra {
        members.insert(member, value);
    }
    json!({ "StartAt": "Work", "States": { "Work": state } }).to_string()
}

/// Create an activity, a one-`Task` state machine around it carrying `bounds`, and
/// start one execution: report `(activityArn, executionArn)`.
async fn scheduled(
    emulator: &Emulator,
    label: &str,
    bounds: Value,
) -> (String, String) {
    let activity = create_activity(emulator, &format!("{label}-activity")).await;
    let definition = activity_definition(&activity, bounds);
    let machine = create_machine(emulator, &format!("{label}-machine"), &definition).await;
    let execution = start_execution(emulator, &machine, "only", r#"{"job":"work"}"#).await;
    (activity, execution)
}

/// Claim the one task of `activity_arn` and report its `taskToken`.
async fn claim(emulator: &Emulator, case: &str, activity_arn: &str) -> String {
    let answer = emulator
        .ok(
            "GetActivityTask",
            &json!({ "activityArn": activity_arn, "workerName": "w-1" }),
        )
        .await;
    answer
        .get("taskToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a taskToken, got {answer:?}"))
        .to_string()
}

/// Send one heartbeat under `token`, requiring a 200 and an empty answer.
async fn heartbeat(emulator: &Emulator, case: &str, token: &str) {
    let answered = emulator
        .ok("SendTaskHeartbeat", &json!({ "taskToken": token }))
        .await;
    assert!(
        answered.as_object().is_none_or(serde_json::Map::is_empty),
        "case {case:?}: expected an empty SendTaskHeartbeat answer, got {answered:?}"
    );
}

/// The status `execution_arn` reports right now.
async fn status(emulator: &Emulator, execution_arn: &str) -> String {
    string(&describe_execution(emulator, execution_arn).await, "status")
}

/// The event types one execution's stored history holds, in order.
///
/// Read from the STORE because what is asserted is that a heartbeat appends NOTHING,
/// which the record holds either way.
fn history(emulator: &Emulator, execution_arn: &str) -> Vec<String> {
    let state = emulator.store();
    let store = state.lock();
    store
        .executions
        .get(execution_arn)
        .map(|execution| {
            execution
                .history
                .iter()
                .map(|event| event.event_type.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// How many tasks the store holds for `activity_arn`.
fn held_tasks(emulator: &Emulator, activity_arn: &str) -> usize {
    let state = emulator.store();
    let store = state.lock();
    store
        .activity_tasks
        .values()
        .filter(|task| task.activity_arn.as_deref() == Some(activity_arn))
        .count()
}

// ---------------------------------------------------------------------------
// A heartbeat resets the heartbeat clock
// ---------------------------------------------------------------------------

/// A task heartbeaten inside each window stays outstanding past the instant an
/// unheartbeaten one would have been closed — over a table whose rows differ along the
/// `HeartbeatSeconds` the state declares, how often the worker beats and how many
/// beats it sends, with and without a far-off whole-task `TimeoutSeconds` beside it.
#[tokio::test]
async fn heartbeats_keep_a_task_and_its_execution_alive() {
    let emulator = Emulator::new();

    // Each row: the bounds the scheduling state declares, how long the worker waits
    // between beats, and how many beats it sends. Every row beats strictly inside its
    // own window and runs for LONGER than that window in total, so a run that reaches
    // the end still RUNNING can only have had its clock reset.
    let cases: Vec<(&str, Value, Duration, usize)> = vec![
        (
            "a one-second window, beaten twice",
            json!({ "HeartbeatSeconds": 1 }),
            Duration::from_millis(700),
            2,
        ),
        (
            "a one-second window beside a far-off TimeoutSeconds, beaten three times",
            json!({ "HeartbeatSeconds": 1, "TimeoutSeconds": 300 }),
            Duration::from_millis(700),
            3,
        ),
        (
            "a two-second window, beaten once",
            json!({ "HeartbeatSeconds": 2 }),
            Duration::from_millis(1_500),
            1,
        ),
    ];

    for (index, (case, bounds, interval, beats)) in cases.into_iter().enumerate() {
        let (activity, execution) =
            scheduled(&emulator, &format!("alive-{index}"), bounds).await;
        let token = claim(&emulator, case, &activity).await;

        for beat in 1..=beats {
            std::thread::sleep(interval);
            heartbeat(&emulator, case, &token).await;
            let reported = status(&emulator, &execution).await;
            assert_eq!(
                reported, "RUNNING",
                "case {case:?}: expected the execution to still wait at its task after beat \
                 {beat}, got {reported}"
            );
        }

        // One more window's worth of waiting, which the LAST beat has to have covered:
        // the same run with no heartbeat at all is closed by this point.
        std::thread::sleep(interval);
        let reported = status(&emulator, &execution).await;
        assert_eq!(
            reported, "RUNNING",
            "case {case:?}: expected the heartbeats to have reset the window, got {reported}"
        );
        let held = held_tasks(&emulator, &activity);
        assert_eq!(
            held, 1,
            "case {case:?}: expected the heartbeaten task to still be outstanding, got {held} held"
        );

        // The token still answers the task: the heartbeats closed nothing.
        emulator
            .ok(
                "SendTaskSuccess",
                &json!({ "taskToken": token, "output": r#"{"done":true}"# }),
            )
            .await;
        std::thread::sleep(PAST_THE_EXECUTION_DELAY);
        let reported = status(&emulator, &execution).await;
        assert_eq!(
            reported, "SUCCEEDED",
            "case {case:?}: expected the reply to complete the heartbeaten task, got {reported}"
        );
    }
}

/// The SAME run with no heartbeat at all is closed by its window: the control the
/// table above is read against.
#[tokio::test]
async fn an_unbeaten_task_is_closed_by_its_window() {
    let emulator = Emulator::new();
    let (activity, execution) = scheduled(
        &emulator,
        "unbeaten",
        json!({ "HeartbeatSeconds": 1 }),
    )
    .await;
    claim(&emulator, "the only claim", &activity).await;

    std::thread::sleep(Duration::from_millis(1_400));
    let described = describe_execution(&emulator, &execution).await;
    assert_eq!(
        (
            described.get("status").and_then(Value::as_str),
            described.get("error").and_then(Value::as_str),
        ),
        (Some("FAILED"), Some(TIMEOUT_ERROR)),
        "expected a task nobody beat to have been closed by its window, got {described:?}"
    );
}

/// Describe `execution_arn` until the clock has closed it, and report what it then
/// says.
///
/// Generous room: the deadline is far longer than any bound a test below declares, and
/// what ends the loop is the CONDITION — the execution having left `RUNNING` — not the
/// time spent waiting for it.
async fn closed_by_the_clock(emulator: &Emulator, execution_arn: &str) -> Value {
    /// Far longer than the bounds the tests here declare, so a loaded host settles
    /// inside it.
    const PATIENCE: Duration = Duration::from_secs(60);
    /// How long the poll pauses between looks.
    const PAUSE: Duration = Duration::from_millis(25);

    let began = ::std::time::Instant::now();
    loop {
        let described = describe_execution(emulator, execution_arn).await;
        if described.get("status").and_then(Value::as_str) != Some("RUNNING") {
            return described;
        }
        assert!(
            began.elapsed() < PATIENCE,
            "expected {execution_arn} to be closed by its bound, got {described:?}"
        );
        ::tokio::time::sleep(PAUSE).await;
    }
}

/// A heartbeat does NOT extend the task's whole `TimeoutSeconds`: that bound is its
/// maximum allowed duration regardless of how many heartbeats were received, so the
/// clock closes it on time and a beat that arrives afterwards is `TaskTimedOut`.
#[tokio::test]
async fn heartbeats_do_not_extend_the_whole_task_timeout() {
    let emulator = Emulator::new();
    let (activity, execution) = scheduled(
        &emulator,
        "capped",
        json!({ "HeartbeatSeconds": 1, "TimeoutSeconds": 2 }),
    )
    .await;
    let token = claim(&emulator, "the capped task", &activity).await;

    // Two beats inside the heartbeat window, which keep the heartbeat clock reset.
    for beat in 1..=2 {
        std::thread::sleep(Duration::from_millis(700));
        heartbeat(&emulator, "inside the window", &token).await;
        let reported = status(&emulator, &execution).await;
        assert_eq!(
            reported, "RUNNING",
            "expected beat {beat} to leave the execution running, got {reported}"
        );
    }

    // Past the whole-task timeout, which the beats could not move: the outcome is what
    // this waits FOR — looked for again until it holds, on a deadline far longer than the
    // two seconds the bound itself allows, so a loaded host reaches it too.
    let described = closed_by_the_clock(&emulator, &execution).await;
    assert_eq!(
        (
            described.get("status").and_then(Value::as_str),
            described.get("error").and_then(Value::as_str),
        ),
        (Some("FAILED"), Some(TIMEOUT_ERROR)),
        "expected the whole-task timeout to close the task anyway, got {described:?}"
    );

    let (http_status, body) = emulator
        .call("SendTaskHeartbeat", &json!({ "taskToken": token }))
        .await;
    let code = error_code(&body);
    assert_eq!(
        (http_status, code.as_str()),
        (400, "TaskTimedOut"),
        "expected a beat after the task was closed to be refused, got {http_status} {code}"
    );
    assert_eq!(
        body.get("message").and_then(Value::as_str),
        Some(task_timed_out_message().as_str()),
        "expected the refusal to state the reason of this error class, got {body:?}"
    );
    let held = held_tasks(&emulator, &activity);
    assert_eq!(
        held, 0,
        "expected the closed task to be gone, got {held} still held"
    );
}

/// A heartbeat writes no history event and continues nothing: "this action does not in
/// itself create an event in the execution history". Asserted for a task the state
/// bounded NO heartbeat window on as well, which a heartbeat still answers.
#[tokio::test]
async fn a_heartbeat_changes_nothing_but_the_clock() {
    let emulator = Emulator::new();

    // Each row: the bounds the scheduling state declares — a task with a window, and a
    // task with none at all, which a worker may still report progress for.
    let cases: Vec<(&str, Value)> = vec![
        ("a window the heartbeat resets", json!({ "HeartbeatSeconds": 60 })),
        ("no bound at all", json!({})),
        (
            "a whole-task timeout only",
            json!({ "TimeoutSeconds": 300 }),
        ),
    ];

    for (index, (case, bounds)) in cases.into_iter().enumerate() {
        let (activity, execution) =
            scheduled(&emulator, &format!("quiet-{index}"), bounds).await;
        let token = claim(&emulator, case, &activity).await;
        let before = history(&emulator, &execution);
        assert_eq!(
            before,
            vec![
                "ExecutionStarted",
                "TaskStateEntered",
                "ActivityScheduled",
                "ActivityStarted",
            ],
            "case {case:?}: expected the events the start and the claim wrote, got {before:?}"
        );

        // Three beats: however many arrive, the history and the task are unchanged.
        for _ in 0..3 {
            heartbeat(&emulator, case, &token).await;
        }
        let after = history(&emulator, &execution);
        assert_eq!(
            after, before,
            "case {case:?}: expected the heartbeats to write no event, got {after:?}"
        );
        let reported = status(&emulator, &execution).await;
        assert_eq!(
            reported, "RUNNING",
            "case {case:?}: expected the execution to be continued by nothing, got {reported}"
        );
        let held = held_tasks(&emulator, &activity);
        assert_eq!(
            held, 1,
            "case {case:?}: expected the task to stay outstanding, got {held} held"
        );

        // And the task is still answerable under the same token.
        emulator
            .ok(
                "SendTaskSuccess",
                &json!({ "taskToken": token, "output": r#"{"ok":true}"# }),
            )
            .await;
        std::thread::sleep(PAST_THE_EXECUTION_DELAY);
        let reported = status(&emulator, &execution).await;
        assert_eq!(
            reported, "SUCCEEDED",
            "case {case:?}: expected the beaten task to still complete, got {reported}"
        );
    }
}

// ---------------------------------------------------------------------------
// What a heartbeat is refused for
// ---------------------------------------------------------------------------

/// A value that is not a token this service mints is `InvalidToken`, judged before the
/// store is consulted — over a table whose rows differ along WHY the value is not one.
#[tokio::test]
async fn a_value_that_is_not_a_task_token_is_refused() {
    let emulator = Emulator::new();
    let activity_arn = format!("arn:aws:states:{REGION}:{ACCOUNT}:activity:a");

    // Each row: a value inside the modelled `TaskToken` length (1..2048) that is not a
    // token this service mints.
    let cases: Vec<(&str, String)> = vec![
        ("text that is no token at all", "not-a-token".to_string()),
        ("an ARN, which is no token either", activity_arn.clone()),
        ("the id alone, naming no activity", MINTED_ID.to_string()),
        ("a token carrying no id at all", task_token("", &activity_arn)),
        (
            "a token whose id is not one this service mints",
            task_token("not-an-id", &activity_arn),
        ),
        (
            "a token naming no ARN of this service",
            task_token(MINTED_ID, "not-an-arn"),
        ),
        (
            "a token naming a state machine rather than an activity",
            task_token(
                MINTED_ID,
                &format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:a"),
            ),
        ),
        ("a single character", "x".to_string()),
    ];
    for (case, token) in cases {
        let (http_status, code) = emulator
            .err("SendTaskHeartbeat", &json!({ "taskToken": token }))
            .await;
        assert_eq!(
            (http_status, code.as_str()),
            (400, "InvalidToken"),
            "case {case:?}: expected a 400 InvalidToken for {token:?}, got {http_status} {code}"
        );
    }
}

/// A token whose task is no longer outstanding is `TaskTimedOut`, and one whose
/// ACTIVITY was deleted while the task was outstanding is `TaskDoesNotExist`.
#[tokio::test]
async fn a_token_whose_task_is_gone_is_refused() {
    let emulator = Emulator::new();

    // A token of a task whose execution was STOPPED: the task went with it.
    let (stopped_activity, stopped) = scheduled(&emulator, "stopped", json!({})).await;
    let stopped_token = claim(&emulator, "the stopped execution", &stopped_activity).await;
    emulator
        .ok("StopExecution", &json!({ "executionArn": stopped }))
        .await;

    // A token of a task that has already been ANSWERED.
    let (answered_activity, _) = scheduled(&emulator, "answered", json!({})).await;
    let answered_token = claim(&emulator, "the answered task", &answered_activity).await;
    emulator
        .ok(
            "SendTaskSuccess",
            &json!({ "taskToken": answered_token, "output": "{}" }),
        )
        .await;

    // A token of a task whose activity was deleted under it.
    let (deleted_activity, orphaned) = scheduled(&emulator, "deleted", json!({})).await;
    let orphaned_token = claim(&emulator, "the orphaned task", &deleted_activity).await;
    emulator
        .ok(
            "DeleteActivity",
            &json!({ "activityArn": deleted_activity }),
        )
        .await;

    // Each row: the token, and the modelled error a heartbeat under it carries.
    let cases: Vec<(&str, String, &str)> = vec![
        (
            "a token of a stopped execution, whose task can never be answered",
            stopped_token,
            "TaskTimedOut",
        ),
        (
            "a token of a task that has already been answered",
            answered_token,
            "TaskTimedOut",
        ),
        (
            "a well-formed token no task was ever scheduled under",
            task_token(MINTED_ID, &stopped_activity),
            "TaskTimedOut",
        ),
        (
            "a token whose activity was deleted while the task was outstanding",
            orphaned_token,
            "TaskDoesNotExist",
        ),
    ];
    for (case, token, expected) in cases {
        let (http_status, code) = emulator
            .err("SendTaskHeartbeat", &json!({ "taskToken": token }))
            .await;
        assert_eq!(
            (http_status, code.as_str()),
            (400, expected),
            "case {case:?}: expected a 400 {expected}, got {http_status} {code}"
        );
    }

    // The orphaned execution is untouched by the refusal: it is still waiting.
    let reported = status(&emulator, &orphaned).await;
    assert_eq!(
        reported, "RUNNING",
        "expected the orphaned execution to still be waiting, got {reported}"
    );
}
