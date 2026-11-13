// SPDX-License-Identifier: CC0-1.0

//! A worker's cancellation report for ONE activity task, end to end over the service's
//! own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: SimpleWorkflowService.<Operation>`), through the same
//! `MultiServiceRouter` the emulator mounts in production. The harness is the one
//! `workflow_execution_lifecycle` already builds.
//!
//! The rule every case here asserts: a worker's terminal report closes exactly the
//! activity task its `taskToken` is the handle on and nothing else — the
//! `ActivityTaskCanceled` event appended to that task's own run carries the `details`
//! the worker reported verbatim ("These details (if provided) appear in the
//! ActivityTaskCanceled event added to the workflow history"), names the events that
//! scheduled and started that task and the LAST `ActivityTaskCancelRequested` event
//! recorded for it ("If set, contains the ID of the last ActivityTaskCancelRequested
//! event recorded for this activity task"), takes that one task out of
//! `openCounts.openActivityTasks` and gives the decider a decision task; every other
//! task of the run stays open with its own handle live. A handle that names no OPEN
//! activity task — one already closed, or one whose run has closed — answers the
//! modelled `UnknownResourceFault` and writes nothing, and a request outside a member's
//! published length bounds is refused before the store is reached.
//!
//! The rows vary what the rule generalises over: the `details` reported (absent, the
//! member's published minimum length, text, non-ASCII, a JSON payload, the member's
//! published maximum length), how many cancellations the decider asked for before the
//! worker reported (none, one, several), which of a run's concurrent tasks the report
//! addresses, and which way a handle came to name no open task.

use serde_json::{json, Value};
use service_com_amazonaws_swf_simpleworkflowservice::ops::types::EVENT_TYPE_VALUES;

use super::workflow_execution_lifecycle::{
    error_code, full_defaults, i64_at, seeded, string_at, whole_history, Emulator,
};

/// The workflow type every run in this suite is started with.
const WORKFLOW_TYPE: (&str, &str) = ("cancel-report-wf", "1.0");

/// The `defaultTaskList` that type carries, so every run's decision tasks wait there.
const DECIDER_LIST: &str = "cancel-report-deciders";

/// The activity type every task in this suite is scheduled from.
const ACTIVITY_TYPE: (&str, &str) = ("cancel-report-activity", "1.0");

/// The activity type's `defaultTaskList`, where a scheduled activity task waits.
const ACTIVITY_LIST: &str = "cancel-report-activities";

/// The published maximum length of `RespondActivityTaskCanceled`'s `details`, from the
/// member's modelled `Data` constraint (`length 0..32768`), as
/// `smithy_query(query="operation", operation="RespondActivityTaskCanceled")` reports
/// it.
const MAX_DETAILS: usize = 32768;

/// The published minimum length of a `taskToken` (`length 1..1024`), from the same
/// query: a shorter handle is outside the modelled constraint.
const MIN_TASK_TOKEN: usize = 1;

/// The `workflowId` every run in this suite is started with.
const WORKFLOW_ID: &str = "wf-1";

/// Register the domain, the workflow type and the activity type one row uses. The task
/// durations are far longer than any of these calls, so no clock-owed timeout can close
/// a task while a row is running.
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
                "defaultTaskScheduleToStartTimeout": "3600",
                "defaultTaskScheduleToCloseTimeout": "7200",
                "defaultTaskStartToCloseTimeout": "3600",
            }),
        )
        .await;
}

