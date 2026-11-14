// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a worker's REPLY to an activity task: `SendTaskSuccess`,
//! driven over this service's own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`.
//!
//! The rules under test:
//!
//! 1. a reply to the task a `taskToken` names is that task's RESULT: the execution
//!    waiting at the state that scheduled it continues from there — the state's own
//!    result-side data flow (`ResultSelector`, `ResultPath`, `OutputPath`) is applied
//!    to the `output` the worker sent, the walk goes on to the state's `Next`, and the
//!    outcome it reaches is the execution's, owed to the clock exactly as a start's is;
//! 2. the reply CLOSES the task: one token answers one task, so a second reply under
//!    the same token resolves nothing;
//! 3. the reply is judged before anything is written: an `output` that is not JSON is
//!    `InvalidOutput` (400) and a value that is not a token this service issued is
//!    `InvalidToken` (400), and neither refusal touches the task or its execution;
//! 4. a token whose task is no longer outstanding — answered, or closed with the
//!    execution it belonged to — is `TaskTimedOut` (400), and one whose activity was
//!    deleted while the task was outstanding is `TaskDoesNotExist` (400).
//!
//! The harness (`Emulator`, `string`, `create_machine`, `start_execution`,
//! `describe_execution`) is the one `state_machine_lifecycle` builds; `tests/all.rs`
//! is this binary's crate root, so it is reached through
//! `crate::state_machine_lifecycle`. The helpers below that are `pub(crate)` are the
//! ones `send_task_failure` — the OTHER reply to a task — drives the same situations
//! through, rather than carrying a second copy of them.

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::{task_timed_out_message, task_token};

use crate::state_machine_lifecycle::{
    create_machine, describe_execution, error_code, start_execution, string, Emulator, ACCOUNT,
    PAST_THE_EXECUTION_DELAY, REGION,
};

/// An id of the form this service mints its own ids in, for a token no task was ever
/// scheduled under.
pub(crate) const MINTED_ID: &str = "7f1b2c3d-4e5f-4a6b-8c9d-0e1f2a3b4c5d";

/// Create an activity that must succeed, and return its ARN.
pub(crate) async fn create_activity(emulator: &Emulator, name: &str) -> String {
    let created = emulator.ok("CreateActivity", &json!({ "name": name })).await;
    string(&created, "activityArn")
}

/// A one-`Task` definition whose state polls `activity_arn`, plus whatever other
/// members `extra` declares for that state.
pub(crate) fn activity_definition(activity_arn: &str, extra: Value) -> String {
    definition_with_states(activity_arn, extra, json!({}))
}

/// A definition whose `Work` state is the activity task — carrying `extra` — followed
/// by whatever further states `rest` declares.
fn definition_with_states(activity_arn: &str, extra: Value, rest: Value) -> String {
    let (Value::Object(extra), Value::Object(rest)) = (extra, rest) else {
        panic!("expected objects of members and of states")
    };
    let mut work = json!({ "Type": "Task", "Resource": activity_arn, "End": true });
    let members = work.as_object_mut().expect("the state is an object");
    for (member, value) in extra {
        // A state that transitions on carries `Next` instead of `"End": true`.
        if member == "Next" {
            members.remove("End");
        }
        members.insert(member, value);
    }
    let mut states = json!({ "Work": work });
    let declared = states.as_object_mut().expect("the states are an object");
    for (name, state) in rest {
        declared.insert(name, state);
    }
    json!({ "StartAt": "Work", "States": states }).to_string()
}

/// Claim the one task of `activity_arn`, and report `(taskToken, input)`.
pub(crate) async fn claim(emulator: &Emulator, case: &str, activity_arn: &str) -> (String, Value) {
    let answer = emulator
        .ok(
            "GetActivityTask",
            &json!({ "activityArn": activity_arn, "workerName": "w-1" }),
        )
        .await;
    let token = answer
        .get("taskToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a taskToken, got {answer:?}"))
        .to_string();
    let input: Value = answer
        .get("input")
        .and_then(Value::as_str)
        .map(|text| {
            serde_json::from_str(text)
                .unwrap_or_else(|error| panic!("case {case:?}: expected JSON input, got {error}"))
        })
        .unwrap_or_else(|| panic!("case {case:?}: expected an input, got {answer:?}"));
    (token, input)
}

/// Reply to `token` with `output`, requiring a 200.
async fn send_success(emulator: &Emulator, token: &str, output: &str) -> Value {
    emulator
        .ok(
            "SendTaskSuccess",
            &json!({ "taskToken": token, "output": output }),
        )
        .await
}

