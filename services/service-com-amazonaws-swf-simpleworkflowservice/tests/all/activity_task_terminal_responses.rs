// SPDX-License-Identifier: CC0-1.0

//! A worker's terminal response to an activity task, end to end over the service's own
//! wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: SimpleWorkflowService.<Operation>`), through the same
//! `MultiServiceRouter` the emulator mounts in production. The harness is the one
//! `workflow_execution_lifecycle` already builds.
//!
//! The rule every case here asserts: "A task is considered open from the time that it is
//! scheduled until it is closed. ... A task is closed after it has been specified in a
//! call to RespondActivityTaskCompleted, RespondActivityTaskCanceled,
//! RespondActivityTaskFailed, or the task has timed out." So each of those three calls
//! records its own close event on the task's run, takes the task out of
//! `openCounts.openActivityTasks` ("The count of activity tasks whose status is OPEN"),
//! and gives the decider a decision task carrying the result; exactly ONE of them is
//! recorded per task, so a second response for the same handle is refused with the
//! modelled `UnknownResourceFault`; and the run then runs on to completion, so the
//! decider's `CompleteWorkflowExecution` closes it and the closed run appears in
//! `ListClosedWorkflowExecutions` with that close status.
//!
//! The rows vary what the rule generalises over: which of the three terminal responses
//! the worker makes, whether the task was asked to cancel first, the payload it reports
//! (absent, text, non-ASCII) and which response loses the race for a token that is
//! already closed.

use serde_json::{json, Value};
use service_com_amazonaws_swf_simpleworkflowservice::ops::types::EVENT_TYPE_VALUES;

use super::workflow_execution_lifecycle::{
    error_code, full_defaults, i64_at, seeded, string_at, whole_history, Emulator,
};

/// The workflow type every run in this suite is started with.
const WORKFLOW_TYPE: (&str, &str) = ("close-wf", "1.0");

/// The `defaultTaskList` that type carries, so every run's decision tasks wait there.
const DECIDER_LIST: &str = "close-deciders";

/// The activity type every task in this suite is scheduled from.
const ACTIVITY_TYPE: (&str, &str) = ("close-activity", "1.0");

/// The activity type's `defaultTaskList`, where a scheduled activity task waits.
const ACTIVITY_LIST: &str = "close-activities";

/// `oldestDate` is required on a visibility listing, and this is the oldest instant the
/// documented Unix-time form can name, so it excludes nothing.
const EPOCH_SECONDS: f64 = 0.0;

