// SPDX-License-Identifier: CC0-1.0

//! Every member this service answers with that its published model snapshot does not
//! declare, end to end over the service's own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: SimpleWorkflowService.<Operation>`), through the same
//! `MultiServiceRouter` the emulator mounts in production. The harness is the one
//! `workflow_execution_lifecycle` already builds.
//!
//! Two rules are asserted here.
//!
//! 1. A shape this service answers with members the snapshot does not declare is declared
//!    from this crate's overlay model (`smithy-model/local-additions.json`, see
//!    `smithy-model/LOCAL-ADDITIONS.md`), and that overlay restates everything else of
//!    each such shape EXACTLY as the snapshot declares it — every member against the
//!    snapshot's own target, with its own `@required` marking, default and constraint
//!    traits — adding only the members listed here and declaring, of a shape it restates
//!    only to resolve a member's target, nothing but that shape's type.
//! 2. Every one of those members reaches the wire: wherever an operation reports one of
//!    those shapes, the added member is reported with it, carrying what this service's own
//!    state says rather than anything of the request.
//!
//! The rows vary what rule 2 generalises over: which shape the record is (a reported tag,
//! a task's scheduling and start events, an execution record, a refusal), which operation
//! reports it, which reader it is read through (a poll's own answer, the history, a
//! describe, each visibility listing), how many tags the domain carries, how many times
//! one run hands a task over, and whether the run is still open.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Map, Value};

use crate::domain_tagging::{checked_tags, list_tags, registered_with_tags, Tag};
use crate::domain_tagging::RESOURCE_TAG_SHAPE_ID;
use crate::task_start_event_members::is_uuid_shaped;
use crate::workflow_execution_lifecycle::{
    error_code, full_defaults, i64_at, string_at, whole_history, Emulator,
};

/// The published model snapshot this crate is generated from, embedded at compile time so
/// the local declarations are checked against the model rather than against a copy of it.
const SNAPSHOT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/com-amazonaws-swf-simpleworkflowservice.json"
));

/// The overlay model those declarations are generated from, embedded for the same reason:
/// what it restates of the snapshot must still BE what the snapshot says.
const OVERLAY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/local-additions.json"
));

/// Every shape the overlay states with more members than the snapshot declares, and the
/// members it adds to each.
const ADDED_MEMBERS: &[(&str, &[&str])] = &[
    ("com.amazonaws.swf#ResourceTag", &["__type"]),
    (
        "com.amazonaws.swf#ActivityTaskScheduledEventAttributes",
        &["immediatelyStart"],
    ),
    (
        "com.amazonaws.swf#ActivityTaskStartedEventAttributes",
        &["clientToken"],
    ),
    (
        "com.amazonaws.swf#DecisionTaskStartedEventAttributes",
        &["clientToken", "startedOnPreviousTaskCompletion"],
    ),
    (
        "com.amazonaws.swf#WorkflowExecutionInfo",
        &[
            "lastClosedEventId",
            "lastClosedTimestamp",
            "reopenGenerationId",
            "reopenable",
        ],
    ),
    (
        "com.amazonaws.swf#WorkflowExecutionAlreadyStartedFault",
        &["workflowExecution"],
    ),
];

/// The shapes the overlay restates only so a member of one of the above can target them.
/// Codegen reads nothing of a structure target but its type and its name — the Rust field
/// carries the modelled type, declared in full from the snapshot — so these are restated
/// WITHOUT their members on purpose.
const TARGET_ONLY_SHAPES: &[&str] = &[
    "com.amazonaws.swf#ActivityType",
    "com.amazonaws.swf#TaskList",
    "com.amazonaws.swf#WorkflowExecution",
    "com.amazonaws.swf#WorkflowType",
];

/// The workflow type every run in this suite is started with.
const WORKFLOW_TYPE: (&str, &str) = ("declared-wf", "1.0");

/// The task list that type's decision tasks wait in.
const DECIDER_LIST: &str = "declared-deciders";

/// The activity type the run schedules a task of.
const ACTIVITY_TYPE: (&str, &str) = ("declared-activity", "2.0");

/// The task list that activity type's tasks wait in.
const ACTIVITY_LIST: &str = "declared-activities";

/// `oldestDate` is required on a visibility listing, and this is the oldest instant the
/// documented Unix-time form can name, so it excludes nothing.
const EPOCH_SECONDS: f64 = 0.0;