/// The `(status, output)` an execution reports once whatever the clock owed is due.
async fn settled(emulator: &Emulator, execution_arn: &str) -> (String, Option<Value>) {
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);
    let described = describe_execution(emulator, execution_arn).await;
    let status = string(&described, "status");
    let output = described
        .get("output")
        .and_then(Value::as_str)
        .map(|text| serde_json::from_str(text).expect("a reported output is JSON"));
    (status, output)
}

/// The event types one execution's stored history holds, in order.
///
/// Read from the STORE rather than through `GetExecutionHistory`: the rule under test
/// is that a reply APPENDS its events to the ones the states before the task left,
/// which is what the record holds either way.
pub(crate) fn history(emulator: &Emulator, execution_arn: &str) -> Vec<String> {
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
pub(crate) fn held_tasks(emulator: &Emulator, activity_arn: &str) -> usize {
    let state = emulator.store();
    let store = state.lock();
    store
        .activity_tasks
        .values()
        .filter(|task| task.activity_arn.as_deref() == Some(activity_arn))
        .count()
}

// ---------------------------------------------------------------------------
// What a reply does to the execution waiting at the task
// ---------------------------------------------------------------------------

/// A reply is the task state's RESULT: the state's result-side data flow is applied to
/// it and the execution continues to its own outcome — over a table whose rows differ
/// along the output document's type, shape and size and along the members the state
/// places that result through.
#[tokio::test]
async fn send_task_success_continues_the_execution_with_the_reported_result() {
    let emulator = Emulator::new();

    // Each row: the execution input, the extra members of the `Task` state, any
    // further states, the `output` the worker reports, and the document the execution
    // must then report.
    let blob = "x".repeat(8_192);
    let blob_output = format!(r#"{{"blob":"{blob}"}}"#);
    let cases: Vec<(&str, &str, Value, Value, &str, Value)> = vec![
        (
            "the empty object, which is the smallest JSON document a task may report",
            "{}",
            json!({}),
            json!({}),
            "{}",
            json!({}),
        ),
        (
            "an object, which becomes the execution's output unchanged",
            r#"{"order":"A-1"}"#,
            json!({}),
            json!({}),
            r#"{"done":true,"items":2}"#,
            json!({ "done": true, "items": 2 }),
        ),
        (
            "an array, which is a JSON document like any other",
            "{}",
            json!({}),
            json!({}),
            "[1,2,3]",
            json!([1, 2, 3]),
        ),
        (
            "a bare string, which is a JSON document too",
            "{}",
            json!({}),
            json!({}),
            r#""done""#,
            json!("done"),
        ),
        (
            "a bare number",
            "{}",
            json!({}),
            json!({}),
            "17",
            json!(17),
        ),
        (
            "null, which the language carries as a value",
            "{}",
            json!({}),
            json!({}),
            "null",
            Value::Null,
        ),
        (
            "characters outside ASCII, which the payload carries verbatim",
            "{}",
            json!({}),
            json!({}),
            r#"{"who":"wörker-日本"}"#,
            json!({ "who": "wörker-日本" }),
        ),
        (
            "a payload larger than one request's worth of members",
            "{}",
            json!({}),
            json!({}),
            &blob_output,
            json!({ "blob": blob }),
        ),
        (
            "ResultSelector, which BUILDS the effective result from the reply",
            "{}",
            json!({ "ResultSelector": { "id.$": "$.ticket", "fixed": 1 } }),
            json!({}),
            r#"{"ticket":"T-9","ignored":true}"#,
            json!({ "id": "T-9", "fixed": 1 }),
        ),
        (
            "ResultPath, which combines the result with the state's RAW input",
            r#"{"order":"A-2"}"#,
            json!({ "ResultPath": "$.result" }),
            json!({}),
            r#"{"done":true}"#,
            json!({ "order": "A-2", "result": { "done": true } }),
        ),
        (
            "ResultPath null, which discards the result and keeps the raw input",
            r#"{"order":"A-3"}"#,
            json!({ "ResultPath": null }),
            json!({}),
            r#"{"ignored":true}"#,
            json!({ "order": "A-3" }),
        ),
        (
            "OutputPath, which selects the part of the output the state reports",
            "{}",
            json!({ "OutputPath": "$.keep" }),
            json!({}),
            r#"{"keep":{"id":7},"drop":"everything else"}"#,
            json!({ "id": 7 }),
        ),
        (
            "ResultSelector, ResultPath and OutputPath together, in that order",
            r#"{"order":"A-4"}"#,
            json!({
                "ResultSelector": { "id.$": "$.ticket" },
                "ResultPath": "$.result",
                "OutputPath": "$.result",
            }),
            json!({}),
            r#"{"ticket":"T-1"}"#,
            json!({ "id": "T-1" }),
        ),
        (
            "InputPath, which the reply is placed against: ResultPath reads the RAW input",
            r#"{"keep":{"order":"A-5"},"drop":1}"#,
            json!({ "InputPath": "$.keep", "ResultPath": "$.result" }),
            json!({}),
            r#"{"done":true}"#,
            json!({
                "keep": { "order": "A-5" },
                "drop": 1,
                "result": { "done": true },
            }),
        ),
        (
            "Next, which the walk carries on to with the state's output",
            "{}",
            json!({ "Next": "After" }),
            json!({
                "After": {
                    "Type": "Pass",
                    "Parameters": { "seen.$": "$.ticket" },
                    "End": true,
                },
            }),
            r#"{"ticket":"T-2"}"#,
            json!({ "seen": "T-2" }),
        ),
        (
            "a Next chain ending at a Succeed state",
            "{}",
            json!({ "Next": "Check" }),
            json!({
                "Check": {
                    "Type": "Choice",
                    "Choices": [{ "Variable": "$.ok", "BooleanEquals": true, "Next": "Done" }],
                    "Default": "Done",
                },
                "Done": { "Type": "Succeed" },
            }),
            r#"{"ok":true}"#,
            json!({ "ok": true }),
        ),
    ];

    for (index, (case, input, extra, rest, output, expected)) in cases.into_iter().enumerate() {
        let activity = create_activity(&emulator, &format!("worker-{index}")).await;
        let definition = definition_with_states(&activity, extra, rest);
        let machine = create_machine(&emulator, &format!("machine-{index}"), &definition).await;
        let execution = start_execution(&emulator, &machine, "only", input).await;
        let (token, _) = claim(&emulator, case, &activity).await;

        let answered = send_success(&emulator, &token, output).await;
        assert!(
            answered.as_object().is_none_or(serde_json::Map::is_empty),
            "case {case:?}: expected an empty SendTaskSuccess answer, got {answered:?}"
        );

        let (status, reported) = settled(&emulator, &execution).await;
        assert_eq!(
            status, "SUCCEEDED",
            "case {case:?}: expected the execution to succeed, got {status}"
        );
        assert_eq!(
            reported.clone(),
            Some(expected.clone()),
            "case {case:?}: expected the output {expected}, got {reported:?}"
        );
        // The reply CLOSED the task: nothing is left for a later poll or reply.
        let held = held_tasks(&emulator, &activity);
        assert_eq!(
            held, 0,
            "case {case:?}: expected the answered task to be closed, got {held} still held"
        );
    }
}

/// The reply's events are APPENDED to the ones the states before the task left: the
/// history runs from the start through the claim to the reply and the outcome, with no
/// event written twice.
#[tokio::test]
async fn a_reply_appends_its_events_to_the_history_the_task_left() {
    let emulator = Emulator::new();
    let activity = create_activity(&emulator, "historian").await;
    let definition = activity_definition(&activity, json!({}));
    let machine = create_machine(&emulator, "history-machine", &definition).await;
    let execution = start_execution(&emulator, &machine, "only", r#"{"n":1}"#).await;

    let (token, _) = claim(&emulator, "the claim", &activity).await;
    let before = history(&emulator, &execution);
    assert_eq!(
        before,
        vec![
            "ExecutionStarted",
            "TaskStateEntered",
            "ActivityScheduled",
            "ActivityStarted",
        ],
        "expected the events the start and the claim wrote, got {before:?}"
    );

    send_success(&emulator, &token, r#"{"done":true}"#).await;
    let (status, _) = settled(&emulator, &execution).await;
    assert_eq!(
        status, "SUCCEEDED",
        "expected the reply to drive the execution to its outcome, got {status}"
    );

    let after = history(&emulator, &execution);
    assert_eq!(
        after,
        vec![
            "ExecutionStarted",
            "TaskStateEntered",
            "ActivityScheduled",
            "ActivityStarted",
            "ActivitySucceeded",
            "TaskStateExited",
            "ExecutionSucceeded",
        ],
        "expected the reply's events appended once, got {after:?}"
    );
    let ids: Vec<i64> = {
        let state = emulator.store();
        let store = state.lock();
        store
            .executions
            .get(&execution)
            .map(|record| record.history.iter().map(|event| event.id).collect())
            .unwrap_or_default()
    };
    let expected: Vec<i64> = (1..=i64::try_from(after.len()).expect("a short history")).collect();
    assert_eq!(
        ids, expected,
        "expected the appended events to continue the numbering, got {ids:?}"
    );
}

/// A reply to a task whose state transitions to ANOTHER activity task schedules that
/// next task, carrying the first one's output as its input — the round trip a chain of
/// workers makes.
#[tokio::test]
async fn a_reply_schedules_the_next_task_in_the_chain() {
    let emulator = Emulator::new();
    let first = create_activity(&emulator, "first-step").await;
    let second = create_activity(&emulator, "second-step").await;
    let definition = definition_with_states(
        &first,
        json!({ "Next": "Second" }),
        json!({
            "Second": { "Type": "Task", "Resource": second, "End": true },
        }),
    );
    let machine = create_machine(&emulator, "chain-machine", &definition).await;
    let execution = start_execution(&emulator, &machine, "only", r#"{"step":0}"#).await;

    let (first_token, first_input) = claim(&emulator, "the first task", &first).await;
    assert_eq!(
        first_input,
        json!({ "step": 0 }),
        "expected the execution input at the first task, got {first_input}"
    );
    send_success(&emulator, &first_token, r#"{"step":1}"#).await;

    // The execution is still RUNNING: it is waiting at the SECOND task now.
    let (status, _) = settled(&emulator, &execution).await;
    assert_eq!(
        status, "RUNNING",
        "expected the execution to wait at the next task, got {status}"
    );
    let (second_token, second_input) = claim(&emulator, "the second task", &second).await;
    assert_eq!(
        second_input,
        json!({ "step": 1 }),
        "expected the first reply's output as the second task's input, got {second_input}"
    );
    assert_ne!(
        second_token, first_token,
        "expected a token of its own for the second task, got {first_token} twice"
    );

    send_success(&emulator, &second_token, r#"{"step":2}"#).await;
    let (status, reported) = settled(&emulator, &execution).await;
    assert_eq!(
        (status.as_str(), reported.clone()),
        ("SUCCEEDED", Some(json!({ "step": 2 }))),
        "expected the second reply to end the execution, got {status} with {reported:?}"
    );
}

// ---------------------------------------------------------------------------
// One token answers one task
// ---------------------------------------------------------------------------

/// A token answers its task ONCE: a second reply is refused with `TaskTimedOut` and
/// changes nothing about the execution the first one completed.
#[tokio::test]
async fn a_token_answers_its_task_once() {
    let emulator = Emulator::new();
    let activity = create_activity(&emulator, "once").await;
    let definition = activity_definition(&activity, json!({}));
    let machine = create_machine(&emulator, "once-machine", &definition).await;
    let execution = start_execution(&emulator, &machine, "only", "{}").await;
    let (token, _) = claim(&emulator, "the only claim", &activity).await;

    send_success(&emulator, &token, r#"{"attempt":1}"#).await;
    let (status, reported) = settled(&emulator, &execution).await;
    assert_eq!(
        (status.as_str(), reported.clone()),
        ("SUCCEEDED", Some(json!({ "attempt": 1 }))),
        "expected the first reply to complete the task, got {status} with {reported:?}"
    );

    let (code_status, body) = emulator
        .call(
            "SendTaskSuccess",
            &json!({ "taskToken": token, "output": r#"{"attempt":2}"# }),
        )
        .await;
    let code = error_code(&body);
    assert_eq!(
        (code_status, code.as_str()),
        (400, "TaskTimedOut"),
        "expected a reused token to be refused, got {code_status} {code}"
    );
    // The refusal names the ACTIVITY the closed task was handed out by, and nothing of
    // the request or of the store.
    assert_eq!(
        body.get("message").and_then(Value::as_str),
        Some(task_timed_out_message(&token).as_str()),
        "expected the refusal to name the activity of this closed task, got {body:?}"
    );
    let (status, reported) = settled(&emulator, &execution).await;
    assert_eq!(
        (status.as_str(), reported.clone()),
        ("SUCCEEDED", Some(json!({ "attempt": 1 }))),
        "expected the refused reply to change nothing, got {status} with {reported:?}"
    );
}

/// A token whose task is no longer outstanding is `TaskTimedOut`, and one whose
/// ACTIVITY was deleted while the task was outstanding is `TaskDoesNotExist`.
#[tokio::test]
async fn a_token_whose_task_is_gone_is_refused() {
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
        .ok(
            "DeleteActivity",
            &json!({ "activityArn": deleted_activity }),
        )
        .await;

    // Each row: the token, and the modelled error the reply to it carries.
    let cases: Vec<(&str, String, &str)> = vec![
        (
            "a token of a stopped execution, whose task can never be answered",
            stopped_token,
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
        let (status, code) = emulator
            .err(
                "SendTaskSuccess",
                &json!({ "taskToken": token, "output": "{}" }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, expected),
            "case {case:?}: expected a 400 {expected}, got {status} {code}"
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

// ---------------------------------------------------------------------------
// What a refused reply leaves behind
// ---------------------------------------------------------------------------

/// A value that is not a token this service mints is `InvalidToken`, judged before the
/// store is consulted — over a table whose rows differ along WHY the value is not one.
#[tokio::test]
async fn a_value_that_is_not_a_task_token_is_refused() {
    let emulator = Emulator::new();

    // Each row: a value inside the modelled `TaskToken` length (1..2048) that is not a
    // token this service mints, and WHY it is not one.
    let cases: Vec<(&str, String)> = vec![
        ("text that is no token at all", "not-a-token".to_string()),
        (
            "an ARN, which is no token either",
            format!("arn:aws:states:{REGION}:{ACCOUNT}:activity:a"),
        ),
        (
            "the id alone, naming no activity",
            MINTED_ID.to_string(),
        ),
        (
            "a token carrying no id at all",
            task_token("", &format!("arn:aws:states:{REGION}:{ACCOUNT}:activity:a")),
        ),
        (
            "a token whose id is not one this service mints",
            task_token(
                "not-an-id",
                &format!("arn:aws:states:{REGION}:{ACCOUNT}:activity:a"),
            ),
        ),
        (
            "a token whose id is in upper case",
            task_token(
                "7F1B2C3D-4E5F-4A6B-8C9D-0E1F2A3B4C5D",
                &format!("arn:aws:states:{REGION}:{ACCOUNT}:activity:a"),
            ),
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
        let (status, code) = emulator
            .err(
                "SendTaskSuccess",
                &json!({ "taskToken": token, "output": "{}" }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidToken"),
            "case {case:?}: expected a 400 InvalidToken for {token:?}, got {status} {code}"
        );
    }
}

/// An `output` that is not JSON is `InvalidOutput`, and the refusal is
/// NON-DESTRUCTIVE: the execution is still running, the task is still outstanding, and
/// a corrected reply under the same token still completes it.
#[tokio::test]
async fn a_reply_that_is_not_json_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    let activity = create_activity(&emulator, "picky").await;
    let definition = activity_definition(&activity, json!({}));
    let machine = create_machine(&emulator, "picky-machine", &definition).await;
    let execution = start_execution(&emulator, &machine, "only", "{}").await;
    let (token, _) = claim(&emulator, "the only claim", &activity).await;

    // Each row: an `output` inside the modelled length (0..262144) that is not a JSON
    // document.
    let cases: Vec<(&str, &str)> = vec![
        ("the empty string, which is no document at all", ""),
        ("text that is not JSON", "not-valid-json{{{"),
        ("a truncated object", r#"{"done":"#),
        ("a truncated array", "[1,2"),
        ("an unquoted word", "done"),
        ("two documents in one payload", "{} {}"),
        ("a single quote, which JSON does not use", "'done'"),
    ];
    for (case, output) in cases {
        let (status, code) = emulator
            .err(
                "SendTaskSuccess",
                &json!({ "taskToken": token, "output": output }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidOutput"),
            "case {case:?}: expected a 400 InvalidOutput for {output:?}, got {status} {code}"
        );
        let described = describe_execution(&emulator, &execution).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("RUNNING"),
            "case {case:?}: expected the refusal to leave the execution running, got {described:?}"
        );
        let held = held_tasks(&emulator, &activity);
        assert_eq!(
            held, 1,
            "case {case:?}: expected the task to stay outstanding, got {held} held"
        );
        // The token the refusal was sent under still answers that task, which is
        // what the worker holding it observes: its progress is acknowledged rather
        // than refused for a task the reply closed.
        let answered = emulator
            .ok("SendTaskHeartbeat", &json!({ "taskToken": token }))
            .await;
        assert_eq!(
            answered,
            json!({}),
            "case {case:?}: expected the token to still answer its task, got {answered:?}"
        );
    }

    // The same token still answers the task once the worker sends a document.
    send_success(&emulator, &token, r#"{"done":true}"#).await;
    let (status, reported) = settled(&emulator, &execution).await;
    assert_eq!(
        (status.as_str(), reported.clone()),
        ("SUCCEEDED", Some(json!({ "done": true }))),
        "expected the corrected reply to complete the task, got {status} with {reported:?}"
    );
}

