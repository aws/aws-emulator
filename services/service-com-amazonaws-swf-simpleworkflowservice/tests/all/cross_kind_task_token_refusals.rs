// SPDX-License-Identifier: CC0-1.0

//! What an `UnknownResourceFault` about a TASK reports when the supplied `taskToken` is a
//! handle on a task of the OTHER kind, end to end over the service's own wire protocol
//! (`awsJson1_0`, `POST /` with `X-Amz-Target: SimpleWorkflowService.<Operation>`) through
//! the same `MultiServiceRouter` the emulator mounts in production. The harness is the one
//! `workflow_execution_lifecycle` already builds.
//!
//! The rule every case here asserts: the `scheduledEventId` such a fault reports is the
//! one the SUPPLIED token addresses — read out of the stored history for whichever KIND of
//! task token the service minted it for — so a token of the kind the call cannot act on
//! still names the task it really is a handle on; only a token that is a handle on no
//! started task of any stored run reports the `EventId` member's own modelled default of
//! `0`. The refusal class, the status and the no-write behaviour are the same either way.
//!
//! Rows vary what the rule generalises over: which kind of task the token is a handle on
//! (decision, activity), which operation receives it (each of the three terminal activity
//! responses, the heartbeat, the decision-task completion), where in its run's history the
//! addressed task sits (the first decision task, a later one, each of two activity tasks
//! of one run), whether that task is still outstanding, and the shape of a token that
//! addresses nothing at all.

use serde_json::{json, Value};

use super::workflow_execution_lifecycle::{
    error_code, full_defaults, i64_at, seeded, string_at, whole_history, Emulator,
};

/// The workflow type every run in this suite is started with. `pub(crate)` so the
/// sibling suite that carries the same rule across domains starts its runs from the same
/// type rather than seeding a second one.
pub(crate) const WORKFLOW_TYPE: (&str, &str) = ("cross-kind-wf", "1.0");

/// The `defaultTaskList` that type carries, so every run's decision tasks wait there.
const DECIDER_LIST: &str = "cross-kind-deciders";

/// The activity type the activity tasks of this suite are scheduled from.
const ACTIVITY_TYPE: (&str, &str) = ("cross-kind-activity", "1.0");

/// The activity type's `defaultTaskList`, where a scheduled activity task waits.
const ACTIVITY_LIST: &str = "cross-kind-activities";

/// The `workflowId` of this suite's single run, and of the sibling suite's one run per
/// domain: a `workflowId` is scoped to its domain, so one name serves both.
pub(crate) const WORKFLOW_ID: &str = "cross-kind-wf-1";

/// The operations that take a `taskToken` and refuse one that addresses no task they can
/// act on, with the fixed text each one's fault carries. The activity-side operations are
/// the three terminal responses the task's own doc names plus the heartbeat; the
/// decision-side one is the decider's completion.
pub(crate) const ACTIVITY_SIDE: [&str; 4] = [
    "RespondActivityTaskCompleted",
    "RespondActivityTaskFailed",
    "RespondActivityTaskCanceled",
    "RecordActivityTaskHeartbeat",
];

/// The fixed text an activity-side refusal carries, with the id the token addresses.
pub(crate) fn unknown_activity(scheduled_event_id: i64) -> String {
    format!("Unknown activity, scheduledEventId = {scheduled_event_id}")
}

/// The fixed text the decision-side refusal carries, with the id the token addresses.
pub(crate) fn unknown_decision_task(scheduled_event_id: i64) -> String {
    format!("Unknown decision task, scheduledEventId = {scheduled_event_id}")
}

/// Register the domain, the workflow type and the activity type this suite uses.
pub(crate) async fn seed(emulator: &Emulator, domain: &str) {
    seeded(
        emulator,
        domain,
        domain,
        WORKFLOW_TYPE,
        full_defaults(DECIDER_LIST, "TERMINATE"),
    )
    .await;
    emulator
        .ok(
            domain,
            "RegisterActivityType",
            &json!({
                "domain": domain,
                "name": ACTIVITY_TYPE.0,
                "version": ACTIVITY_TYPE.1,
                "defaultTaskList": { "name": ACTIVITY_LIST },
                "defaultTaskScheduleToStartTimeout": "60",
                "defaultTaskScheduleToCloseTimeout": "600",
                "defaultTaskStartToCloseTimeout": "300",
            }),
        )
        .await;
}

/// Poll one decision task that must be waiting, and return
/// `(its taskToken, the scheduledEventId of the task it handed over)`.
pub(crate) async fn poll_decision_task(
    emulator: &Emulator,
    case: &str,
    domain: &str,
) -> (String, i64) {
    let task = emulator
        .ok(
            case,
            "PollForDecisionTask",
            &json!({ "domain": domain, "taskList": { "name": DECIDER_LIST } }),
        )
        .await;
    let token = string_at(&task, "/taskToken");
    assert!(
        !token.is_empty(),
        "case {case:?}: expected a decision task waiting in {DECIDER_LIST:?}, got {task:?}"
    );
    let started_event_id = i64_at(&task, "/startedEventId");
    let events = task
        .pointer("/events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let scheduled_event_id = scheduled_of_started(
        case,
        &events,
        started_event_id,
        "decisionTaskStartedEventAttributes",
    );
    (token, scheduled_event_id)
}

