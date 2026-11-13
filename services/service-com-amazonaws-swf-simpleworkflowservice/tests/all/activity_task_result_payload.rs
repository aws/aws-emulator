// SPDX-License-Identifier: CC0-1.0

//! The payload a worker reports when it completes an activity task, end to end over the
//! service's own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: SimpleWorkflowService.<Operation>`), through the same
//! `MultiServiceRouter` the emulator mounts in production. The harness is the one
//! `workflow_execution_lifecycle` already builds.
//!
//! The rule every case here asserts: a worker's completion report persists the `result`
//! it carried on the one `ActivityTaskCompleted` event of the task's own run — "the
//! result appears in the ActivityTaskCompleted event in the workflow history" — byte for
//! byte over the whole range the member's modelled constraint admits, with the member
//! absent from the event exactly when the report carried none; every later reader (the
//! stored history, and the decision task the close hands the decider) answers from that
//! stored event rather than from any request, so the result survives a round trip
//! unaltered; and a payload outside the modelled constraint is refused at the boundary
//! with nothing recorded, leaving the task open for a report that is inside it.
//!
//! The rows vary what the rule generalises over: whether a result is reported at all,
//! and the reported value's size (both published bounds), encoding (ASCII, non-ASCII
//! multibyte, a base64-rendered binary blob) and form (structured text, text carrying
//! the characters a JSON envelope has to escape). A further case varies the number of
//! tasks in flight and the order their workers report, so each result is pinned to its
//! own task rather than to the last report made.

use serde_json::{json, Value};
use service_com_amazonaws_swf_simpleworkflowservice::ops::types::EVENT_TYPE_VALUES;

use super::workflow_execution_lifecycle::{
    error_code, full_defaults, i64_at, seeded, string_at, whole_history, Emulator,
};

/// The workflow type every run in this suite is started with.
const WORKFLOW_TYPE: (&str, &str) = ("result-wf", "1.0");

/// The `defaultTaskList` that type carries, so every run's decision tasks wait there.
const DECIDER_LIST: &str = "result-deciders";

/// The activity type every task in this suite is scheduled from.
const ACTIVITY_TYPE: (&str, &str) = ("result-activity", "1.0");

/// The activity type's `defaultTaskList`, where a scheduled activity task waits.
const ACTIVITY_LIST: &str = "result-activities";

/// The published maximum length of `RespondActivityTaskCompleted`'s `result`, from the
/// member's modelled `Data` constraint (`length 0..32768`) as
/// `smithy_query(query="operation", operation="RespondActivityTaskCompleted")` reports
/// it. The same constraint is on `ActivityTaskCompletedEventAttributes$result`, so the
/// event can carry whatever the report could.
const MAX_RESULT: usize = 32768;

/// The `workflowId` every run in this suite is started with.
const WORKFLOW_ID: &str = "wf-1";

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
                "defaultTaskScheduleToStartTimeout": "600",
                "defaultTaskScheduleToCloseTimeout": "900",
                "defaultTaskStartToCloseTimeout": "900",
            }),
        )
        .await;
}

/// Poll one decision task off the decider list, returning the whole answer.
async fn poll_decision(emulator: &Emulator, case: &str, domain: &str) -> Value {
    let task = emulator
        .ok(
            case,
            "PollForDecisionTask",
            &json!({ "domain": domain, "taskList": { "name": DECIDER_LIST } }),
        )
        .await;
    assert!(
        !string_at(&task, "/taskToken").is_empty(),
        "case {case:?}: expected a decision task waiting in {DECIDER_LIST:?}, got {task:?}"
    );
    task
}

/// Poll one decision task off the decider list and answer it with `decisions`.
async fn decide(emulator: &Emulator, case: &str, domain: &str, decisions: Value) {
    let task = poll_decision(emulator, case, domain).await;
    emulator
        .ok(
            case,
            "RespondDecisionTaskCompleted",
            &json!({ "taskToken": string_at(&task, "/taskToken"), "decisions": decisions }),
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
                "execution": { "workflowId": WORKFLOW_ID, "runId": run_id },
            }),
        )
        .await
}

/// Every event of `events` whose type is `event_type`, in the order recorded.
fn of_type(case: &str, events: &[Value], event_type: &str) -> Vec<Value> {
    assert!(
        EVENT_TYPE_VALUES.contains(&event_type),
        "case {case:?}: {event_type:?} must be a modelled EventType"
    );
    events
        .iter()
        .filter(|event| event.get("eventType").and_then(Value::as_str) == Some(event_type))
        .cloned()
        .collect()
}