/// What one reported record must carry at one member.
#[derive(Debug)]
enum Expected {
    /// Exactly this JSON value, computed from the run's own state.
    Value(Value),
    /// Nothing: the record has nothing to report there.
    Absent,
    /// A UUID-shaped handle this service generated for that one hand-over.
    Handle,
}

/// One assertion: the record a response reported, the added member it must carry, and
/// what that member must carry.
#[derive(Debug)]
struct Row {
    case: String,
    reported: Value,
    member: &'static str,
    expected: Expected,
}

impl Row {
    fn new(case: impl Into<String>, reported: &Value, member: &'static str, expected: Expected) -> Self {
        Self {
            case: case.into(),
            reported: reported.clone(),
            member,
            expected,
        }
    }
}

/// The `shapes` object of one model document.
fn model_shapes(model: &str) -> Map<String, Value> {
    let parsed: Value = serde_json::from_str(model).expect("a model document must be JSON");
    parsed["shapes"]
        .as_object()
        .expect("a model document must carry a shapes object")
        .clone()
}

/// `declaration` with every documentation trait dropped, recursively — and with a traits
/// object left empty by that dropped too, so a member the snapshot documents and nothing
/// else compares equal to the same member restated with no traits at all.
///
/// Documentation is prose: the overlay restates what codegen READS — targets, types, enum
/// values and constraint traits — so comparing declarations has to ignore the one trait
/// that carries no behaviour.
fn without_docs(declaration: &Value) -> Value {
    match declaration {
        Value::Object(members) => Value::Object(
            members
                .iter()
                .filter(|(key, _)| key.as_str() != "smithy.api#documentation")
                .map(|(key, value)| (key.clone(), without_docs(value)))
                .filter(|(key, value)| {
                    key.as_str() != "traits" || !value.as_object().is_some_and(Map::is_empty)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(without_docs).collect()),
        other => other.clone(),
    }
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

/// The attributes member of `event`, which is named after the event's own type.
fn attributes_of(case: &str, event: &Value, member: &str) -> Value {
    event
        .get(member)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no {member} in {event:?}"))
}

/// Read one run back through `DescribeWorkflowExecution`.
async fn described_info(emulator: &Emulator, case: &str, domain: &str, run: (&str, &str)) -> Value {
    let body = emulator
        .ok(
            case,
            "DescribeWorkflowExecution",
            &json!({
                "domain": domain,
                "execution": { "workflowId": run.0, "runId": run.1 },
            }),
        )
        .await;
    attributes_of(case, &body, "executionInfo")
}

/// The `executionInfos` entry one visibility listing reports for `run_id`.
async fn listed_info(
    emulator: &Emulator,
    case: &str,
    operation: &str,
    domain: &str,
    run_id: &str,
) -> Value {
    let body = emulator
        .ok(
            case,
            operation,
            &json!({ "domain": domain, "startTimeFilter": { "oldestDate": EPOCH_SECONDS } }),
        )
        .await;
    body.pointer("/executionInfos")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no executionInfos in {body:?}"))
        .iter()
        .find(|info| {
            info.pointer("/execution/runId")
                .and_then(Value::as_str)
                .is_some_and(|listed| listed == run_id)
        })
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: {operation} did not report {run_id:?}"))
}

/// Poll one decision task off `DECIDER_LIST` and answer it with `decisions`, returning the
/// poll's own answer — which carries the history the start it just recorded is in.
async fn hand_over_decision(emulator: &Emulator, case: &str, domain: &str, decisions: Value) -> Value {
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
    task
}

#[test]
fn the_overlay_restates_the_snapshot_exactly_but_for_the_members_it_adds() {
    let snapshot = model_shapes(SNAPSHOT);
    let overlay = model_shapes(OVERLAY);
    let added: BTreeMap<&str, &[&str]> = ADDED_MEMBERS.iter().copied().collect();

    for (shape_id, declared) in &overlay {
        let modelled = snapshot.get(shape_id).unwrap_or_else(|| {
            panic!("the overlay restates {shape_id}, which the snapshot does not declare")
        });

        if let Some(adds) = added.get(shape_id.as_str()) {
            let declared_members = declared["members"]
                .as_object()
                .unwrap_or_else(|| panic!("the overlay declares {shape_id} with members"));
            let modelled_members = modelled["members"]
                .as_object()
                .unwrap_or_else(|| panic!("the snapshot declares {shape_id} with members"));
            for (member, modelled_member) in modelled_members {
                assert_eq!(
                    without_docs(&declared_members[member]),
                    without_docs(modelled_member),
                    "case {shape_id:?}: member {member} must be restated as the snapshot \
                     declares it, got {:?}",
                    declared_members.get(member)
                );
            }
            let extra: Vec<&String> = declared_members
                .keys()
                .filter(|member| !modelled_members.contains_key(*member))
                .collect();
            assert_eq!(
                extra,
                adds.iter().collect::<Vec<&&str>>(),
                "case {shape_id:?}: the overlay may add {adds:?} and nothing else, added \
                 {extra:?}"
            );
            // Every trait of the shape itself is restated as the snapshot spells it — the
            // error class of a fault included, so a refusal keeps its code and status.
            let declared_traits = without_docs(declared)
                .get("traits")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let modelled_traits = without_docs(modelled)
                .get("traits")
                .cloned()
                .unwrap_or_else(|| json!({}));
            assert_eq!(
                declared_traits, modelled_traits,
                "case {shape_id:?}: the shape's own traits must be restated as the snapshot \
                 spells them, got {declared_traits:?}"
            );
            continue;
        }

        if TARGET_ONLY_SHAPES.contains(&shape_id.as_str()) {
            assert_eq!(
                declared["type"], modelled["type"],
                "case {shape_id:?}: a target-only shape must be restated with the type the \
                 snapshot gives it, got {declared:?}"
            );
            assert!(
                declared["members"].as_object().is_some_and(Map::is_empty),
                "case {shape_id:?}: a target-only shape is restated for its type alone, so \
                 it must declare no members, got {declared:?}"
            );
            continue;
        }

        assert_eq!(
            without_docs(declared),
            without_docs(modelled),
            "case {shape_id:?}: must be restated exactly as the snapshot declares it"
        );
    }

    // Every shape that adds a member is restated, and the snapshot declares none of the
    // added members: if a refreshed snapshot ever does, that declaration should give way
    // to the modelled shape rather than quietly shadow it.
    for (shape_id, adds) in ADDED_MEMBERS {
        let declared = overlay
            .get(*shape_id)
            .unwrap_or_else(|| panic!("case {shape_id:?}: the overlay must restate this shape"));
        assert!(
            declared["members"].as_object().is_some_and(|m| !m.is_empty()),
            "case {shape_id:?}: restated with its members, got {declared:?}"
        );
        let modelled_members = snapshot[*shape_id]["members"]
            .as_object()
            .unwrap_or_else(|| panic!("case {shape_id:?}: the snapshot declares it with members"))
            .clone();
        for member in *adds {
            assert!(
                !modelled_members.contains_key(*member),
                "case {shape_id:?}: the snapshot now declares {member}, so the local \
                 declaration should give way to the modelled member; snapshot members {:?}",
                modelled_members.keys().collect::<Vec<&String>>()
            );
        }
    }
}

#[tokio::test]
async fn every_member_the_overlay_adds_is_reported_on_the_wire() {
    let emulator = Emulator::new();
    let domain = "declared-members";
    let workflow_id = "wf-declared";

    // ---- the records every row below is read out of ----------------------------------
    // A domain carrying tags of each form the shape allows: a value, an empty value and no
    // value at all, so the discriminator is asserted beside every form of stored tag.
    let stored = vec![
        Tag::new("squad", Some("payments")),
        Tag::new("empty-value", Some("")),
        Tag::new("no-value", None),
    ];
    let arn = registered_with_tags(&emulator, "declared", domain, &stored).await;
    emulator
        .ok(
            "declared",
            "RegisterWorkflowType",
            &{
                let mut body = json!({
                    "domain": domain,
                    "name": WORKFLOW_TYPE.0,
                    "version": WORKFLOW_TYPE.1,
                });
                crate::workflow_execution_lifecycle::merge(
                    &mut body,
                    full_defaults(DECIDER_LIST, "TERMINATE"),
                );
                body
            },
        )
        .await;
    emulator
        .ok(
            "declared",
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

    let listed_tags = list_tags(&emulator, "declared", &arn).await;
    assert_eq!(
        checked_tags("declared", &listed_tags),
        stored.iter().cloned().collect::<BTreeSet<Tag>>(),
        "case \"declared\": the discriminator sits beside the stored key and value, which \
         must read back unchanged, got {listed_tags:?}"
    );
    let tag_elements = listed_tags
        .get("tags")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case \"declared\": expected a tags array, got {listed_tags:?}"))
        .clone();

    let started = emulator
        .ok(
            "declared",
            "StartWorkflowExecution",
            &json!({
                "domain": domain,
                "workflowId": workflow_id,
                "workflowType": { "name": WORKFLOW_TYPE.0, "version": WORKFLOW_TYPE.1 },
            }),
        )
        .await;
    let run_id = string_at(&started, "/runId");

    // A second start of the same id collides with that open run, so the refusal is the
    // record carrying the fault's own added member.
    let (status, refused) = emulator
        .call(
            "StartWorkflowExecution",
            &json!({
                "domain": domain,
                "workflowId": workflow_id,
                "workflowType": { "name": WORKFLOW_TYPE.0, "version": WORKFLOW_TYPE.1 },
            }),
        )
        .await;
    let code = error_code(&refused);
    assert_eq!(
        (status, code.as_str()),
        (400, "WorkflowExecutionAlreadyStartedFault"),
        "case \"collision\": a start colliding with an open run is 400 \
         WorkflowExecutionAlreadyStartedFault, got {status} {code} with {refused:?}"
    );

    let open_described = described_info(&emulator, "open", domain, (workflow_id, &run_id)).await;
    let open_listed = listed_info(
        &emulator,
        "open",
        "ListOpenWorkflowExecutions",
        domain,
        &run_id,
    )
    .await;

    // The run's first hand-over, answered with a decision that schedules an activity task
    // into its own task list — so the scheduled task is one a worker must poll for.
    let first_poll = hand_over_decision(
        &emulator,
        "hand-over",
        domain,
        json!([{
            "decisionType": "ScheduleActivityTask",
            "scheduleActivityTaskDecisionAttributes": {
                "activityId": "act-1",
                "activityType": { "name": ACTIVITY_TYPE.0, "version": ACTIVITY_TYPE.1 },
                "input": "payload",
            },
        }]),
    )
    .await;
    let polled_events = first_poll
        .pointer("/events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case \"hand-over\": no events in {first_poll:?}"));

    // The second hand-over: something the decider has not seen is what schedules it, so a
    // signal is what the next task carries.
    emulator
        .ok(
            "hand-over",
            "SignalWorkflowExecution",
            &json!({
                "domain": domain,
                "workflowId": workflow_id,
                "runId": run_id,
                "signalName": "wake-up",
            }),
        )
        .await;
    emulator
        .ok(
            "hand-over",
            "PollForActivityTask",
            &json!({ "domain": domain, "taskList": { "name": ACTIVITY_LIST } }),
        )
        .await;
    hand_over_decision(&emulator, "hand-over", domain, json!([])).await;

    let history = whole_history(&emulator, domain, workflow_id, &run_id).await;
    let decision_starts = of_type(&history, "DecisionTaskStarted");
    assert_eq!(
        decision_starts.len(),
        2,
        "case \"hand-over\": the run has handed a decision task over twice, got {history:?}"
    );
    let activity_starts = of_type(&history, "ActivityTaskStarted");
    assert_eq!(
        activity_starts.len(),
        1,
        "case \"hand-over\": the worker was handed the one scheduled task, got {history:?}"
    );
    let activity_scheduled = of_type(&history, "ActivityTaskScheduled");
    assert_eq!(
        activity_scheduled.len(),
        1,
        "case \"hand-over\": the decision scheduled exactly one activity task, got {history:?}"
    );

    // Close the run, so the execution record has a close to report: the id of the event
    // that closed it and the instant that event is stamped at.
    emulator
        .ok(
            "closed",
            "TerminateWorkflowExecution",
            &json!({
                "domain": domain,
                "workflowId": workflow_id,
                "runId": run_id,
                "reason": "done",
            }),
        )
        .await;
    let closed_history = whole_history(&emulator, domain, workflow_id, &run_id).await;
    let terminated = of_type(&closed_history, "WorkflowExecutionTerminated");
    assert_eq!(
        terminated.len(),
        1,
        "case \"closed\": the run is terminated exactly once, got {closed_history:?}"
    );
    let closing_event_id = i64_at(&terminated[0], "/eventId");
    let closed_described =
        described_info(&emulator, "closed", domain, (workflow_id, &run_id)).await;
    let closed_listed = listed_info(
        &emulator,
        "closed",
        "ListClosedWorkflowExecutions",
        domain,
        &run_id,
    )
    .await;
    let close_timestamp = closed_described
        .get("closeTimestamp")
        .cloned()
        .unwrap_or_else(|| panic!("case \"closed\": a closed run reports when it closed, got {closed_described:?}"));

    // ---- one row per added member of every record above ------------------------------
    let mut rows: Vec<Row> = Vec::new();
    for (index, element) in tag_elements.iter().enumerate() {
        rows.push(Row::new(
            format!("listed tag {index}"),
            element,
            "__type",
            Expected::Value(json!(RESOURCE_TAG_SHAPE_ID)),
        ));
    }
    for (reader, event) in [
        ("the poll's own answer", &of_type(&polled_events, "DecisionTaskStarted")[0]),
        ("the history", &decision_starts[0]),
        ("the history, second hand-over", &decision_starts[1]),
    ] {
        let attributes = attributes_of(reader, event, "decisionTaskStartedEventAttributes");
        rows.push(Row::new(
            format!("a decision task's start through {reader}"),
            &attributes,
            "clientToken",
            Expected::Handle,
        ));
        rows.push(Row::new(
            format!("a decision task's start through {reader}"),
            &attributes,
            "startedOnPreviousTaskCompletion",
            Expected::Value(json!(false)),
        ));
    }
    rows.push(Row::new(
        "an activity task's start",
        &attributes_of(
            "an activity task's start",
            &activity_starts[0],
            "activityTaskStartedEventAttributes",
        ),
        "clientToken",
        Expected::Handle,
    ));
    rows.push(Row::new(
        "an activity task scheduled into a task list",
        &attributes_of(
            "an activity task scheduled into a task list",
            &activity_scheduled[0],
            "activityTaskScheduledEventAttributes",
        ),
        "immediatelyStart",
        Expected::Value(json!(false)),
    ));
    for (reader, info) in [
        ("a describe", &open_described),
        ("the open listing", &open_listed),
    ] {
        rows.push(Row::new(
            format!("an open run through {reader}"),
            info,
            "lastClosedEventId",
            Expected::Value(json!(0)),
        ));
        rows.push(Row::new(
            format!("an open run through {reader}"),
            info,
            "lastClosedTimestamp",
            Expected::Absent,
        ));
        rows.push(Row::new(
            format!("an open run through {reader}"),
            info,
            "reopenGenerationId",
            Expected::Value(json!(0)),
        ));
        rows.push(Row::new(
            format!("an open run through {reader}"),
            info,
            "reopenable",
            Expected::Value(json!(false)),
        ));
    }
    for (reader, info) in [
        ("a describe", &closed_described),
        ("the closed listing", &closed_listed),
    ] {
        rows.push(Row::new(
            format!("a closed run through {reader}"),
            info,
            "lastClosedEventId",
            Expected::Value(json!(closing_event_id)),
        ));
        rows.push(Row::new(
            format!("a closed run through {reader}"),
            info,
            "lastClosedTimestamp",
            Expected::Value(close_timestamp.clone()),
        ));
        rows.push(Row::new(
            format!("a closed run through {reader}"),
            info,
            "reopenGenerationId",
            Expected::Value(json!(0)),
        ));
        rows.push(Row::new(
            format!("a closed run through {reader}"),
            info,
            "reopenable",
            Expected::Value(json!(false)),
        ));
    }
    rows.push(Row::new(
        "a colliding start",
        &refused,
        "workflowExecution",
        Expected::Value(json!({ "workflowId": workflow_id, "runId": run_id })),
    ));

    let mut handles: BTreeSet<String> = BTreeSet::new();
    for row in &rows {
        let actual = row.reported.get(row.member);
        match &row.expected {
            Expected::Value(expected) => assert_eq!(
                actual,
                Some(expected),
                "case {:?}: expected {} to be {expected:?}, got {actual:?} in {:?}",
                row.case,
                row.member,
                row.reported
            ),
            Expected::Absent => assert!(
                actual.is_none(),
                "case {:?}: expected no {}, got {actual:?} in {:?}",
                row.case,
                row.member,
                row.reported
            ),
            Expected::Handle => {
                let handle = actual.and_then(Value::as_str).unwrap_or_else(|| {
                    panic!(
                        "case {:?}: expected a {} string, got {actual:?} in {:?}",
                        row.case, row.member, row.reported
                    )
                });
                assert!(
                    is_uuid_shaped(handle),
                    "case {:?}: expected {} to be UUID-shaped, got {handle:?}",
                    row.case,
                    row.member
                );
                handles.insert(handle.to_string());
            }
        }
    }

    // Each hand-over gets its own handle, and reading the SAME start again reports the
    // handle the stored event already carries: three hand-overs, three handles, read
    // through four records.
    assert_eq!(
        handles.len(),
        3,
        "case \"hand-over\": each of the run's three hand-overs has its own handle, got \
         {handles:?}"
    );
}
