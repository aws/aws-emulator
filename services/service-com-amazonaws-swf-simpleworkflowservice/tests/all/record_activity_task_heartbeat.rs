// SPDX-License-Identifier: CC0-1.0

//! An activity worker's progress report, end to end over the service's own wire
//! protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: SimpleWorkflowService.<Operation>`), through the same
//! `MultiServiceRouter` the emulator mounts in production. The harness is the one
//! `workflow_execution_lifecycle` already builds.
//!
//! The rule every case here asserts: `RecordActivityTaskHeartbeat` reports progress on
//! the activity task its `taskToken` is the handle on and does exactly two things —
//! it "resets the taskHeartbeatTimeout clock", so that task's own `heartbeatTimeout`
//! runs from this call and the `details` it carried is what an `ActivityTaskTimedOut`
//! event reports ("the content of the details parameter for the last call made by the
//! activity to RecordActivityTaskHeartbeat"), and it answers `cancelRequested`, "set to
//! true if cancellation of the task is requested". It "doesn't in itself create an event
//! in the workflow execution history", so the run's event log and open counts are left
//! as they were, and a handle on a task that is no longer open answers the modelled
//! `UnknownResourceFault`.
//!
//! The rows vary what the rule generalises over: the `details` reported (absent, empty,
//! text, non-ASCII, the member's published maximum length), how many reports the worker
//! makes before the budget would have run out, whether the last of them carried details
//! at all, where the task's `heartbeatTimeout` came from (the activity type's registered
//! default or the scheduling decision's override), whether a cancellation has been asked
//! for, and which way a handle came to name no open task.

use std::time::Duration;

use serde_json::{json, Value};

use super::workflow_execution_lifecycle::{
    error_code, full_defaults, i64_at, seeded, string_at, whole_history, Emulator,
};

/// The workflow type every run in this suite is started with.
const WORKFLOW_TYPE: (&str, &str) = ("beat-wf", "1.0");

/// The `defaultTaskList` that type carries, so every run's decision tasks wait there.
const DECIDER_LIST: &str = "beat-deciders";

/// The activity type every task in this suite is scheduled from.
const ACTIVITY_TYPE: (&str, &str) = ("beat-activity", "1.0");

/// The activity type's `defaultTaskList`, where a scheduled activity task waits.
const ACTIVITY_LIST: &str = "beat-activities";

/// The published maximum length of `RecordActivityTaskHeartbeat`'s `details`, from the
/// member's modelled `LimitedData` constraint (`length 0..2048`), as
/// `smithy_query(query="operation", operation="RecordActivityTaskHeartbeat")` reports
/// it.
const MAX_DETAILS: usize = 2048;

/// The slack added to a wait so a deadline is certainly in the past by the time the run
/// is read back. The sweep is driven by the wall clock, so a wait only has to cross the
/// deadline, never match it.
const SLACK: Duration = Duration::from_millis(400);