/// One activity task's `eventId` pair: the event that scheduled it and the one that
/// started it, matched by the `activityId` the scheduling decision named.
fn task_event_ids(case: &str, events: &[Value], activity_id: &str) -> (i64, i64) {
    let scheduled = of_type(case, events, "ActivityTaskScheduled")
        .into_iter()
        .find(|event| {
            event
                .pointer("/activityTaskScheduledEventAttributes/activityId")
                .and_then(Value::as_str)
                == Some(activity_id)
        })
        .unwrap_or_else(|| {
            panic!("case {case:?}: no ActivityTaskScheduled for {activity_id:?} in {events:?}")
        });
    let scheduled_event_id = i64_at(&scheduled, "/eventId");
    let started = of_type(case, events, "ActivityTaskStarted")
        .into_iter()
        .find(|event| {
            event
                .pointer("/activityTaskStartedEventAttributes/scheduledEventId")
                .and_then(Value::as_i64)
                == Some(scheduled_event_id)
        })
        .unwrap_or_else(|| {
            panic!("case {case:?}: no ActivityTaskStarted for {activity_id:?} in {events:?}")
        });
    (scheduled_event_id, i64_at(&started, "/eventId"))
}

/// The completion report body for `token`, carrying `result` only when one is reported.
fn completed_body(token: &str, result: Option<&str>) -> Value {
    let mut body = json!({ "taskToken": token });
    if let (Some(target), Some(result)) = (body.as_object_mut(), result) {
        target.insert("result".to_string(), json!(result));
    }
    body
}

/// Schedule one activity task on a fresh run and hand it to a worker, returning
/// `(runId, taskToken)`.
async fn task_in_hand(emulator: &Emulator, case: &str, domain: &str) -> (String, String) {
    let run_id = start_run(emulator, case, domain).await;
    decide(emulator, case, domain, json!([schedule_decision("act-1")])).await;
    let token = take_activity_task(emulator, case, domain).await;
    (run_id, token)
}

/// Start one run of this suite's workflow type, returning its `runId`.
async fn start_run(emulator: &Emulator, case: &str, domain: &str) -> String {
    let started = emulator
        .ok(
            case,
            "StartWorkflowExecution",
            &json!({
                "domain": domain,
                "workflowId": WORKFLOW_ID,
                "workflowType": { "name": WORKFLOW_TYPE.0, "version": WORKFLOW_TYPE.1 },
            }),
        )
        .await;
    string_at(&started, "/runId")
}

/// A `ScheduleActivityTask` decision for `activity_id`.
fn schedule_decision(activity_id: &str) -> Value {
    json!({
        "decisionType": "ScheduleActivityTask",
        "scheduleActivityTaskDecisionAttributes": {
            "activityId": activity_id,
            "activityType": { "name": ACTIVITY_TYPE.0, "version": ACTIVITY_TYPE.1 },
        },
    })
}

/// Take the next activity task waiting in this suite's activity list.
async fn take_activity_task(emulator: &Emulator, case: &str, domain: &str) -> String {
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
        "case {case:?}: the scheduled task must be handed to the worker, got {task:?}"
    );
    token
}

/// One worker's completion report, varying the payload it carries.
struct ResultCase {
    name: &'static str,
    domain: &'static str,
    /// The `result` the worker reports, or `None` when it reports none at all. An owned
    /// string so a row can be the member's published maximum length.
    result: Option<String>,
}