/// Register the domain, the workflow type and the activity type one row uses.
async fn seed(emulator: &Emulator, domain: &str) {
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

/// Poll one decision task off the decider list and answer it with `decisions`.
async fn decide(emulator: &Emulator, case: &str, domain: &str, decisions: Value) {
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
    emulator
        .ok(
            case,
            "RespondDecisionTaskCompleted",
            &json!({ "taskToken": token, "decisions": decisions }),
        )
        .await;
}

/// Read one run back through `DescribeWorkflowExecution`.
async fn describe(emulator: &Emulator, case: &str, domain: &str, run_id: &str) -> Value {
    emulator
        .ok(
            case,
            "DescribeWorkflowExecution",
            &json!({
                "domain": domain,
                "execution": { "workflowId": "wf-1", "runId": run_id },
            }),
        )
        .await
}

/// Every event of `events` whose type is `event_type`.
fn of_type(case: &str, events: &[Value], event_type: &str) -> Vec<Value> {
    assert!(
        EVENT_TYPE_VALUES.contains(&event_type),
        "case {case:?}: {event_type:?} must be a modelled EventType"
    );
    events
        .iter()
        .filter(|event| {
            event
                .pointer("/eventType")
                .and_then(Value::as_str)
                .is_some_and(|recorded| recorded == event_type)
        })
        .cloned()
        .collect()
}

/// One worker's terminal response.
struct CloseCase {
    name: &'static str,
    domain: &'static str,
    /// The operation the worker answers its task with.
    operation: &'static str,
    /// The members that operation carries beside the `taskToken`.
    payload: Value,
    /// The event that response records.
    event_type: &'static str,
    /// Whether the decider asks the task to cancel before the worker answers, which is
    /// what gives an `ActivityTaskCanceled` event a cancel-requested event to name.
    request_cancel: bool,
    /// The losing racer: the operation that answers the SAME token afterwards.
    loser: &'static str,
}

#[tokio::test]
async fn a_terminal_response_closes_its_task_and_lets_the_run_finish() {
    let emulator = Emulator::new();
    let cases = [
        CloseCase {
            name: "completed with a result",
            domain: "close-completed",
            operation: "RespondActivityTaskCompleted",
            payload: json!({ "result": "{\"ok\":true}" }),
            event_type: "ActivityTaskCompleted",
            request_cancel: false,
            loser: "RespondActivityTaskFailed",
        },
        CloseCase {
            name: "completed with no result at all",
            domain: "close-completed-bare",
            operation: "RespondActivityTaskCompleted",
            payload: json!({}),
            event_type: "ActivityTaskCompleted",
            request_cancel: false,
            loser: "RespondActivityTaskCanceled",
        },
        CloseCase {
            name: "failed with a reason and details",
            domain: "close-failed",
            operation: "RespondActivityTaskFailed",
            payload: json!({ "reason": "nope", "details": "the worker gave up" }),
            event_type: "ActivityTaskFailed",
            request_cancel: false,
            loser: "RespondActivityTaskCompleted",
        },
        CloseCase {
            name: "failed reporting non-ASCII details",
            domain: "close-failed-utf8",
            operation: "RespondActivityTaskFailed",
            payload: json!({ "reason": "ошибка", "details": "подробности" }),
            event_type: "ActivityTaskFailed",
            request_cancel: false,
            loser: "RespondActivityTaskCanceled",
        },
        CloseCase {
            name: "canceled after the decider asked",
            domain: "close-canceled",
            operation: "RespondActivityTaskCanceled",
            payload: json!({ "details": "rolled back" }),
            event_type: "ActivityTaskCanceled",
            request_cancel: true,
            loser: "RespondActivityTaskCompleted",
        },
        CloseCase {
            name: "canceled with no details and nobody asking",
            domain: "close-canceled-bare",
            operation: "RespondActivityTaskCanceled",
            payload: json!({}),
            event_type: "ActivityTaskCanceled",
            request_cancel: false,
            loser: "RespondActivityTaskFailed",
        },
    ];

    for case in &cases {
        seed(&emulator, case.domain).await;
        let started = emulator
            .ok(
                case.name,
                "StartWorkflowExecution",
                &json!({
                    "domain": case.domain,
                    "workflowId": "wf-1",
                    "workflowType": { "name": WORKFLOW_TYPE.0, "version": WORKFLOW_TYPE.1 },
                }),
            )
            .await;
        let run_id = string_at(&started, "/runId");

        // The decider schedules one activity task, and a worker takes it.
        decide(
            &emulator,
            case.name,
            case.domain,
            json!([{
                "decisionType": "ScheduleActivityTask",
                "scheduleActivityTaskDecisionAttributes": {
                    "activityId": "act-1",
                    "activityType": { "name": ACTIVITY_TYPE.0, "version": ACTIVITY_TYPE.1 },
                },
            }]),
        )
        .await;
        let scheduled = describe(&emulator, case.name, case.domain, &run_id).await;
        assert_eq!(
            i64_at(&scheduled, "/openCounts/openActivityTasks"),
            1,
            "case {:?}: the scheduled activity task is open, got {scheduled:?}",
            case.name
        );
        let activity_task = emulator
            .ok(
                case.name,
                "PollForActivityTask",
                &json!({ "domain": case.domain, "taskList": { "name": ACTIVITY_LIST } }),
            )
            .await;
        let task_token = string_at(&activity_task, "/taskToken");
        assert!(
            !task_token.is_empty(),
            "case {:?}: the scheduled task must be handed to the worker, got \
             {activity_task:?}",
            case.name
        );
        let held = describe(&emulator, case.name, case.domain, &run_id).await;
        assert_eq!(
            i64_at(&held, "/openCounts/openActivityTasks"),
            1,
            "case {:?}: a task with a worker is still open, got {held:?}",
            case.name
        );

        if case.request_cancel {
            // Nothing the decider has not seen is outstanding while the worker holds the
            // task, so a signal is what gives the run a decision task to answer with the
            // cancellation request.
            emulator
                .ok(
                    case.name,
                    "SignalWorkflowExecution",
                    &json!({
                        "domain": case.domain,
                        "workflowId": "wf-1",
                        "runId": run_id,
                        "signalName": "please-cancel",
                    }),
                )
                .await;
            // The decider asks the running task to cancel; the worker is the one that
            // reports back, so the task stays open until it does.
            decide(
                &emulator,
                case.name,
                case.domain,
                json!([{
                    "decisionType": "RequestCancelActivityTask",
                    "requestCancelActivityTaskDecisionAttributes": { "activityId": "act-1" },
                }]),
            )
            .await;
            let asked = describe(&emulator, case.name, case.domain, &run_id).await;
            assert_eq!(
                i64_at(&asked, "/openCounts/openActivityTasks"),
                1,
                "case {:?}: asking a started task to cancel does not close it, got \
                 {asked:?}",
                case.name
            );
        }

        // The worker's terminal response: an empty 200, and the close recorded in state.
        let mut body = json!({ "taskToken": task_token });
        if let (Some(target), Some(source)) = (body.as_object_mut(), case.payload.as_object()) {
            for (member, value) in source {
                target.insert(member.clone(), value.clone());
            }
        }
        let answered = emulator.ok(case.name, case.operation, &body).await;
        assert_eq!(
            answered,
            json!({}),
            "case {:?}: {} answers with an empty body, got {answered:?}",
            case.name,
            case.operation
        );

        let events = whole_history(&emulator, case.domain, "wf-1", &run_id).await;
        let closed = of_type(case.name, &events, case.event_type);
        assert_eq!(
            closed.len(),
            1,
            "case {:?}: exactly one {} event is recorded, got {events:?}",
            case.name,
            case.event_type
        );
        let scheduled_event_id = i64_at(
            &of_type(case.name, &events, "ActivityTaskScheduled")[0],
            "/eventId",
        );
        let started_event_id = i64_at(
            &of_type(case.name, &events, "ActivityTaskStarted")[0],
            "/eventId",
        );
        let attributes = closed[0]
            .pointer(&format!(
                "/{}EventAttributes",
                lower_first(case.event_type)
            ))
            .unwrap_or_else(|| panic!("case {:?}: no close attributes in {:?}", case.name, closed[0]));
        assert_eq!(
            (
                i64_at(attributes, "/scheduledEventId"),
                i64_at(attributes, "/startedEventId")
            ),
            (scheduled_event_id, started_event_id),
            "case {:?}: the close event names the two events that scheduled and started \
             the task, got {attributes:?}",
            case.name
        );
        for (member, expected) in case.payload.as_object().expect("an object payload") {
            assert_eq!(
                attributes.get(member),
                Some(expected),
                "case {:?}: the close event carries the {member} the worker reported, \
                 got {attributes:?}",
                case.name
            );
        }
        if case.event_type == "ActivityTaskCanceled" {
            let asked_at = of_type(case.name, &events, "ActivityTaskCancelRequested")
                .first()
                .map_or(0, |event| i64_at(event, "/eventId"));
            assert_eq!(
                i64_at(attributes, "/latestCancelRequestedEventId"),
                asked_at,
                "case {:?}: the event names the last cancellation asked for, or the \
                 member's default where none was, got {attributes:?}",
                case.name
            );
        }

        // The task is out of flight, and the decider has a task carrying the result.
        let drained = describe(&emulator, case.name, case.domain, &run_id).await;
        assert_eq!(
            (
                i64_at(&drained, "/openCounts/openActivityTasks"),
                i64_at(&drained, "/openCounts/openDecisionTasks"),
                string_at(&drained, "/executionInfo/executionStatus"),
            ),
            (0, 1, "OPEN".to_string()),
            "case {:?}: the closed task leaves the open-task count and the decider is \
             told, got {drained:?}",
            case.name
        );

        // The losing racer addresses a task that is closed, so it names no resource.
        let (status, refused) = emulator
            .call(case.loser, &json!({ "taskToken": task_token }))
            .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "UnknownResourceFault"),
            "case {:?}: a second terminal response for a closed task is refused, got \
             {status} with {refused:?}",
            case.name
        );
        assert_eq!(
            refused.get("message").and_then(Value::as_str),
            Some(format!("Unknown activity, scheduledEventId = {scheduled_event_id}").as_str()),
            "case {:?}: the refusal names the activity task the handle was on, got \
             {refused:?}",
            case.name
        );
        let after_refusal = whole_history(&emulator, case.domain, "wf-1", &run_id).await;
        assert_eq!(
            after_refusal.len(),
            events.len(),
            "case {:?}: the refused response records nothing at all, got \
             {after_refusal:?}",
            case.name
        );

        // The decider answers that task by completing the run, which is what the
        // visibility readers must then report.
        decide(
            &emulator,
            case.name,
            case.domain,
            json!([{
                "decisionType": "CompleteWorkflowExecution",
                "completeWorkflowExecutionDecisionAttributes": { "result": "finished" },
            }]),
        )
        .await;
        let finished = describe(&emulator, case.name, case.domain, &run_id).await;
        assert_eq!(
            (
                string_at(&finished, "/executionInfo/executionStatus"),
                string_at(&finished, "/executionInfo/closeStatus"),
                i64_at(&finished, "/openCounts/openActivityTasks"),
                i64_at(&finished, "/openCounts/openDecisionTasks"),
            ),
            ("CLOSED".to_string(), "COMPLETED".to_string(), 0, 0),
            "case {:?}: the run the decider completed is CLOSED/COMPLETED with nothing \
             left in flight, got {finished:?}",
            case.name
        );
        assert!(
            finished
                .pointer("/executionInfo/closeTimestamp")
                .and_then(Value::as_f64)
                .is_some_and(|at| at > 0.0),
            "case {:?}: a closed run carries a close instant, got {finished:?}",
            case.name
        );

        let listed = emulator
            .ok(
                case.name,
                "ListClosedWorkflowExecutions",
                &json!({
                    "domain": case.domain,
                    "startTimeFilter": { "oldestDate": EPOCH_SECONDS },
                }),
            )
            .await;
        let infos = listed
            .pointer("/executionInfos")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {:?}: no executionInfos in {listed:?}", case.name));
        let found = infos
            .iter()
            .find(|info| {
                info.pointer("/execution/runId")
                    .and_then(Value::as_str)
                    .is_some_and(|listed| listed == run_id)
            })
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the completed run must appear in the closed listing, got \
                     {listed:?}",
                    case.name
                )
            });
        assert_eq!(
            string_at(found, "/closeStatus"),
            "COMPLETED",
            "case {:?}: the closed listing reports the status the run closed with, got \
             {found:?}",
            case.name
        );
    }
}