/// Poll one activity task that must be waiting, and return
/// `(its taskToken, the scheduledEventId of the task it handed over)`, reading that id
/// back out of the run's own history.
pub(crate) async fn poll_activity_task(
    emulator: &Emulator,
    case: &str,
    domain: &str,
    run_id: &str,
) -> (String, i64) {
    let task = emulator
        .ok(
            case,
            "PollForActivityTask",
            &json!({ "domain": domain, "taskList": { "name": ACTIVITY_LIST } }),
        )
        .await;
    let token = string_at(&task, "/taskToken");
    assert!(
        !token.is_empty(),
        "case {case:?}: expected an activity task waiting in {ACTIVITY_LIST:?}, got {task:?}"
    );
    let started_event_id = i64_at(&task, "/startedEventId");
    let events = whole_history(emulator, domain, WORKFLOW_ID, run_id).await;
    let scheduled_event_id = scheduled_of_started(
        case,
        &events,
        started_event_id,
        "activityTaskStartedEventAttributes",
    );
    (token, scheduled_event_id)
}

/// The `scheduledEventId` the started event `started_event_id` names, as the stored
/// history records it — the same derivation a client makes from `GetWorkflowExecutionHistory`.
fn scheduled_of_started(
    case: &str,
    events: &[Value],
    started_event_id: i64,
    attributes_member: &str,
) -> i64 {
    let started = events
        .iter()
        .find(|event| {
            event
                .get("eventId")
                .and_then(Value::as_i64)
                .is_some_and(|id| id == started_event_id)
        })
        .unwrap_or_else(|| {
            panic!("case {case:?}: no event with eventId {started_event_id} in {events:?}")
        });
    i64_at(started, &format!("/{attributes_member}/scheduledEventId"))
}

/// One refusal: the operation the token is handed to, the token itself, and the whole
/// message the fault must carry.
struct Row {
    case: String,
    operation: &'static str,
    token: String,
    expected_message: String,
}

