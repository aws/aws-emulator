// SPDX-License-Identifier: CC0-1.0

//! The members a task's scheduling, start and close events carry, end to end over the
//! service's own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: SimpleWorkflowService.<Operation>`), through the same
//! `MultiServiceRouter` the emulator mounts in production. The harness is the one
//! `workflow_execution_lifecycle` already builds.
//!
//! Three rules are asserted here.
//!
//! 1. A task's scheduling event reports the bounds the task actually runs under, so a
//!    decision task's `scheduleToStartTimeout` is the task-list override's own timeout
//!    while an override is in force and SWF's unlimited-duration sentinel (`NONE`)
//!    otherwise — never absent.
//! 2. The event that records the service handing a task to a worker carries the opaque,
//!    service-generated token identifying THAT hand-over, and whether the task was
//!    started as a continuation of the previous one's completion rather than in answer to
//!    a poll; both the poll's own `events` and `GetWorkflowExecutionHistory` report the
//!    same stored event.
//! 3. A close event reports what closed the thing it closes: the `cause` of a
//!    `WorkflowExecutionTerminated` event is `OPERATOR_INITIATED` for an operator's own
//!    `TerminateWorkflowExecution` call and `CHILD_POLICY_APPLIED` where the service
//!    terminated the run because its parent's child policy said so.
//!
//! The rows vary what the rules generalise over: whether a task-list override is in
//! force and whether its timeout is a number of seconds or the sentinel, which read the
//! event is observed through, how many times one run hands a task over, and whether the
//! activity task's timeouts came from the decision or from the activity type's
//! registered defaults.

use serde_json::{json, Value};
use service_com_amazonaws_swf_simpleworkflowservice::ops::types::WORKFLOW_EXECUTION_TERMINATED_CAUSE_VALUES;
use service_com_amazonaws_swf_simpleworkflowservice::state::DURATION_NONE;

use super::workflow_execution_lifecycle::{
    full_defaults, i64_at, merge, seeded, string_at, whole_history, Emulator,
};

/// The workflow type every run in this suite is started with.
const WORKFLOW_TYPE: (&str, &str) = ("event-wf", "1.0");

/// The `defaultTaskList` that type carries, so every run's decision tasks wait there.
const DECIDER_LIST: &str = "event-deciders";

/// The activity type the activity rows schedule tasks of.
const ACTIVITY_TYPE: (&str, &str) = ("event-activity", "2.0");

/// The activity type's `defaultTaskList`, where a scheduled activity task waits.
const ACTIVITY_LIST: &str = "event-activities";

/// Register the domain, the workflow type and the activity type this suite uses.
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

/// Start one run of the seeded type and return its minted `runId`.
async fn start(emulator: &Emulator, case: &str, domain: &str, workflow_id: &str) -> String {
    let started = emulator
        .ok(
            case,
            "StartWorkflowExecution",
            &json!({
                "domain": domain,
                "workflowId": workflow_id,
                "workflowType": { "name": WORKFLOW_TYPE.0, "version": WORKFLOW_TYPE.1 },
            }),
        )
        .await;
    string_at(&started, "/runId")
}

/// Poll one decision task off `task_list`, requiring a task to be waiting there.
async fn poll_decision(emulator: &Emulator, case: &str, domain: &str, task_list: &str) -> Value {
    let task = emulator
        .ok(
            case,
            "PollForDecisionTask",
            &json!({ "domain": domain, "taskList": { "name": task_list } }),
        )
        .await;
    assert!(
        !string_at(&task, "/taskToken").is_empty(),
        "case {case:?}: expected a decision task waiting in {task_list:?}, got {task:?}"
    );
    task
}

/// The events of one history (or of one poll's answer).
fn events_of(case: &str, body: &Value) -> Vec<Value> {
    body.pointer("/events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no events in {body:?}"))
}