/// Start one run of the seeded workflow type and return its minted `runId`.
async fn start(emulator: &Emulator, case: &str, domain: &str) -> String {
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

/// Nothing a decider has not seen is outstanding while a worker holds its task, so a
/// signal is what gives the run a decision task the decider can answer with a further
/// decision.
async fn nudge(emulator: &Emulator, case: &str, domain: &str, run_id: &str, signal: &str) {
    emulator
        .ok(
            case,
            "SignalWorkflowExecution",
            &json!({
                "domain": domain,
                "workflowId": WORKFLOW_ID,
                "runId": run_id,
                "signalName": signal,
            }),
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

/// Every event of `events` whose `eventType` is `event_type`.
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

/// The `eventId` of the one `ActivityTaskScheduled` event that scheduled `activity_id`.
fn scheduled_event_id(case: &str, events: &[Value], activity_id: &str) -> i64 {
    let scheduled: Vec<Value> = of_type(case, events, "ActivityTaskScheduled")
        .into_iter()
        .filter(|event| {
            event
                .pointer("/activityTaskScheduledEventAttributes/activityId")
                .and_then(Value::as_str)
                == Some(activity_id)
        })
        .collect();
    assert_eq!(
        scheduled.len(),
        1,
        "case {case:?}: expected exactly one scheduling event for {activity_id:?}, got \
         {scheduled:?}"
    );
    i64_at(&scheduled[0], "/eventId")
}

/// One activity task a worker holds: the handle, and the two history events that
/// scheduled and started it.
#[derive(Debug, Clone)]
struct HeldTask {
    activity_id: String,
    task_token: String,
    started_event_id: i64,
    scheduled_event_id: i64,
}

/// Have the decider schedule one activity task per `activity_ids` in a single decision,
/// then hand each of them to a worker. Returns one [`HeldTask`] per id, in the order the
/// ids were scheduled.
async fn schedule_and_hold(
    emulator: &Emulator,
    case: &str,
    domain: &str,
    run_id: &str,
    activity_ids: &[&str],
) -> Vec<HeldTask> {
    let decisions: Vec<Value> = activity_ids
        .iter()
        .map(|activity_id| {
            json!({
                "decisionType": "ScheduleActivityTask",
                "scheduleActivityTaskDecisionAttributes": {
                    "activityId": activity_id,
                    "activityType": { "name": ACTIVITY_TYPE.0, "version": ACTIVITY_TYPE.1 },
                },
            })
        })
        .collect();
    decide(emulator, case, domain, json!(decisions)).await;

    let scheduled = whole_history(emulator, domain, WORKFLOW_ID, run_id).await;
    let mut held: Vec<HeldTask> = Vec::new();
    for _ in activity_ids {
        let task = emulator
            .ok(
                case,
                "PollForActivityTask",
                &json!({ "domain": domain, "taskList": { "name": ACTIVITY_LIST } }),
            )
            .await;
        let task_token = string_at(&task, "/taskToken");
        assert!(
            !task_token.is_empty(),
            "case {case:?}: every scheduled task must be handed to a worker, got {task:?}"
        );
        let activity_id = string_at(&task, "/activityId");
        held.push(HeldTask {
            scheduled_event_id: scheduled_event_id(case, &scheduled, &activity_id),
            activity_id,
            task_token,
            started_event_id: i64_at(&task, "/startedEventId"),
        });
    }
    // The tasks are reported in the order the row named them, whichever order the list
    // handed them over in.
    let mut ordered: Vec<HeldTask> = Vec::new();
    for activity_id in activity_ids {
        let found = held
            .iter()
            .find(|task| task.activity_id == *activity_id)
            .unwrap_or_else(|| {
                panic!("case {case:?}: no task was handed over for {activity_id:?}, got {held:?}")
            });
        ordered.push(found.clone());
    }
    ordered
}

/// The `RespondActivityTaskCanceled` body for `token`, carrying `details` when the row
/// reports any.
fn cancel_body(token: &str, details: Option<&str>) -> Value {
    match details {
        Some(details) => json!({ "taskToken": token, "details": details }),
        None => json!({ "taskToken": token }),
    }
}

/// The one `ActivityTaskCanceled` event of a history, and its own attributes.
fn sole_cancellation(case: &str, events: &[Value]) -> Value {
    let canceled = of_type(case, events, "ActivityTaskCanceled");
    assert_eq!(
        canceled.len(),
        1,
        "case {case:?}: exactly one ActivityTaskCanceled event is recorded, got {events:?}"
    );
    canceled[0]
        .pointer("/activityTaskCanceledEventAttributes")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no cancellation attributes in {:?}", canceled[0]))
}

/// One worker's cancellation report, varying the `details` it carries.
struct DetailsCase {
    name: &'static str,
    domain: &'static str,
    /// The `details` reported, or `None` when the worker reports none at all. Owned so a
    /// row can be the member's published maximum length.
    details: Option<String>,
}

#[tokio::test]
async fn a_cancellation_closes_one_task_and_records_its_details_verbatim() {
    let cases = [
        DetailsCase {
            name: "no details at all",
            domain: "cancel-bare",
            details: None,
        },
        DetailsCase {
            name: "the member's published minimum length",
            domain: "cancel-empty",
            details: Some(String::new()),
        },
        DetailsCase {
            name: "a sentence of text",
            domain: "cancel-text",
            details: Some("the worker rolled its work back".to_string()),
        },
        DetailsCase {
            name: "non-ASCII details",
            domain: "cancel-utf8",
            details: Some("отменено наполовину".to_string()),
        },
        DetailsCase {
            name: "a JSON payload",
            domain: "cancel-json",
            details: Some("{\"undone\":true,\"steps\":[1,2,3]}".to_string()),
        },
        DetailsCase {
            name: "the member's published maximum length",
            domain: "cancel-max",
            details: Some("d".repeat(MAX_DETAILS)),
        },
    ];

    for case in &cases {
        let emulator = Emulator::new();
        seed(&emulator, case.domain).await;
        let run_id = start(&emulator, case.name, case.domain).await;
        // Two tasks in flight at once: the report must close the one its handle names and
        // leave the other exactly as it was.
        let held = schedule_and_hold(
            &emulator,
            case.name,
            case.domain,
            &run_id,
            &["act-reported", "act-untouched"],
        )
        .await;
        let reported = &held[0];
        let untouched = &held[1];

        let before = describe(&emulator, case.name, case.domain, &run_id).await;
        assert_eq!(
            i64_at(&before, "/openCounts/openActivityTasks"),
            2,
            "case {:?}: both tasks a worker holds are open, got {before:?}",
            case.name
        );

        let answered = emulator
            .ok(
                case.name,
                "RespondActivityTaskCanceled",
                &cancel_body(&reported.task_token, case.details.as_deref()),
            )
            .await;
        assert_eq!(
            answered,
            json!({}),
            "case {:?}: RespondActivityTaskCanceled's output shape is Unit, so the body \
             must be empty, got {answered:?}",
            case.name
        );

        let events = whole_history(&emulator, case.domain, WORKFLOW_ID, &run_id).await;
        let attributes = sole_cancellation(case.name, &events);
        assert_eq!(
            attributes.get("details"),
            case.details.as_deref().map(|details| json!(details)).as_ref(),
            "case {:?}: the event carries the details the worker reported, and none when \
             it reported none, got {attributes:?}",
            case.name
        );
        assert_eq!(
            (
                i64_at(&attributes, "/scheduledEventId"),
                i64_at(&attributes, "/startedEventId"),
                i64_at(&attributes, "/latestCancelRequestedEventId"),
            ),
            (reported.scheduled_event_id, reported.started_event_id, 0),
            "case {:?}: the event names the events that scheduled and started THIS task, \
             and the EventId default where no cancellation was asked for, got \
             {attributes:?}",
            case.name
        );

        let after = describe(&emulator, case.name, case.domain, &run_id).await;
        assert_eq!(
            (
                i64_at(&after, "/openCounts/openActivityTasks"),
                i64_at(&after, "/openCounts/openDecisionTasks"),
                string_at(&after, "/executionInfo/executionStatus"),
            ),
            (1, 1, "OPEN".to_string()),
            "case {:?}: only the reported task leaves the open count, and the decider is \
             told, got {after:?}",
            case.name
        );

        // The same handle now names a task that is closed, so a second report of it is
        // refused and records nothing.
        let (status, refused) = emulator
            .call(
                "RespondActivityTaskCanceled",
                &cancel_body(&reported.task_token, case.details.as_deref()),
            )
            .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "UnknownResourceFault"),
            "case {:?}: a second cancellation of a closed task is refused, got {status} \
             with {refused:?}",
            case.name
        );
        assert_eq!(
            refused.get("message").and_then(Value::as_str),
            Some(
                format!(
                    "Unknown activity, scheduledEventId = {}",
                    reported.scheduled_event_id
                )
                .as_str()
            ),
            "case {:?}: the refusal names the activity task the handle was on, got \
             {refused:?}",
            case.name
        );
        let after_refusal = whole_history(&emulator, case.domain, WORKFLOW_ID, &run_id).await;
        assert_eq!(
            after_refusal, events,
            "case {:?}: the refused report records nothing at all, got {after_refusal:?} \
             after {events:?}",
            case.name
        );

        // The other task's handle is untouched: it is still open, so its own worker can
        // still report it canceled.
        emulator
            .ok(
                case.name,
                "RespondActivityTaskCanceled",
                &cancel_body(&untouched.task_token, None),
            )
            .await;
        let drained = describe(&emulator, case.name, case.domain, &run_id).await;
        assert_eq!(
            i64_at(&drained, "/openCounts/openActivityTasks"),
            0,
            "case {:?}: the second task's own handle closes it too, got {drained:?}",
            case.name
        );
        let both = whole_history(&emulator, case.domain, WORKFLOW_ID, &run_id).await;
        let closes = of_type(case.name, &both, "ActivityTaskCanceled");
        assert_eq!(
            closes
                .iter()
                .map(|event| i64_at(event, "/activityTaskCanceledEventAttributes/startedEventId"))
                .collect::<Vec<i64>>(),
            vec![reported.started_event_id, untouched.started_event_id],
            "case {:?}: each report closed its own task, in the order they were made, got \
             {closes:?}",
            case.name
        );
    }
}

#[tokio::test]
async fn the_cancellation_names_the_last_request_recorded_for_that_task() {
    // How many times the decider asked this task to cancel before the worker reported
    // back: none (the worker cancelled a task nobody asked about), one, and several — the
    // case the "LAST ActivityTaskCancelRequested event recorded for this activity task"
    // wording is about.
    for requests in [0_usize, 1, 2, 3] {
        let case = format!("{requests} cancellation request(s)");
        let emulator = Emulator::new();
        let domain = format!("cancel-asked-{requests}");
        seed(&emulator, &domain).await;
        let run_id = start(&emulator, &case, &domain).await;
        let held = schedule_and_hold(&emulator, &case, &domain, &run_id, &["act-1"]).await;
        let task = &held[0];

        for request in 0..requests {
            nudge(
                &emulator,
                &case,
                &domain,
                &run_id,
                &format!("please-cancel-{request}"),
            )
            .await;
            decide(
                &emulator,
                &case,
                &domain,
                json!([{
                    "decisionType": "RequestCancelActivityTask",
                    "requestCancelActivityTaskDecisionAttributes": {
                        "activityId": task.activity_id,
                    },
                }]),
            )
            .await;
            let asked = describe(&emulator, &case, &domain, &run_id).await;
            assert_eq!(
                i64_at(&asked, "/openCounts/openActivityTasks"),
                1,
                "case {case:?}: asking a started task to cancel does not close it — its \
                 worker reports back — got {asked:?}"
            );
        }

        let before = whole_history(&emulator, &domain, WORKFLOW_ID, &run_id).await;
        let asks = of_type(&case, &before, "ActivityTaskCancelRequested");
        assert_eq!(
            asks.len(),
            requests,
            "case {case:?}: every request the decider made is recorded, got {asks:?}"
        );
        let last_ask = asks.last().map_or(0, |event| i64_at(event, "/eventId"));

        emulator
            .ok(
                &case,
                "RespondActivityTaskCanceled",
                &cancel_body(&task.task_token, Some("reported back")),
            )
            .await;

        let events = whole_history(&emulator, &domain, WORKFLOW_ID, &run_id).await;
        let attributes = sole_cancellation(&case, &events);
        assert_eq!(
            i64_at(&attributes, "/latestCancelRequestedEventId"),
            last_ask,
            "case {case:?}: the event names the last request recorded for this task, or \
             the EventId default where none was, got {attributes:?}"
        );
        assert_eq!(
            (
                i64_at(&attributes, "/scheduledEventId"),
                i64_at(&attributes, "/startedEventId"),
            ),
            (task.scheduled_event_id, task.started_event_id),
            "case {case:?}: the event still names the task's own scheduling and starting \
             events, got {attributes:?}"
        );
    }
}

#[tokio::test]
async fn a_report_outside_the_published_bounds_is_refused_and_leaves_the_task_open() {
    let emulator = Emulator::new();
    let domain = "cancel-bounds";
    seed(&emulator, domain).await;
    let run_id = start(&emulator, "bounds", domain).await;
    let held = schedule_and_hold(&emulator, "bounds", domain, &run_id, &["act-1"]).await;
    let task = &held[0];
    let before = whole_history(&emulator, domain, WORKFLOW_ID, &run_id).await;

    let over_long_details = "d".repeat(MAX_DETAILS + 1);
    // One character below the minimum: the empty handle, for the modelled minimum of 1.
    let short_token = "t".repeat(MIN_TASK_TOKEN.saturating_sub(1));
    let rows = [
        (
            "details one past the member's published maximum",
            cancel_body(&task.task_token, Some(&over_long_details)),
        ),
        (
            "a taskToken one below the member's published minimum",
            cancel_body(&short_token, None),
        ),
    ];
    for (case, body) in &rows {
        emulator
            .refused(case, "RespondActivityTaskCanceled", body, "ValidationException")
            .await;
        let after = whole_history(&emulator, domain, WORKFLOW_ID, &run_id).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused report records nothing at all, got {after:?} after \
             {before:?}"
        );
    }

    // The task the refusals addressed is still open, so a report at the bound closes it.
    let open = describe(&emulator, "bounds", domain, &run_id).await;
    assert_eq!(
        i64_at(&open, "/openCounts/openActivityTasks"),
        1,
        "case \"bounds\": the refusals left the task open, got {open:?}"
    );
    emulator
        .ok(
            "bounds",
            "RespondActivityTaskCanceled",
            &cancel_body(&task.task_token, Some(&"d".repeat(MAX_DETAILS))),
        )
        .await;
    let closed = describe(&emulator, "bounds", domain, &run_id).await;
    assert_eq!(
        i64_at(&closed, "/openCounts/openActivityTasks"),
        0,
        "case \"bounds\": a report at the published bound closes the task, got {closed:?}"
    );
}