#[tokio::test]
async fn a_task_fault_names_the_task_the_supplied_token_addresses() {
    let emulator = Emulator::new();
    let domain = "cross-kind";
    seed(&emulator, domain).await;
    let started = emulator
        .ok(
            "cross-kind",
            "StartWorkflowExecution",
            &json!({
                "domain": domain,
                "workflowId": WORKFLOW_ID,
                "workflowType": { "name": WORKFLOW_TYPE.0, "version": WORKFLOW_TYPE.1 },
            }),
        )
        .await;
    let run_id = string_at(&started, "/runId");

    // The run's FIRST decision task, answered by scheduling two activity tasks, so its
    // token is a handle on a decision task that is over.
    let (first_decision_token, first_decision_scheduled) =
        poll_decision_task(&emulator, "the first decision task", domain).await;
    emulator
        .ok(
            "the first decision task",
            "RespondDecisionTaskCompleted",
            &json!({
                "taskToken": first_decision_token,
                "decisions": [
                    {
                        "decisionType": "ScheduleActivityTask",
                        "scheduleActivityTaskDecisionAttributes": {
                            "activityId": "act-1",
                            "activityType": {
                                "name": ACTIVITY_TYPE.0,
                                "version": ACTIVITY_TYPE.1,
                            },
                        },
                    },
                    {
                        "decisionType": "ScheduleActivityTask",
                        "scheduleActivityTaskDecisionAttributes": {
                            "activityId": "act-2",
                            "activityType": {
                                "name": ACTIVITY_TYPE.0,
                                "version": ACTIVITY_TYPE.1,
                            },
                        },
                    },
                ],
            }),
        )
        .await;

    // Two open activity tasks of the same run, each with its own scheduling event.
    let (first_activity_token, first_activity_scheduled) =
        poll_activity_task(&emulator, "the first activity task", domain, &run_id).await;
    let (second_activity_token, second_activity_scheduled) =
        poll_activity_task(&emulator, "the second activity task", domain, &run_id).await;
    assert_ne!(
        first_activity_scheduled, second_activity_scheduled,
        "case \"two activity tasks\": each task has its own scheduling event, got \
         {first_activity_scheduled} twice"
    );

    // A LATER decision task of the same run, still outstanding, so its token addresses a
    // different scheduling event than the first one's.
    emulator
        .ok(
            "a later decision task",
            "SignalWorkflowExecution",
            &json!({
                "domain": domain,
                "workflowId": WORKFLOW_ID,
                "runId": run_id,
                "signalName": "wake-the-decider",
            }),
        )
        .await;
    let (later_decision_token, later_decision_scheduled) =
        poll_decision_task(&emulator, "a later decision task", domain).await;
    assert_ne!(
        first_decision_scheduled, later_decision_scheduled,
        "case \"a later decision task\": the replacement task has its own scheduling \
         event, got {first_decision_scheduled} twice"
    );

    // A token that is a handle on nothing at all: one of a single character, one of the
    // minted shape with a wrong digest, and the live decision token with one character
    // changed.
    let mutated: String = {
        let mut characters: Vec<char> = later_decision_token.chars().collect();
        if let Some(first) = characters.first_mut() {
            *first = if *first == 'a' { 'b' } else { 'a' };
        }
        characters.into_iter().collect()
    };
    let addresses_nothing = [
        ("a token of a single character", "x".to_string()),
        (
            "a token of the minted shape with a wrong digest",
            "0".repeat(later_decision_token.len()),
        ),
        ("a live token with one character changed", mutated),
    ];

    let mut rows = Vec::new();
    for operation in ACTIVITY_SIDE {
        // A DECISION token handed to an activity-side call: the fault names the decision
        // task the token is really a handle on, whether that task is over or outstanding.
        rows.push(Row {
            case: format!("{operation} on the token of a completed decision task"),
            operation,
            token: first_decision_token.clone(),
            expected_message: unknown_activity(first_decision_scheduled),
        });
        rows.push(Row {
            case: format!("{operation} on the token of an outstanding decision task"),
            operation,
            token: later_decision_token.clone(),
            expected_message: unknown_activity(later_decision_scheduled),
        });
        for (case, token) in &addresses_nothing {
            rows.push(Row {
                case: format!("{operation} on {case}"),
                operation,
                token: token.clone(),
                expected_message: unknown_activity(0),
            });
        }
    }
    // An ACTIVITY token handed to the decider's completion: the mirror of the same rule,
    // one row per activity task of the run so the id is the token's and not the first
    // task's.
    for (case, token, scheduled) in [
        (
            "the token of the first open activity task",
            &first_activity_token,
            first_activity_scheduled,
        ),
        (
            "the token of the second open activity task",
            &second_activity_token,
            second_activity_scheduled,
        ),
    ] {
        rows.push(Row {
            case: format!("RespondDecisionTaskCompleted on {case}"),
            operation: "RespondDecisionTaskCompleted",
            token: token.clone(),
            expected_message: unknown_decision_task(scheduled),
        });
    }
    for (case, token) in &addresses_nothing {
        rows.push(Row {
            case: format!("RespondDecisionTaskCompleted on {case}"),
            operation: "RespondDecisionTaskCompleted",
            token: token.clone(),
            expected_message: unknown_decision_task(0),
        });
    }

    let before = whole_history(&emulator, domain, WORKFLOW_ID, &run_id).await;
    for row in &rows {
        let case = &row.case;
        let (status, refused) = emulator
            .call(row.operation, &json!({ "taskToken": row.token }))
            .await;
        assert_eq!(
            (
                status,
                error_code(&refused).as_str(),
                refused.get("message").and_then(Value::as_str),
            ),
            (
                400,
                "UnknownResourceFault",
                Some(row.expected_message.as_str()),
            ),
            "case {case:?}: expected 400 UnknownResourceFault naming {:?}, got {status} \
             with {refused:?}",
            row.expected_message
        );
    }
    let after = whole_history(&emulator, domain, WORKFLOW_ID, &run_id).await;
    assert_eq!(
        after, before,
        "case \"every refusal\": a refused call records nothing, got {after:?}"
    );

    // Each live handle still addresses its own task, so the refusals consumed nothing:
    // the worker's report on the first activity task and the decider's completion of the
    // outstanding decision task are both honoured.
    let completed = emulator
        .ok(
            "the live activity handle",
            "RespondActivityTaskCompleted",
            &json!({ "taskToken": first_activity_token, "result": "done" }),
        )
        .await;
    assert_eq!(
        completed,
        json!({}),
        "case \"the live activity handle\": the worker's report is honoured, got \
         {completed:?}"
    );
    let decided = emulator
        .ok(
            "the live decision handle",
            "RespondDecisionTaskCompleted",
            &json!({ "taskToken": later_decision_token, "decisions": [] }),
        )
        .await;
    assert_eq!(
        decided,
        json!({}),
        "case \"the live decision handle\": the decider's completion is honoured, got \
         {decided:?}"
    );

    // And once that activity task is CLOSED, its own token addresses no open activity
    // task either — reported with the same id it always named.
    let (status, refused) = emulator
        .call(
            "RespondActivityTaskCompleted",
            &json!({ "taskToken": first_activity_token }),
        )
        .await;
    assert_eq!(
        (
            status,
            error_code(&refused).as_str(),
            refused.get("message").and_then(Value::as_str),
        ),
        (
            400,
            "UnknownResourceFault",
            Some(unknown_activity(first_activity_scheduled).as_str()),
        ),
        "case \"a replayed activity handle\": the refusal names the task the handle was \
         on, got {status} with {refused:?}"
    );
}