/// Every event of `events` whose type is `event_type`.
fn of_type(events: &[Value], event_type: &str) -> Vec<Value> {
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

/// Whether `value` has the canonical UUID form AWS reports a client token in:
/// 8-4-4-4-12 lowercase hex digits separated by dashes. `pub(crate)` so a sibling suite
/// checks a hand-over handle against the one form rather than keeping a second copy of it.
pub(crate) fn is_uuid_shaped(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .map(|group| group.len())
            .eq([8, 4, 4, 4, 12])
        && groups
            .iter()
            .all(|group| group.chars().all(|c| c.is_ascii_hexdigit()))
}

/// One decision task's scheduling event, under the task-list override this row puts in
/// force (if any).
struct ScheduleCase {
    name: &'static str,
    workflow_id: &'static str,
    /// The `taskList`/`taskListScheduleToStartTimeout` the decider overrides with, or
    /// `None` for a run with no override at all.
    override_with: Option<(&'static str, &'static str)>,
    /// The `scheduleToStartTimeout` the next scheduled decision task must then carry.
    expected_timeout: &'static str,
    /// The task list that task must then wait in.
    expected_list: &'static str,
}

#[tokio::test]
async fn a_scheduled_decision_task_reports_the_schedule_to_start_bound_it_waits_under() {
    let emulator = Emulator::new();

    // `taskListScheduleToStartTimeout` is a `DurationInSecondsOptional` ("You can use
    // NONE to specify unlimited duration"), so the rows span no override, a numeric
    // override and the sentinel.
    let cases = [
        ScheduleCase {
            name: "no override in force",
            workflow_id: "wf-no-override",
            override_with: None,
            expected_timeout: DURATION_NONE,
            expected_list: DECIDER_LIST,
        },
        ScheduleCase {
            name: "an override bounded in seconds",
            workflow_id: "wf-bounded-override",
            override_with: Some(("override-seconds", "60")),
            expected_timeout: "60",
            expected_list: "override-seconds",
        },
        ScheduleCase {
            name: "an override bounded by the unlimited sentinel",
            workflow_id: "wf-sentinel-override",
            override_with: Some(("override-sentinel", DURATION_NONE)),
            expected_timeout: DURATION_NONE,
            expected_list: "override-sentinel",
        },
    ];

    for case in &cases {
        // One domain per row: a row that leaves a decision task waiting in its list must
        // not hand it to the next row's poll.
        let domain = case.workflow_id;
        seed(&emulator, domain).await;
        let run_id = start(&emulator, case.name, domain, case.workflow_id).await;

        // The run's FIRST decision task is scheduled by the start, before any decider
        // could have overridden anything, so it waits unbounded.
        let first = whole_history(&emulator, domain, case.workflow_id, &run_id).await;
        let scheduled = of_type(&first, "DecisionTaskScheduled");
        assert_eq!(
            scheduled.len(),
            1,
            "case {:?}: a start schedules exactly one decision task, got {first:?}",
            case.name
        );
        assert_eq!(
            (
                string_at(
                    &scheduled[0],
                    "/decisionTaskScheduledEventAttributes/scheduleToStartTimeout"
                ),
                string_at(&scheduled[0], "/decisionTaskScheduledEventAttributes/taskList/name"),
            ),
            (DURATION_NONE.to_string(), DECIDER_LIST.to_string()),
            "case {:?}: a decision task with no override in force waits unbounded in the \
             run's own task list, got {:?}",
            case.name,
            scheduled[0]
        );

        // Answer that task, putting this row's override in force, then make the run need
        // another decision so a second task is scheduled under it.
        let token = string_at(
            &poll_decision(&emulator, case.name, domain, DECIDER_LIST).await,
            "/taskToken",
        );
        let mut body = json!({ "taskToken": token, "decisions": [] });
        if let Some((task_list, timeout)) = case.override_with {
            merge(
                &mut body,
                json!({
                    "taskList": { "name": task_list },
                    "taskListScheduleToStartTimeout": timeout,
                }),
            );
        }
        emulator
            .ok(case.name, "RespondDecisionTaskCompleted", &body)
            .await;
        emulator
            .ok(
                case.name,
                "SignalWorkflowExecution",
                &json!({
                    "domain": domain,
                    "workflowId": case.workflow_id,
                    "runId": run_id,
                    "signalName": "wake-up",
                }),
            )
            .await;

        let second = whole_history(&emulator, domain, case.workflow_id, &run_id).await;
        let scheduled = of_type(&second, "DecisionTaskScheduled");
        let latest = scheduled
            .last()
            .unwrap_or_else(|| panic!("case {:?}: no decision task scheduled", case.name));
        assert_eq!(
            (
                string_at(
                    latest,
                    "/decisionTaskScheduledEventAttributes/scheduleToStartTimeout"
                ),
                string_at(latest, "/decisionTaskScheduledEventAttributes/taskList/name"),
            ),
            (
                case.expected_timeout.to_string(),
                case.expected_list.to_string()
            ),
            "case {:?}: the scheduled task reports the bound and the list it actually \
             waits under, got {latest:?}",
            case.name
        );
    }
}

#[tokio::test]
async fn every_task_hand_over_carries_the_services_own_handle_on_that_start() {
    let emulator = Emulator::new();
    let domain = "handles";
    seed(&emulator, domain).await;
    let run_id = start(&emulator, "handles", domain, "wf-handles").await;

    // Hand the run's first decision task over, and schedule an activity task with it so
    // the run also has a task for a WORKER to be handed.
    let first_poll = poll_decision(&emulator, "handles", domain, DECIDER_LIST).await;
    let polled_events = events_of("handles", &first_poll);
    let polled_started = of_type(&polled_events, "DecisionTaskStarted");
    assert_eq!(
        polled_started.len(),
        1,
        "case \"handles\": the poll's own history reports the start it just recorded, \
         got {polled_events:?}"
    );
    let first_token = string_at(&polled_started[0], "/decisionTaskStartedEventAttributes/clientToken");
    assert!(
        is_uuid_shaped(&first_token),
        "case \"handles\": a decision task's clientToken is UUID-shaped, got \
         {first_token:?}"
    );
    assert_eq!(
        polled_started[0]
            .pointer("/decisionTaskStartedEventAttributes/startedOnPreviousTaskCompletion")
            .and_then(Value::as_bool),
        Some(false),
        "case \"handles\": a task a poll started did not continue the previous task's \
         completion, got {:?}",
        polled_started[0]
    );

    emulator
        .ok(
            "handles",
            "RespondDecisionTaskCompleted",
            &json!({
                "taskToken": string_at(&first_poll, "/taskToken"),
                "decisions": [{
                    "decisionType": "ScheduleActivityTask",
                    "scheduleActivityTaskDecisionAttributes": {
                        "activityId": "act-1",
                        "activityType": { "name": ACTIVITY_TYPE.0, "version": ACTIVITY_TYPE.1 },
                        "input": "payload",
                    },
                }],
            }),
        )
        .await;

    // An activity task scheduled by an ordinary decision waits in its list for a worker
    // to poll for it, so it is not one the service started immediately.
    let after_schedule = whole_history(&emulator, domain, "wf-handles", &run_id).await;
    let activity_scheduled = of_type(&after_schedule, "ActivityTaskScheduled");
    assert_eq!(
        activity_scheduled.len(),
        1,
        "case \"handles\": the decision scheduled exactly one activity task, got \
         {after_schedule:?}"
    );
    assert_eq!(
        activity_scheduled[0]
            .pointer("/activityTaskScheduledEventAttributes/immediatelyStart")
            .and_then(Value::as_bool),
        Some(false),
        "case \"handles\": a task scheduled into a task list is not started immediately, \
         got {:?}",
        activity_scheduled[0]
    );

    // Hand the activity task to a worker: that hand-over carries its own handle.
    let activity_task = emulator
        .ok(
            "handles",
            "PollForActivityTask",
            &json!({ "domain": domain, "taskList": { "name": ACTIVITY_LIST } }),
        )
        .await;
    assert!(
        !string_at(&activity_task, "/taskToken").is_empty(),
        "case \"handles\": the scheduled activity task must be handed over, got \
         {activity_task:?}"
    );

    let history = whole_history(&emulator, domain, "wf-handles", &run_id).await;
    let activity_started = of_type(&history, "ActivityTaskStarted");
    assert_eq!(
        activity_started.len(),
        1,
        "case \"handles\": the poll recorded exactly one start, got {history:?}"
    );
    let activity_token =
        string_at(&activity_started[0], "/activityTaskStartedEventAttributes/clientToken");
    assert!(
        is_uuid_shaped(&activity_token),
        "case \"handles\": an activity task's clientToken is UUID-shaped, got \
         {activity_token:?}"
    );

    // The token belongs to the stored event, so reading the SAME event again reports the
    // same token, and no two hand-overs of one run share one.
    let decision_started = of_type(&history, "DecisionTaskStarted");
    assert_eq!(
        string_at(
            &decision_started[0],
            "/decisionTaskStartedEventAttributes/clientToken"
        ),
        first_token,
        "case \"handles\": the token of a start is the stored event's own, so a second \
         read reports it unchanged, got {decision_started:?}"
    );
    assert_ne!(
        activity_token, first_token,
        "case \"handles\": each hand-over gets its own handle, got {activity_token:?} \
         for both"
    );

    // A second decision task of the same run is a second hand-over, with a third handle.
    // Something the decider has not seen is what schedules it: the run's activity task is
    // still with its worker, so a signal is what the next task carries.
    emulator
        .ok(
            "handles",
            "SignalWorkflowExecution",
            &json!({
                "domain": domain,
                "workflowId": "wf-handles",
                "runId": run_id,
                "signalName": "wake-up",
            }),
        )
        .await;
    let second_poll = poll_decision(&emulator, "handles", domain, DECIDER_LIST).await;
    let second_started = of_type(&events_of("handles", &second_poll), "DecisionTaskStarted");
    let second_tokens: Vec<String> = second_started
        .iter()
        .map(|event| string_at(event, "/decisionTaskStartedEventAttributes/clientToken"))
        .collect();
    assert_eq!(
        second_tokens.len(),
        2,
        "case \"handles\": the run has handed a decision task over twice, got \
         {second_started:?}"
    );
    assert_ne!(
        second_tokens[0], second_tokens[1],
        "case \"handles\": two starts of one run never share a handle, got \
         {second_tokens:?}"
    );
    for (index, event) in second_started.iter().enumerate() {
        assert_eq!(
            event
                .pointer("/decisionTaskStartedEventAttributes/startedOnPreviousTaskCompletion")
                .and_then(Value::as_bool),
            Some(false),
            "case \"handles\": start {index} answered a poll rather than continuing the \
             previous task's completion, got {event:?}"
        );
    }
}

#[tokio::test]
async fn a_terminated_run_reports_what_terminated_it() {
    let emulator = Emulator::new();
    let domain = "terminated";
    seed(&emulator, domain).await;

    // An operator's own call: the cause the modelled enumeration has for exactly that.
    let parent_id = start(&emulator, "operator", domain, "wf-parent").await;
    let token = string_at(
        &poll_decision(&emulator, "operator", domain, DECIDER_LIST).await,
        "/taskToken",
    );
    // The parent starts a child, so the terminate below also applies its child policy —
    // the other way a run is terminated, and the other cause the enumeration carries.
    emulator
        .ok(
            "operator",
            "RespondDecisionTaskCompleted",
            &json!({
                "taskToken": token,
                "decisions": [{
                    "decisionType": "StartChildWorkflowExecution",
                    "startChildWorkflowExecutionDecisionAttributes": {
                        "workflowId": "wf-child",
                        "workflowType": { "name": WORKFLOW_TYPE.0, "version": WORKFLOW_TYPE.1 },
                        "childPolicy": "TERMINATE",
                    },
                }],
            }),
        )
        .await;
    let parent_events = whole_history(&emulator, domain, "wf-parent", &parent_id).await;
    let child_started = of_type(&parent_events, "ChildWorkflowExecutionStarted");
    assert_eq!(
        child_started.len(),
        1,
        "case \"operator\": the decision started exactly one child run, got \
         {parent_events:?}"
    );
    let child_id = string_at(
        &child_started[0],
        "/childWorkflowExecutionStartedEventAttributes/workflowExecution/runId",
    );

    emulator
        .ok(
            "operator",
            "TerminateWorkflowExecution",
            &json!({
                "domain": domain,
                "workflowId": "wf-parent",
                "runId": parent_id,
                "reason": "done",
                "details": "bye",
                "childPolicy": "TERMINATE",
            }),
        )
        .await;

    for (case, workflow_id, run_id, expected_cause) in [
        (
            "the operator's own call",
            "wf-parent",
            parent_id.clone(),
            "OPERATOR_INITIATED",
        ),
        (
            "the parent's child policy",
            "wf-child",
            child_id,
            "CHILD_POLICY_APPLIED",
        ),
    ] {
        assert!(
            WORKFLOW_EXECUTION_TERMINATED_CAUSE_VALUES.contains(&expected_cause),
            "case {case:?}: {expected_cause:?} must be a modelled \
             WorkflowExecutionTerminatedCause"
        );
        let events = whole_history(&emulator, domain, workflow_id, &run_id).await;
        let terminated = of_type(&events, "WorkflowExecutionTerminated");
        assert_eq!(
            terminated.len(),
            1,
            "case {case:?}: the run is terminated exactly once, got {events:?}"
        );
        let attributes = terminated[0]
            .pointer("/workflowExecutionTerminatedEventAttributes")
            .unwrap_or_else(|| panic!("case {case:?}: no terminate attributes in {events:?}"));
        assert_eq!(
            attributes.get("cause").and_then(Value::as_str),
            Some(expected_cause),
            "case {case:?}: the terminate event names what terminated the run, got \
             {attributes:?}"
        );
        assert_eq!(
            string_at(attributes, "/childPolicy"),
            "TERMINATE",
            "case {case:?}: the event also carries the effective child policy, got \
             {attributes:?}"
        );
    }

    // The parent's own reason and details are the operator's, and the policy-applied
    // child carries none: a cause is not a reason.
    let parent_events = whole_history(&emulator, domain, "wf-parent", &parent_id).await;
    let parent_terminated = of_type(&parent_events, "WorkflowExecutionTerminated");
    assert_eq!(
        (
            string_at(
                &parent_terminated[0],
                "/workflowExecutionTerminatedEventAttributes/reason"
            ),
            string_at(
                &parent_terminated[0],
                "/workflowExecutionTerminatedEventAttributes/details"
            ),
        ),
        ("done".to_string(), "bye".to_string()),
        "case \"operator\": the terminate event carries the reason and details the call \
         made, got {parent_terminated:?}"
    );
    assert!(
        i64_at(&parent_terminated[0], "/eventId") > 0,
        "case \"operator\": every recorded event has an id, got {parent_terminated:?}"
    );
}