/// Register the domain, the workflow type and the activity type one row uses.
/// `heartbeat` is the activity type's `defaultTaskHeartbeatTimeout`; the other task
/// durations are far longer than any wait here, so the heartbeat budget is the only one
/// that can run out.
async fn seed(emulator: &Emulator, domain: &str, heartbeat: &str) {
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
                "defaultTaskHeartbeatTimeout": heartbeat,
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

/// Every event of `events` whose `eventType` is `event_type`.
fn of_type(events: &[Value], event_type: &str) -> Vec<Value> {
    events
        .iter()
        .filter(|event| event.get("eventType").and_then(Value::as_str) == Some(event_type))
        .cloned()
        .collect()
}

/// Start a run, have the decider schedule one activity task (overriding its
/// `heartbeatTimeout` when the row asks for it) and hand that task to a worker. Returns
/// `(runId, taskToken)`.
async fn task_in_hand(
    emulator: &Emulator,
    case: &str,
    domain: &str,
    scheduled_heartbeat: Option<&str>,
) -> (String, String) {
    let started = emulator
        .ok(
            case,
            "StartWorkflowExecution",
            &json!({
                "domain": domain,
                "workflowId": "wf-1",
                "workflowType": { "name": WORKFLOW_TYPE.0, "version": WORKFLOW_TYPE.1 },
            }),
        )
        .await;
    let run_id = string_at(&started, "/runId");

    let mut attributes = json!({
        "activityId": "act-1",
        "activityType": { "name": ACTIVITY_TYPE.0, "version": ACTIVITY_TYPE.1 },
    });
    if let (Some(target), Some(timeout)) = (attributes.as_object_mut(), scheduled_heartbeat) {
        target.insert("heartbeatTimeout".to_string(), json!(timeout));
    }
    decide(
        emulator,
        case,
        domain,
        json!([{
            "decisionType": "ScheduleActivityTask",
            "scheduleActivityTaskDecisionAttributes": attributes,
        }]),
    )
    .await;

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
    (run_id, token)
}

/// The heartbeat request body for `token`, carrying `details` when the row reports any.
fn beat_body(token: &str, details: Option<&str>) -> Value {
    let mut body = json!({ "taskToken": token });
    if let (Some(target), Some(details)) = (body.as_object_mut(), details) {
        target.insert("details".to_string(), json!(details));
    }
    body
}

/// One worker's progress report, varying the `details` it carries.
struct BeatCase {
    name: &'static str,
    domain: &'static str,
    /// The `details` the worker reports, or `None` when it reports none at all. An
    /// owned string so a row can be the member's maximum length.
    details: Option<String>,
}

#[tokio::test]
async fn a_heartbeat_answers_from_state_and_records_no_event() {
    let cases = [
        BeatCase {
            name: "no details at all",
            domain: "beat-bare",
            details: None,
        },
        BeatCase {
            name: "the member's published minimum length",
            domain: "beat-empty",
            details: Some(String::new()),
        },
        BeatCase {
            name: "a progress report",
            domain: "beat-text",
            details: Some("50% complete".to_string()),
        },
        BeatCase {
            name: "non-ASCII details",
            domain: "beat-utf8",
            details: Some("наполовину готово".to_string()),
        },
        BeatCase {
            name: "the member's published maximum length",
            domain: "beat-max",
            details: Some("d".repeat(MAX_DETAILS)),
        },
    ];

    for case in &cases {
        let emulator = Emulator::new();
        seed(&emulator, case.domain, "600").await;
        let (run_id, token) = task_in_hand(&emulator, case.name, case.domain, None).await;

        let before = whole_history(&emulator, case.domain, "wf-1", &run_id).await;
        let held = describe(&emulator, case.name, case.domain, &run_id).await;

        // Nobody has asked this task to cancel, so the flag the worker checks is false.
        let answered = emulator
            .ok(
                case.name,
                "RecordActivityTaskHeartbeat",
                &beat_body(&token, case.details.as_deref()),
            )
            .await;
        assert_eq!(
            answered.get("cancelRequested"),
            Some(&json!(false)),
            "case {:?}: with no cancellation asked for, cancelRequested is false, \
             got {answered:?}",
            case.name
        );

        // "This action doesn't in itself create an event in the workflow execution
        // history": the log and the open counts are exactly what they were.
        let after = whole_history(&emulator, case.domain, "wf-1", &run_id).await;
        assert_eq!(
            after, before,
            "case {:?}: a heartbeat records no event at all, got {after:?} after \
             {before:?}",
            case.name
        );
        let beaten = describe(&emulator, case.name, case.domain, &run_id).await;
        assert_eq!(
            (
                i64_at(&beaten, "/openCounts/openActivityTasks"),
                i64_at(&beaten, "/openCounts/openDecisionTasks"),
                string_at(&beaten, "/executionInfo/executionStatus"),
            ),
            (
                i64_at(&held, "/openCounts/openActivityTasks"),
                i64_at(&held, "/openCounts/openDecisionTasks"),
                string_at(&held, "/executionInfo/executionStatus"),
            ),
            "case {:?}: a heartbeat leaves the task open and the decider untold, \
             got {beaten:?} after {held:?}",
            case.name
        );

        // The decider asks the running task to cancel. A task a worker already holds is
        // told through its heartbeat, so the next report of the SAME handle answers
        // true — the flag is read from the run's history, not from the request.
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
        let asked = whole_history(&emulator, case.domain, "wf-1", &run_id).await;
        let told = emulator
            .ok(
                case.name,
                "RecordActivityTaskHeartbeat",
                &beat_body(&token, case.details.as_deref()),
            )
            .await;
        assert_eq!(
            told.get("cancelRequested"),
            Some(&json!(true)),
            "case {:?}: once a cancellation is asked for, the heartbeat reports it, \
             got {told:?}",
            case.name
        );
        let after_told = whole_history(&emulator, case.domain, "wf-1", &run_id).await;
        assert_eq!(
            after_told, asked,
            "case {:?}: the heartbeat that reported the cancellation still records \
             nothing, got {after_told:?} after {asked:?}",
            case.name
        );
        assert_eq!(
            i64_at(
                &describe(&emulator, case.name, case.domain, &run_id).await,
                "/openCounts/openActivityTasks"
            ),
            1,
            "case {:?}: the task stays open until its worker reports back",
            case.name
        );

        // The worker honours the request, which is the one thing that closes the task:
        // the handle it heartbeated with is still live.
        emulator
            .ok(
                case.name,
                "RespondActivityTaskCanceled",
                &json!({ "taskToken": token }),
            )
            .await;
        let closed = whole_history(&emulator, case.domain, "wf-1", &run_id).await;
        let canceled = of_type(&closed, "ActivityTaskCanceled");
        assert_eq!(
            canceled.len(),
            1,
            "case {:?}: the worker's own report closes the task, got {closed:?}",
            case.name
        );
        let requested = of_type(&closed, "ActivityTaskCancelRequested");
        assert_eq!(
            i64_at(
                &canceled[0],
                "/activityTaskCanceledEventAttributes/latestCancelRequestedEventId"
            ),
            i64_at(&requested[0], "/eventId"),
            "case {:?}: the close names the cancellation the heartbeat reported, \
             got {canceled:?}",
            case.name
        );

        // And a handle on a task that has closed names no open activity task.
        let (status, refused) = emulator
            .call(
                "RecordActivityTaskHeartbeat",
                &beat_body(&token, case.details.as_deref()),
            )
            .await;
        let scheduled_event_id = i64_at(
            &of_type(&closed, "ActivityTaskScheduled")[0],
            "/eventId",
        );
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "UnknownResourceFault"),
            "case {:?}: a heartbeat for a closed task is refused, got {status} with \
             {refused:?}",
            case.name
        );
        assert_eq!(
            refused.get("message").and_then(Value::as_str),
            Some(format!("Unknown activity, scheduledEventId = {scheduled_event_id}").as_str()),
            "case {:?}: the refusal names the activity task the handle was on, \
             got {refused:?}",
            case.name
        );
    }
}

