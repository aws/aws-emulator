// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the activity task a `Parallel` BRANCH had already handed to a
//! worker when a SIBLING unit's failure stopped it, driven over this service's own wire
//! protocol (`awsJson1_0`, `POST /` with `X-Amz-Target: AWSStepFunctions.<Operation>`)
//! through the production `MultiServiceRouter`.
//!
//! RULE: a `Task` state that names an activity SCHEDULES its task the moment the state is
//! entered, so a task scheduled before the failure of a sibling unit reached it was really
//! handed over: the next poll of that activity is handed its `taskToken` and the state's
//! input, the execution reports the state as ABORTED rather than as one that scheduled
//! nothing, and the token answers nothing — the work it carries was stopped with the state
//! that asked for it. The execution's own outcome is the failing unit's, which is owed to
//! the clock at the instant the handed-over work was stopped, so the task is claimable
//! until then.
//!
//! The rows differ along what the rule generalises over: WHAT fails beside the hand-over (a
//! `Fail` state, and a `Map` whose iteration misevaluates a JSONata field two levels down,
//! which is the shape every batch-of-worker-steps workflow has), WHERE the failing unit is
//! listed relative to the branch that hands over (before it, and after it), and HOW DEEP
//! the hand-over is nested (a `Parallel` branch, and a `Parallel` inside a `Parallel`).
//!
//! The harness (`Emulator`, `create_machine`, `start_execution`, `describe_execution`,
//! `string`) is the one `state_machine_lifecycle` builds and the activity helpers are
//! `activity_tasks`'; `tests/all/main.rs` picks every suite of this target up, so each is
//! reached through `crate::<suite>`.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::activity_tasks::{activity_arn, create_activity};
use crate::execution_history::history;
use crate::state_machine_lifecycle::{
    create_machine, describe_execution, start_execution, string, Emulator,
};

/// How long a poll waits for the status the clock owes before giving up.
///
/// Generous: what the loop waits for is the CONDITION, so no row turns on how fast this
/// host runs.
const PATIENCE: Duration = Duration::from_secs(90);

/// How long one poll pauses between two looks.
const POLL_PAUSE: Duration = Duration::from_millis(25);

/// The error and cause the `Fail` state of one row's failing branch declares.
const SIBLING_ERROR: &str = "SiblingBoom";
const SIBLING_CAUSE: &str = "the sibling branch failed on purpose";

/// The error the language names a JSONata expression that misevaluated with
/// ("States.QueryEvaluationError — the JSONata expression evaluation failed").
const QUERY_EVALUATION_ERROR: &str = "States.QueryEvaluationError";

/// What the service refuses a `SendTask*` quoting the token of a task that has already been
/// closed with: "the task token has either expired or the task associated with the token has
/// already been closed".
const TASK_TIMED_OUT: &str = "TaskTimedOut";

/// The events a hand-over the sibling's failure stopped records, as `HistoryEventType`
/// models them.
const ACTIVITY_SCHEDULED: &str = "ActivityScheduled";
const ACTIVITY_STARTED: &str = "ActivityStarted";
const TASK_STATE_ABORTED: &str = "TaskStateAborted";

/// The branch that hands its work to a worker: one `Task` state naming `activity`.
fn handing_over(activity: &str) -> Value {
    json!({
        "StartAt": "Waits",
        "States": { "Waits": { "Type": "Task", "Resource": activity, "End": true } },
    })
}

/// That same branch `levels` deep: the hand-over itself at one level, and a `Parallel` of
/// its own around it at two.
fn handing_over_branch(activity: &str, levels: usize) -> Value {
    let machine = handing_over(activity);
    if levels <= 1 {
        return machine;
    }
    json!({
        "StartAt": "Nested",
        "States": {
            "Nested": { "Type": "Parallel", "Branches": [machine], "End": true }
        },
    })
}

/// A branch that fails at once, at its own `Fail` state.
fn failing_branch() -> Value {
    json!({
        "StartAt": "Bang",
        "States": {
            "Bang": { "Type": "Fail", "Error": SIBLING_ERROR, "Cause": SIBLING_CAUSE }
        },
    })
}

/// A branch whose `Map` runs one item through an `ItemProcessor` whose `Task` state
/// misevaluates its `Arguments`, so the failure is two levels below the `Parallel`.
fn failing_map_branch() -> Value {
    json!({
        "StartAt": "Each",
        "States": {
            "Each": {
                "Type": "Map",
                "Items": [1],
                "ItemProcessor": {
                    "StartAt": "BadTask",
                    "States": {
                        "BadTask": {
                            "Type": "Task",
                            "Resource": "arn:aws:states:::lambda:invoke",
                            "Arguments": { "Payload": "{% $states.input.absent %}" },
                            "End": true,
                        }
                    }
                },
                "End": true,
            }
        },
    })
}

/// What fails beside the hand-over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fails {
    /// A `Fail` state in the sibling branch.
    AtOnce,
    /// A `Map` whose one iteration misevaluates a JSONata field.
    MisevaluatingMap,
}