#[tokio::test]
async fn the_reported_result_is_persisted_on_the_completed_event() {
    let cases = [
        ResultCase {
            name: "no result at all",
            domain: "result-absent",
            result: None,
        },
        ResultCase {
            name: "the member's published minimum length",
            domain: "result-empty",
            result: Some(String::new()),
        },
        ResultCase {
            name: "a single character",
            domain: "result-one",
            result: Some("7".to_string()),
        },
        ResultCase {
            name: "structured text",
            domain: "result-json",
            result: Some("{\"items\":[1,2,3],\"ok\":true}".to_string()),
        },
        ResultCase {
            name: "non-ASCII multibyte text",
            domain: "result-utf8",
            result: Some("результат ✅ 完了".to_string()),
        },
        ResultCase {
            name: "a binary blob rendered as base64",
            domain: "result-blob",
            result: Some("AAECf4D/6pSNEREiMw==".to_string()),
        },
        ResultCase {
            name: "characters the JSON envelope must escape",
            domain: "result-escapes",
            result: Some("line\r\n\ttab \"quoted\" back\\slash \u{1}\u{1f}".to_string()),
        },
        ResultCase {
            name: "the member's published maximum length",
            domain: "result-max",
            result: Some("r".repeat(MAX_RESULT)),
        },
    ];

    for case in &cases {
        let emulator = Emulator::new();
        seed(&emulator, case.domain).await;
        let (run_id, token) = task_in_hand(&emulator, case.name, case.domain).await;

        let answered = emulator
            .ok(
                case.name,
                "RespondActivityTaskCompleted",
                &completed_body(&token, case.result.as_deref()),
            )
            .await;
        assert_eq!(
            answered,
            json!({}),
            "case {:?}: RespondActivityTaskCompleted answers with an empty body, got \
             {answered:?}",
            case.name
        );

        // The stored history: exactly one close event for the task, carrying the payload
        // the worker reported and naming the two events that scheduled and started it.
        let events = whole_history(&emulator, case.domain, WORKFLOW_ID, &run_id).await;
        let completed = of_type(case.name, &events, "ActivityTaskCompleted");
        assert_eq!(
            completed.len(),
            1,
            "case {:?}: exactly one ActivityTaskCompleted event is recorded, got {events:?}",
            case.name
        );
        let attributes = completed[0]
            .pointer("/activityTaskCompletedEventAttributes")
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: no activityTaskCompletedEventAttributes in {:?}",
                    case.name, completed[0]
                )
            });
        assert_eq!(
            attributes.get("result").and_then(Value::as_str),
            case.result.as_deref(),
            "case {:?}: the event carries the result the worker reported, and the member \
             only when one was reported, got {:?}",
            case.name,
            attributes.get("result")
        );
        assert_eq!(
            (
                i64_at(attributes, "/scheduledEventId"),
                i64_at(attributes, "/startedEventId"),
            ),
            task_event_ids(case.name, &events, "act-1"),
            "case {:?}: the event names the two events that scheduled and started the \
             task, got {attributes:?}",
            case.name
        );

        // The close took the task out of flight and gave the decider a task to act on.
        let drained = describe(&emulator, case.name, case.domain, &run_id).await;
        assert_eq!(
            (
                i64_at(&drained, "/openCounts/openActivityTasks"),
                i64_at(&drained, "/openCounts/openDecisionTasks"),
                string_at(&drained, "/executionInfo/executionStatus"),
            ),
            (0, 1, "OPEN".to_string()),
            "case {:?}: the completed task leaves the open-task count and the decider is \
             told, got {drained:?}",
            case.name
        );

        // A second report on the same handle addresses a task that is closed, and
        // records nothing.
        let (status, refused) = emulator
            .call(
                "RespondActivityTaskCompleted",
                &completed_body(&token, Some("second")),
            )
            .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "UnknownResourceFault"),
            "case {:?}: a second completion report for a closed task is refused, got \
             {status} with {refused:?}",
            case.name
        );
        let after_refusal = whole_history(&emulator, case.domain, WORKFLOW_ID, &run_id).await;
        assert_eq!(
            after_refusal, events,
            "case {:?}: the refused report changes no event at all, got {after_refusal:?}",
            case.name
        );

        // The decider reads the result from the stored history too: the decision task the
        // close scheduled carries the same event with the same payload.
        let handed = poll_decision(&emulator, case.name, case.domain).await;
        let delivered = of_type(
            case.name,
            handed
                .pointer("/events")
                .and_then(Value::as_array)
                .unwrap_or_else(|| {
                    panic!("case {:?}: no events on the decision task {handed:?}", case.name)
                }),
            "ActivityTaskCompleted",
        );
        assert_eq!(
            delivered
                .first()
                .and_then(|event| event
                    .pointer("/activityTaskCompletedEventAttributes/result")
                    .and_then(Value::as_str)),
            case.result.as_deref(),
            "case {:?}: the decision task carries the stored result to the decider, got \
             {delivered:?}",
            case.name
        );
    }
}