/// One task's heartbeat budget and the reports its worker makes against it.
struct ClockCase {
    name: &'static str,
    domain: &'static str,
    /// The activity type's `defaultTaskHeartbeatTimeout`.
    registered: &'static str,
    /// What the `ScheduleActivityTask` decision overrides it with, if anything.
    scheduled: Option<&'static str>,
    /// The budget those two resolve to.
    budget: Duration,
    /// The reports the worker makes, each after 60% of the budget has passed, so two
    /// reports take longer than the budget itself: the task survives only if each one
    /// restarted the clock.
    beats: &'static [Option<&'static str>],
}

#[tokio::test]
async fn a_heartbeat_restarts_the_tasks_heartbeat_budget() {
    let cases = [
        ClockCase {
            name: "a worker that never reports",
            domain: "clock-silent",
            registered: "1",
            scheduled: None,
            budget: Duration::from_secs(1),
            beats: &[],
        },
        ClockCase {
            name: "one report, from the registered default budget",
            domain: "clock-one",
            registered: "1",
            scheduled: None,
            budget: Duration::from_secs(1),
            beats: &[Some("halfway")],
        },
        ClockCase {
            name: "two reports, on a budget the decision overrode",
            domain: "clock-override",
            registered: "600",
            scheduled: Some("1"),
            budget: Duration::from_secs(1),
            beats: &[Some("a third"), Some("двe трети")],
        },
        ClockCase {
            name: "two reports whose last one carried no details",
            domain: "clock-last-bare",
            registered: "1",
            scheduled: None,
            budget: Duration::from_secs(1),
            beats: &[Some("a third"), None],
        },
    ];

    for case in &cases {
        let emulator = Emulator::new();
        seed(&emulator, case.domain, case.registered).await;
        let (run_id, token) =
            task_in_hand(&emulator, case.name, case.domain, case.scheduled).await;
        // 60% of the budget per report: after the second one more than a whole budget
        // has passed since the task was handed over, so a task still open can only be
        // running on a clock the reports restarted.
        let step = case.budget.mul_f64(0.6);

        for (index, details) in case.beats.iter().enumerate() {
            tokio::time::sleep(step).await;
            let answered = emulator
                .ok(
                    case.name,
                    "RecordActivityTaskHeartbeat",
                    &beat_body(&token, *details),
                )
                .await;
            assert_eq!(
                answered.get("cancelRequested"),
                Some(&json!(false)),
                "case {:?}: report {index} is answered without a cancellation, \
                 got {answered:?}",
                case.name
            );
            let events = whole_history(&emulator, case.domain, "wf-1", &run_id).await;
            assert_eq!(
                of_type(&events, "ActivityTaskTimedOut"),
                Vec::<Value>::new(),
                "case {:?}: after report {index} the task's budget has been restarted, \
                 so nothing has timed out, got {events:?}",
                case.name
            );
            assert_eq!(
                i64_at(
                    &describe(&emulator, case.name, case.domain, &run_id).await,
                    "/openCounts/openActivityTasks"
                ),
                1,
                "case {:?}: after report {index} the task is still open",
                case.name
            );
        }

        // The worker goes quiet: one whole budget later the service itself times the
        // task out, under the budget the heartbeat was measured against.
        tokio::time::sleep(case.budget + SLACK).await;
        let events = whole_history(&emulator, case.domain, "wf-1", &run_id).await;
        let timed_out = of_type(&events, "ActivityTaskTimedOut");
        assert_eq!(
            timed_out.len(),
            1,
            "case {:?}: a worker that stopped reporting has its task timed out exactly \
             once, got {events:?}",
            case.name
        );
        let attributes = &timed_out[0]["activityTaskTimedOutEventAttributes"];
        assert_eq!(
            attributes.get("timeoutType"),
            Some(&json!("HEARTBEAT")),
            "case {:?}: the budget that ran out is the heartbeat one, got {attributes:?}",
            case.name
        );
        assert_eq!(
            (
                i64_at(attributes, "/scheduledEventId"),
                i64_at(attributes, "/startedEventId"),
            ),
            (
                i64_at(&of_type(&events, "ActivityTaskScheduled")[0], "/eventId"),
                i64_at(&of_type(&events, "ActivityTaskStarted")[0], "/eventId"),
            ),
            "case {:?}: the timeout names the events that scheduled and started the \
             task, got {attributes:?}",
            case.name
        );
        // "details: Contains the content of the details parameter for the last call made
        // by the activity to RecordActivityTaskHeartbeat" — including none at all, when
        // the last call carried none.
        let reported = case.beats.last().copied().flatten();
        assert_eq!(
            attributes.get("details").and_then(Value::as_str),
            reported,
            "case {:?}: the timeout reports the details of the worker's LAST report, \
             got {attributes:?}",
            case.name
        );

        // The timed-out task is out of flight, the decider is told, and the handle the
        // worker held names no open task any more.
        let described = describe(&emulator, case.name, case.domain, &run_id).await;
        assert_eq!(
            (
                i64_at(&described, "/openCounts/openActivityTasks"),
                i64_at(&described, "/openCounts/openDecisionTasks"),
                string_at(&described, "/executionInfo/executionStatus"),
            ),
            (0, 1, "OPEN".to_string()),
            "case {:?}: the timed-out task leaves the open-task count and the decider \
             is given the timeout, got {described:?}",
            case.name
        );
        let (status, refused) = emulator
            .call("RecordActivityTaskHeartbeat", &beat_body(&token, None))
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
                Some(
                    format!(
                        "Unknown activity, scheduledEventId = {}",
                        i64_at(&of_type(&events, "ActivityTaskScheduled")[0], "/eventId")
                    )
                    .as_str()
                ),
            ),
            "case {:?}: a report on a task the clock closed is refused naming that \
             task, got {status} with {refused:?}",
            case.name
        );
    }
}