#[tokio::test]
async fn a_handle_that_names_no_open_activity_task_is_refused() {
    let emulator = Emulator::new();
    let domain = "stale";
    seed(&emulator, domain).await;
    let started = emulator
        .ok(
            "stale",
            "StartWorkflowExecution",
            &json!({
                "domain": domain,
                "workflowId": "wf-1",
                "workflowType": { "name": WORKFLOW_TYPE.0, "version": WORKFLOW_TYPE.1 },
            }),
        )
        .await;
    let run_id = string_at(&started, "/runId");

    // A decision task's own token is a handle on a decision task, not on an activity
    // task, so it addresses no activity at all — and neither does a token the service
    // never minted.
    let decision_task = emulator
        .ok(
            "stale",
            "PollForDecisionTask",
            &json!({ "domain": domain, "taskList": { "name": DECIDER_LIST } }),
        )
        .await;
    let decision_token = string_at(&decision_task, "/taskToken");
    // A decision token is a handle on a decision task of the stored history, so the
    // refusal names that task's own scheduling event; a token the service never minted is
    // a handle on no started task at all, so the `EventId` member's own default is what
    // names it.
    let decision_started_event_id = i64_at(&decision_task, "/startedEventId");
    let decision_scheduled_event_id = decision_task
        .pointer("/events")
        .and_then(Value::as_array)
        .and_then(|events| {
            events
                .iter()
                .find(|event| {
                    event
                        .get("eventId")
                        .and_then(Value::as_i64)
                        .is_some_and(|id| id == decision_started_event_id)
                })
                .and_then(|event| {
                    event
                        .pointer("/decisionTaskStartedEventAttributes/scheduledEventId")
                        .and_then(Value::as_i64)
                })
        })
        .unwrap_or_else(|| {
            panic!("case \"stale\": the handed-out decision task names its own scheduling event in {decision_task:?}")
        });

    for (case, token, expected_scheduled_event_id) in [
        (
            "a decision task's own token",
            decision_token.clone(),
            decision_scheduled_event_id,
        ),
        (
            "a token the service never minted",
            "not-a-token".to_string(),
            0,
        ),
    ] {
        let expected_message =
            format!("Unknown activity, scheduledEventId = {expected_scheduled_event_id}");
        for operation in [
            "RespondActivityTaskCompleted",
            "RespondActivityTaskFailed",
            "RespondActivityTaskCanceled",
        ] {
            let (status, refused) = emulator
                .call(operation, &json!({ "taskToken": token }))
                .await;
            assert_eq!(
                (status, error_code(&refused).as_str()),
                (400, "UnknownResourceFault"),
                "case {case:?}: {operation} on a handle that names no activity task is \
                 refused, got {status} with {refused:?}"
            );
            assert_eq!(
                refused.get("message").and_then(Value::as_str),
                Some(expected_message.as_str()),
                "case {case:?}: {operation} names the task the supplied handle addresses, \
                 got {refused:?}"
            );
        }
    }

    // Nothing of the run changed: the refusals recorded no event.
    let events = whole_history(&emulator, domain, "wf-1", &run_id).await;
    assert_eq!(
        of_type("stale", &events, "ActivityTaskCompleted").len()
            + of_type("stale", &events, "ActivityTaskFailed").len()
            + of_type("stale", &events, "ActivityTaskCanceled").len(),
        0,
        "case \"stale\": a refused response records no close event, got {events:?}"
    );
}

/// The `eventType` name with its first letter lowercased, which is how `HistoryEvent`
/// names the attributes member carrying that event's own attributes.
fn lower_first(event_type: &str) -> String {
    let mut characters = event_type.chars();
    match characters.next() {
        Some(first) => first.to_lowercase().chain(characters).collect(),
        None => String::new(),
    }
}