#[tokio::test]
async fn a_result_past_the_published_bound_is_refused_and_leaves_the_task_open() {
    let emulator = Emulator::new();
    let domain = "result-overlong";
    seed(&emulator, domain).await;
    let (run_id, token) = task_in_hand(&emulator, "one past the bound", domain).await;
    let before = whole_history(&emulator, domain, WORKFLOW_ID, &run_id).await;

    let (status, refused) = emulator
        .call(
            "RespondActivityTaskCompleted",
            &completed_body(&token, Some(&"r".repeat(MAX_RESULT + 1))),
        )
        .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "ValidationException"),
        "case \"one past the bound\": a result longer than the modelled {MAX_RESULT} \
         characters is refused at the boundary, got {status} with {refused:?}"
    );

    let after = whole_history(&emulator, domain, WORKFLOW_ID, &run_id).await;
    assert_eq!(
        after, before,
        "case \"one past the bound\": a refused report records no event, got {after:?} \
         after {before:?}"
    );
    let held = describe(&emulator, "one past the bound", domain, &run_id).await;
    assert_eq!(
        (
            i64_at(&held, "/openCounts/openActivityTasks"),
            i64_at(&held, "/openCounts/openDecisionTasks"),
        ),
        (1, 0),
        "case \"one past the bound\": the refused report leaves the task open and the \
         decider untold, got {held:?}"
    );

    // The handle was not consumed: the same worker reports a payload inside the bound and
    // that one is recorded.
    emulator
        .ok(
            "one past the bound",
            "RespondActivityTaskCompleted",
            &completed_body(&token, Some(&"r".repeat(MAX_RESULT))),
        )
        .await;
    let events = whole_history(&emulator, domain, WORKFLOW_ID, &run_id).await;
    let completed = of_type("one past the bound", &events, "ActivityTaskCompleted");
    assert_eq!(
        completed.len(),
        1,
        "case \"one past the bound\": the retried report closes the task that was still \
         open, got {events:?}"
    );
    assert_eq!(
        completed[0]
            .pointer("/activityTaskCompletedEventAttributes/result")
            .and_then(Value::as_str)
            .map(str::len),
        Some(MAX_RESULT),
        "case \"one past the bound\": the accepted payload is stored whole, got {:?}",
        completed[0]
    );
}

#[tokio::test]
async fn each_task_of_one_run_records_its_own_result() {
    let emulator = Emulator::new();
    let domain = "result-many";
    let case = "two tasks reporting out of order";
    seed(&emulator, domain).await;
    let run_id = start_run(&emulator, case, domain).await;

    // Two tasks in flight at once, so a report has to be matched to the handle it was
    // made on rather than to the task scheduled last.
    decide(
        &emulator,
        case,
        domain,
        json!([schedule_decision("act-first"), schedule_decision("act-second")]),
    )
    .await;
    let first_token = take_activity_task(&emulator, case, domain).await;
    let second_token = take_activity_task(&emulator, case, domain).await;
    assert_ne!(
        first_token, second_token,
        "case {case:?}: two tasks in flight are two distinct handles, got {first_token:?} twice"
    );

    // The worker holding the SECOND task reports first.
    emulator
        .ok(
            case,
            "RespondActivityTaskCompleted",
            &completed_body(&second_token, Some("second-result")),
        )
        .await;
    emulator
        .ok(
            case,
            "RespondActivityTaskCompleted",
            &completed_body(&first_token, Some("first-result")),
        )
        .await;

    let events = whole_history(&emulator, domain, WORKFLOW_ID, &run_id).await;
    for (activity_id, expected) in [
        ("act-first", "first-result"),
        ("act-second", "second-result"),
    ] {
        let (scheduled_event_id, started_event_id) = task_event_ids(case, &events, activity_id);
        let recorded = of_type(case, &events, "ActivityTaskCompleted")
            .into_iter()
            .find(|event| {
                event
                    .pointer("/activityTaskCompletedEventAttributes/scheduledEventId")
                    .and_then(Value::as_i64)
                    == Some(scheduled_event_id)
            })
            .unwrap_or_else(|| {
                panic!("case {case:?}: no close event for {activity_id:?} in {events:?}")
            });
        let attributes = recorded
            .pointer("/activityTaskCompletedEventAttributes")
            .unwrap_or_else(|| panic!("case {case:?}: no close attributes in {recorded:?}"));
        assert_eq!(
            (
                attributes.get("result").and_then(Value::as_str),
                i64_at(attributes, "/startedEventId"),
            ),
            (Some(expected), started_event_id),
            "case {case:?}: {activity_id:?} records the result its own worker reported, \
             got {attributes:?}"
        );
    }

    // Both tasks are out of flight, and the event log the readers answer from is still a
    // single monotonic sequence.
    let drained = describe(&emulator, case, domain, &run_id).await;
    assert_eq!(
        i64_at(&drained, "/openCounts/openActivityTasks"),
        0,
        "case {case:?}: both completed tasks leave the open-task count, got {drained:?}"
    );
    let ids: Vec<i64> = events
        .iter()
        .map(|event| i64_at(event, "/eventId"))
        .collect();
    assert!(
        ids.windows(2).all(|pair| pair[1] > pair[0]),
        "case {case:?}: event ids increase through the log, got {ids:?}"
    );
}