#[tokio::test]
async fn a_handle_that_names_no_open_activity_task_is_refused() {
    let emulator = Emulator::new();
    let domain = "beat-stale";
    seed(&emulator, domain, "600").await;
    let (run_id, token) = task_in_hand(&emulator, "stale", domain, None).await;

    // A decision task's own token is a handle on a decision task, not on an activity
    // task, and a token the service never minted is a handle on nothing.
    emulator
        .ok(
            "stale",
            "SignalWorkflowExecution",
            &json!({
                "domain": domain,
                "workflowId": "wf-1",
                "runId": run_id,
                "signalName": "wake-the-decider",
            }),
        )
        .await;
    let decision_task = emulator
        .ok(
            "stale",
            "PollForDecisionTask",
            &json!({ "domain": domain, "taskList": { "name": DECIDER_LIST } }),
        )
        .await;
    let decision_token = string_at(&decision_task, "/taskToken");
    // A decision token is still a handle on a task of the stored history, so the refusal
    // names the decision task's own scheduling event; a token the service never minted is
    // a handle on no started task at all, so there is no scheduling event behind it and
    // the `EventId` member's own default is reported.
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

    for (case, handle, expected_scheduled_event_id) in [
        (
            "a decision task's own token",
            decision_token.as_str(),
            decision_scheduled_event_id,
        ),
        ("a token the service never minted", "not-a-token", 0),
        (
            "a token with the right shape but a wrong digest",
            &"0".repeat(64),
            0,
        ),
    ] {
        let expected_message =
            format!("Unknown activity, scheduledEventId = {expected_scheduled_event_id}");
        let (status, refused) = emulator
            .call("RecordActivityTaskHeartbeat", &beat_body(handle, None))
            .await;
        assert_eq!(
            (
                status,
                error_code(&refused).as_str(),
                refused.get("message").and_then(Value::as_str),
            ),
            (400, "UnknownResourceFault", Some(expected_message.as_str())),
            "case {case:?}: the refusal names the task the supplied handle addresses, got \
             {status} with {refused:?}"
        );
    }

    // A handle on a task of a run that has CLOSED names no open task either.
    let events = whole_history(&emulator, domain, "wf-1", &run_id).await;
    let scheduled_event_id = i64_at(&of_type(&events, "ActivityTaskScheduled")[0], "/eventId");
    emulator
        .ok(
            "stale",
            "TerminateWorkflowExecution",
            &json!({ "domain": domain, "workflowId": "wf-1", "runId": run_id }),
        )
        .await;
    let (status, refused) = emulator
        .call("RecordActivityTaskHeartbeat", &beat_body(&token, None))
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
            Some(format!("Unknown activity, scheduledEventId = {scheduled_event_id}").as_str()),
        ),
        "case \"a terminated run\": a report on a task whose run has closed is refused \
         naming that task, got {status} with {refused:?}"
    );

    // And a `details` past the member's published maximum is refused by the modelled
    // constraint, before any state is reached.
    let (status, refused) = emulator
        .call(
            "RecordActivityTaskHeartbeat",
            &beat_body(&token, Some(&"d".repeat(MAX_DETAILS + 1))),
        )
        .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "ValidationException"),
        "case \"details one past the maximum\": a report longer than the member allows \
         is refused, got {status} with {refused:?}"
    );
}