/// One row: where the two branches sit, what fails, how deep the hand-over is, and the
/// error the execution must end with.
struct Row {
    case: &'static str,
    fails: Fails,
    /// Whether the branch that HANDS OVER is listed before the one that fails.
    handing_over_first: bool,
    /// How many `Parallel` levels the handing-over state is nested inside.
    levels: usize,
    error: &'static str,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the hand-over is listed first and a sibling Fail state fails",
            fails: Fails::AtOnce,
            handing_over_first: true,
            levels: 1,
            error: SIBLING_ERROR,
        },
        Row {
            case: "the failing branch is listed first, so the hand-over is reached after it",
            fails: Fails::AtOnce,
            handing_over_first: false,
            levels: 1,
            error: SIBLING_ERROR,
        },
        Row {
            case: "a Map two levels down misevaluates a JSONata field beside the hand-over",
            fails: Fails::MisevaluatingMap,
            handing_over_first: true,
            levels: 1,
            error: QUERY_EVALUATION_ERROR,
        },
        Row {
            case: "the hand-over is a Parallel inside a Parallel, two levels down",
            fails: Fails::MisevaluatingMap,
            handing_over_first: false,
            levels: 2,
            error: QUERY_EVALUATION_ERROR,
        },
    ]
}

/// The definition a row runs: a `Parallel` over the branch that hands work to a worker and
/// the branch that fails.
fn definition(activity: &str, row: &Row) -> String {
    let handed = handing_over_branch(activity, row.levels);
    let failing = match row.fails {
        Fails::AtOnce => failing_branch(),
        Fails::MisevaluatingMap => failing_map_branch(),
    };
    let branches: Vec<Value> = match row.handing_over_first {
        true => vec![handed, failing],
        false => vec![failing, handed],
    };
    json!({
        "QueryLanguage": "JSONata",
        "StartAt": "Fan",
        "States": { "Fan": { "Type": "Parallel", "Branches": branches, "End": true } },
    })
    .to_string()
}

/// Claim the task `activity` has for a worker: `GetActivityTask` is itself a long poll, so
/// one call waits for the task to arrive and answers `(the token, the input)`.
async fn claim(emulator: &Emulator, case: &str, activity: &str) -> (String, Value) {
    let answered = emulator
        .ok("GetActivityTask", &json!({ "activityArn": activity }))
        .await;
    let token = answered
        .get("taskToken")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a task on {activity}, got {answered:?}")
        })
        .to_string();
    let input: Value = serde_json::from_str(&string(&answered, "input")).unwrap_or_else(|why| {
        panic!("case {case:?}: expected JSON input on the task, got {why} for {answered:?}")
    });
    (token, input)
}

/// Describe `execution` until it reports `status`, and answer what it then said.
async fn reported_as(emulator: &Emulator, case: &str, execution: &str, status: &str) -> Value {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let described = describe_execution(emulator, execution).await;
        if described.get("status").and_then(Value::as_str) == Some(status) {
            return described;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected {execution} to report {status}, got {described:?}"
        );
        ::tokio::time::sleep(POLL_PAUSE).await;
    }
}