#[tokio::test]
async fn a_handle_whose_run_has_closed_names_no_open_activity_task() {
    // The other way a live handle goes stale: the task is still outstanding, but the run
    // it belongs to is closed, so there is no open activity task behind the handle either.
    let emulator = Emulator::new();
    let domain = "cancel-closed-run";
    seed(&emulator, domain).await;
    let run_id = start(&emulator, "closed run", domain).await;
    let held = schedule_and_hold(&emulator, "closed run", domain, &run_id, &["act-1"]).await;
    let task = &held[0];

    emulator
        .ok(
            "closed run",
            "TerminateWorkflowExecution",
            &json!({
                "domain": domain,
                "workflowId": WORKFLOW_ID,
                "runId": run_id,
                "reason": "operator stopped it",
            }),
        )
        .await;
    let before = whole_history(&emulator, domain, WORKFLOW_ID, &run_id).await;

    let (status, refused) = emulator
        .call(
            "RespondActivityTaskCanceled",
            &cancel_body(&task.task_token, Some("too late")),
        )
        .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "UnknownResourceFault"),
        "case \"closed run\": a handle on a task whose run has closed is refused, got \
         {status} with {refused:?}"
    );
    assert_eq!(
        refused.get("message").and_then(Value::as_str),
        Some(
            format!(
                "Unknown activity, scheduledEventId = {}",
                task.scheduled_event_id
            )
            .as_str()
        ),
        "case \"closed run\": the refusal names the activity task the handle was on, got \
         {refused:?}"
    );
    let after = whole_history(&emulator, domain, WORKFLOW_ID, &run_id).await;
    assert_eq!(
        after, before,
        "case \"closed run\": the refused report records nothing on the closed run, got \
         {after:?} after {before:?}"
    );
    assert!(
        of_type("closed run", &after, "ActivityTaskCanceled").is_empty(),
        "case \"closed run\": no cancellation is recorded for a closed run, got {after:?}"
    );
}