/// The types of the events an execution's history holds, in order.
fn event_types(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| event.get("type").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// A task a branch had handed to a worker before a sibling unit's failure reached it is
/// SCHEDULED, claimable and then aborted — and the execution ends with the failing unit's
/// own error.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_handed_over_task_is_claimable_and_then_aborted_by_a_siblings_failure() {
    let emulator = Emulator::new();
    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let name = format!("handover-abort-{index}");
        let activity = create_activity(&emulator, &name).await;
        assert_eq!(
            activity,
            activity_arn(&name),
            "case {case:?}: expected the activity's own ARN, got {activity:?}"
        );
        let machine = create_machine(&emulator, &name, &definition(&activity, &row)).await;
        let execution = start_execution(&emulator, &machine, "only", "{}").await;

        // The task really was scheduled under the activity, from inside the branch: a
        // worker claims it and is handed the state's effective input.
        let (token, input) = claim(&emulator, case, &activity).await;
        assert_eq!(
            input,
            json!({}),
            "case {case:?}: expected the state's effective input on the task, got {input:?}"
        );

        // While the hand-over has not been stopped the execution has reached no outcome.
        let waiting = describe_execution(&emulator, &execution).await;
        assert_eq!(
            waiting.get("status").and_then(Value::as_str),
            Some("RUNNING"),
            "case {case:?}: expected the execution to be running, got {waiting:?}"
        );

        // The token answers NOTHING: the work it carries was stopped with the state that
        // asked for it, so a worker reporting on it names a task that has been closed.
        let (status, code) = emulator
            .err(
                "SendTaskSuccess",
                &json!({ "taskToken": &token, "output": "{}" }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, TASK_TIMED_OUT),
            "case {case:?}: expected the aborted task's token to answer nothing, got {status} \
             {code}"
        );

        // And the execution ends with the FAILING unit's own error.
        let ended = reported_as(&emulator, case, &execution, "FAILED").await;
        assert_eq!(
            ended.get("error").and_then(Value::as_str),
            Some(row.error),
            "case {case:?}: expected the failing unit's error, got {ended:?}"
        );
        if row.fails == Fails::AtOnce {
            assert_eq!(
                ended.get("cause").and_then(Value::as_str),
                Some(SIBLING_CAUSE),
                "case {case:?}: expected the failing branch's own cause, got {ended:?}"
            );
        }

        // The history reports the hand-over as scheduled, taken by a worker and then
        // ABORTED — the event `HistoryEventType` models for a `Task` state the work around
        // it stopped — rather than as a state that scheduled nothing.
        let events = history(&emulator, &execution, json!({})).await;
        let types = event_types(&events);
        for required in [ACTIVITY_SCHEDULED, ACTIVITY_STARTED, TASK_STATE_ABORTED] {
            assert_eq!(
                types.iter().filter(|seen| *seen == required).count(),
                1,
                "case {case:?}: expected exactly one {required} event, got {types:?}"
            );
        }
        // The ABORT comes after the claim, which is after the scheduling: the task was
        // handed over, taken, and only then stopped.
        let position = |kind: &str| types.iter().position(|seen| seen == kind);
        assert!(
            position(ACTIVITY_SCHEDULED) < position(ACTIVITY_STARTED)
                && position(ACTIVITY_STARTED) < position(TASK_STATE_ABORTED),
            "case {case:?}: expected the hand-over to be scheduled, started and then aborted, \
             got {types:?}"
        );
        // And the event the scheduling was recorded by carries what was scheduled: the
        // activity and the document the worker was handed.
        let scheduled = events
            .iter()
            .find(|event| event.get("type").and_then(Value::as_str) == Some(ACTIVITY_SCHEDULED))
            .and_then(|event| event.get("activityScheduledEventDetails"))
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected details on the {ACTIVITY_SCHEDULED} event")
            });
        assert_eq!(
            scheduled.get("resource").and_then(Value::as_str),
            Some(activity.as_str()),
            "case {case:?}: expected the scheduled activity on the event, got {scheduled:?}"
        );
    }
}

/// The history of a run whose hand-over was aborted numbers its events in ONE unbroken
/// series, however the claim interleaves with what the clock still owes: every event names
/// the event before it, and no two share an id.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_claim_of_an_aborted_task_keeps_the_history_numbering_unbroken() {
    let emulator = Emulator::new();
    let case = "a hand-over a misevaluating Map beside it stopped";
    let row = Row {
        case,
        fails: Fails::MisevaluatingMap,
        handing_over_first: true,
        levels: 1,
        error: QUERY_EVALUATION_ERROR,
    };
    let activity = create_activity(&emulator, "handover-numbering").await;
    let machine =
        create_machine(&emulator, "handover-numbering", &definition(&activity, &row)).await;
    let execution = start_execution(&emulator, &machine, "only", "{}").await;
    let (_token, _input) = claim(&emulator, case, &activity).await;
    let ended = reported_as(&emulator, case, &execution, "FAILED").await;

    let events = history(&emulator, &execution, json!({})).await;
    let ids: Vec<i64> = events
        .iter()
        .filter_map(|event| event.get("id").and_then(Value::as_i64))
        .collect();
    let expected: Vec<i64> = (1..=i64::try_from(events.len()).unwrap_or(0)).collect();
    assert_eq!(
        ids, expected,
        "case {case:?}: expected one unbroken series of event ids, got {ids:?}"
    );
    for pair in events.windows(2) {
        let [before, after] = pair else { continue };
        // An `ExecutionStarted` is no predecessor: the event after it reports the sentinel
        // the service reports for "no previous event".
        if before.get("type").and_then(Value::as_str) == Some("ExecutionStarted") {
            continue;
        }
        assert_eq!(
            after.get("previousEventId").and_then(Value::as_i64),
            before.get("id").and_then(Value::as_i64),
            "case {case:?}: expected {after:?} to name {before:?} as its predecessor"
        );
    }

    // The diagnosis the run failed with names the event the failing state was ENTERED at,
    // and that id is the one this history really holds that entry under — the claim's own
    // event cannot move it.
    let entered = events
        .iter()
        .find(|event| {
            event
                .get("stateEnteredEventDetails")
                .and_then(|details| details.get("name"))
                .and_then(Value::as_str)
                == Some("BadTask")
        })
        .and_then(|event| event.get("id").and_then(Value::as_i64))
        .unwrap_or_else(|| panic!("case {case:?}: expected BadTask to be entered in {events:?}"));
    let cause = ended
        .get("cause")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        cause.contains(&format!("(entered at the event id #{entered})")),
        "case {case:?}: expected the cause to name event #{entered}, got {cause:?}"
    );
}
