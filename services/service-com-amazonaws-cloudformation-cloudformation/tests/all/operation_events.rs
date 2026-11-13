// SPDX-License-Identifier: CC0-1.0

//! `DescribeEvents` — a stack's recorded events read as the events of the OPERATIONS that
//! caused them, end to end over the awsQuery wire.
//!
//! The rule under test: **every event a stack recorded belongs to exactly one operation —
//! "a discrete change attempt on the stack" — and `DescribeEvents` reports it with that
//! operation's id, type, status and span, grouped by operation and newest first, selected
//! by the query criteria the request states.** So:
//!
//! * each reported event carries the `OperationId` of the operation that caused it, and the
//!   `OperationType` / `OperationStatus` / `StartTime` / `EndTime` of THAT operation — one
//!   record per operation, so every event of one operation agrees about it;
//! * an event's `Timestamp` falls inside its operation's span;
//! * the operations come back newest first, each one's events together, and so do the
//!   events inside one operation;
//! * `EventType` says what kind of event it is: the stack's own status changes are
//!   `STACK_EVENT`, a resource's progress is a `PROGRESS_EVENT`, and a resource transition
//!   an owning service refused is a `PROVISIONING_ERROR`;
//! * `StackName`, `OperationId`, `ChangeSetName` and `Filters.FailedEvents` each select a
//!   subset of that same log, and a request that states no criteria at all is refused.
//!
//! Without that, a client watching a deployment cannot tell which attempt an event belongs
//! to, which is the whole point of the operation ("Groups events by operation ID, enabling
//! you to focus on individual stack operations during deployment").
//!
//! The rows differ along what the rule generalises over: how many operations one stack has
//! lived through and of which types (create, update, delete, change-set creation and its
//! execution), how many resources an operation walks, whether the operation succeeded or
//! failed, and which criterion the read selects by. The recorded trace this suite was
//! written for is ONE row of the first table.

use serde_json::json;

use service_com_amazonaws_cloudformation_cloudformation::state::{
    operation_status, operation_type, status,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::change_set_lifecycle::{create_creating_change_set, execute_change_set};
use crate::stack_lifecycle::{
    acknowledging_iam, call, create_stack, describe_stack, iam, member, router,
    wait_for_the_transition, xml_blocks, xml_text,
};
use crate::stack_resources::element;

/// A template declaring one topic, which Amazon SNS really creates.
fn one_resource(suffix: &str) -> String {
    json!({"Resources": {"Topic": {
        "Type": "AWS::SNS::Topic",
        "Properties": {"TopicName": format!("operation-events-{suffix}")}
    }}})
    .to_string()
}

/// A template declaring three resources with no dependency between them, so one operation's
/// walk records several resources' events.
fn three_resources(suffix: &str) -> String {
    json!({"Resources": {
        "TopicA": {
            "Type": "AWS::SNS::Topic",
            "Properties": {"TopicName": format!("operation-events-{suffix}-a")}
        },
        "TopicB": {
            "Type": "AWS::SNS::Topic",
            "Properties": {"TopicName": format!("operation-events-{suffix}-b")}
        },
        "Bucket": {
            "Type": "AWS::S3::Bucket",
            "Properties": {"BucketName": format!("operation-events-{suffix}-bucket")}
        }
    }})
    .to_string()
}

/// A trust policy IAM accepts.
fn trust_policy() -> serde_json::Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": "glue.amazonaws.com"},
            "Action": "sts:AssumeRole"
        }]
    })
}

/// A template whose second resource is an IAM role under a name the account already holds,
/// so the service that owns it REFUSES to create it and the create fails on that resource.
fn refused_resource(suffix: &str, role: &str) -> String {
    json!({"Resources": {
        "ATopic": {
            "Type": "AWS::SNS::Topic",
            "Properties": {"TopicName": format!("operation-events-{suffix}")}
        },
        "ZRole": {
            "Type": "AWS::IAM::Role",
            "Properties": {"RoleName": role, "AssumeRolePolicyDocument": trust_policy()}
        }
    }})
    .to_string()
}

/// One `OperationEvent` as the wire reports it, reduced to what this suite reads.
#[derive(Debug, Clone)]
struct Event {
    event_id: String,
    stack_id: String,
    operation_id: String,
    operation_type: String,
    operation_status: String,
    event_type: String,
    logical_id: String,
    resource_status: String,
    timestamp: String,
    start_time: String,
    end_time: Option<String>,
}

/// `DescribeEvents` with whatever criteria a case states.
async fn describe_events(
    router: &MultiServiceRouter,
    members: &[(String, String)],
) -> (u16, String) {
    call(router, "DescribeEvents", members).await
}

/// The `OperationEvents` a response carries, in wire order.
///
/// Read INSIDE the `OperationEvents` block, so nothing else the envelope carries can be
/// swept up with it.
fn parsed_events(case: &str, body: &str) -> Vec<Event> {
    let block = xml_blocks(body, "OperationEvents")
        .into_iter()
        .next()
        .unwrap_or_default();
    xml_blocks(&block, "member")
        .iter()
        .map(|entry| Event {
            event_id: element(entry, "EventId").unwrap_or_else(|| {
                panic!("case {case:?}: every event carries an EventId, got {entry}")
            }),
            stack_id: element(entry, "StackId").unwrap_or_else(|| {
                panic!("case {case:?}: every event carries a StackId, got {entry}")
            }),
            operation_id: element(entry, "OperationId").unwrap_or_else(|| {
                panic!("case {case:?}: every event carries an OperationId, got {entry}")
            }),
            operation_type: element(entry, "OperationType").unwrap_or_else(|| {
                panic!("case {case:?}: every event carries an OperationType, got {entry}")
            }),
            operation_status: element(entry, "OperationStatus").unwrap_or_else(|| {
                panic!("case {case:?}: every event carries an OperationStatus, got {entry}")
            }),
            event_type: element(entry, "EventType").unwrap_or_else(|| {
                panic!("case {case:?}: every event carries an EventType, got {entry}")
            }),
            logical_id: element(entry, "LogicalResourceId").unwrap_or_default(),
            resource_status: element(entry, "ResourceStatus").unwrap_or_default(),
            timestamp: element(entry, "Timestamp").unwrap_or_else(|| {
                panic!("case {case:?}: every event carries a Timestamp, got {entry}")
            }),
            start_time: element(entry, "StartTime").unwrap_or_else(|| {
                panic!("case {case:?}: every event carries its operation's StartTime, got {entry}")
            }),
            end_time: element(entry, "EndTime"),
        })
        .collect()
}

/// The instant one reported timestamp names, in microseconds since the epoch.
///
/// Read back with the same parser the crate itself uses for an ISO 8601 date-time, so what
/// the cases compare is the instant the emulator published rather than text.
fn micros(case: &str, label: &str, text: &str) -> i128 {
    let instant = ::aws_smithy_types::DateTime::from_str(
        text,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .unwrap_or_else(|error| {
        panic!("case {case:?}: expected an ISO 8601 {label}, got {text:?} ({error})")
    });
    i128::from(instant.secs()) * 1_000_000 + i128::from(instant.subsec_nanos() / 1_000)
}

/// The `OperationId`s a page reports, in wire order, with each one appearing once: an
/// operation whose events are NOT contiguous appears twice here, which is what the grouping
/// assertion reads.
fn operation_runs(events: &[Event]) -> Vec<String> {
    let mut runs: Vec<String> = Vec::new();
    for event in events {
        if runs.last() != Some(&event.operation_id) {
            runs.push(event.operation_id.clone());
        }
    }
    runs
}

/// `DescribeStackEvents` for one stack, reduced to the `EventId`s it reports.
async fn stack_event_ids(case: &str, router: &MultiServiceRouter, name_or_id: &str) -> Vec<String> {
    let (status_code, body) = call(
        router,
        "DescribeStackEvents",
        &[member("StackName", name_or_id)],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: DescribeStackEvents must answer 200, got {status_code} {body}"
    );
    crate::harness::xml_texts(&body, "EventId")
}

/// Hold every event of one page to the rule: it names its operation, agrees with every
/// other event of that operation about it, and happened inside that operation's span.
fn assert_events_belong_to_their_operation(case: &str, events: &[Event], stack_id: &str) {
    assert!(
        !events.is_empty(),
        "case {case:?}: the stack has recorded events, so the read must report some"
    );
    for event in events {
        assert_eq!(
            event.stack_id, stack_id,
            "case {case:?}: every event names the stack it belongs to, got {event:?}"
        );
        assert!(
            !event.event_id.is_empty() && !event.operation_id.is_empty(),
            "case {case:?}: an event names itself and its operation, got {event:?}"
        );
        let at = micros(case, "Timestamp", &event.timestamp);
        let started = micros(case, "StartTime", &event.start_time);
        assert!(
            at >= started,
            "case {case:?}: an event cannot happen before the operation that caused it \
             started, got {at} against {started} in {event:?}"
        );
        if let Some(ended) = event.end_time.as_deref() {
            let ended = micros(case, "EndTime", ended);
            assert!(
                at <= ended,
                "case {case:?}: an event cannot happen after the operation that caused it \
                 ended, got {at} against {ended} in {event:?}"
            );
            assert!(
                ended >= started,
                "case {case:?}: an operation ends at or after it started, got {ended} \
                 against {started} in {event:?}"
            );
        }
        // One record per operation: every event of one operation reports the same
        // operation.
        for other in events
            .iter()
            .filter(|other| other.operation_id == event.operation_id)
        {
            assert_eq!(
                (
                    other.operation_type.as_str(),
                    other.operation_status.as_str(),
                    other.start_time.as_str(),
                    other.end_time.as_deref()
                ),
                (
                    event.operation_type.as_str(),
                    event.operation_status.as_str(),
                    event.start_time.as_str(),
                    event.end_time.as_deref()
                ),
                "case {case:?}: two events of one operation must report that operation \
                 alike, got {other:?} and {event:?}"
            );
        }
    }
}

/// Hold a page to the ORDER the rule states: newest first, with each operation's events
/// contiguous.
fn assert_newest_first_and_grouped(case: &str, events: &[Event]) {
    let mut previous: Option<i128> = None;
    for event in events {
        let at = micros(case, "Timestamp", &event.timestamp);
        if let Some(previous) = previous {
            assert!(
                at <= previous,
                "case {case:?}: the events come back newest first, got {at} after \
                 {previous} in {events:?}"
            );
        }
        previous = Some(at);
    }
    let runs = operation_runs(events);
    let mut distinct = runs.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        runs.len(),
        distinct.len(),
        "case {case:?}: the events of one operation are reported together, so no operation \
         may appear in two runs, got {runs:?}"
    );
}

/// One row of the lifecycle table: the steps a stack is driven through and the operations
/// its events must then report, newest first.
struct Lifecycle {
    case: &'static str,
    stack: &'static str,
    /// The template each `CreateStack` / `UpdateStack` step submits, in order.
    templates: Vec<String>,
    /// Whether the stack is deleted after those steps.
    delete: bool,
    /// The `OperationType` of each operation the read must report, newest first.
    expected: Vec<&'static str>,
}

#[tokio::test]
async fn every_event_reports_the_operation_that_caused_it_newest_operation_first() {
    let rows = vec![
        Lifecycle {
            case: "a create alone",
            stack: "operation-events-create",
            templates: vec![one_resource("create")],
            delete: false,
            expected: vec![operation_type::CREATE_STACK],
        },
        Lifecycle {
            case: "a create whose walk covers three resources",
            stack: "operation-events-wide",
            templates: vec![three_resources("wide")],
            delete: false,
            expected: vec![operation_type::CREATE_STACK],
        },
        Lifecycle {
            case: "a create and then an update",
            stack: "operation-events-update",
            templates: vec![one_resource("update-one"), one_resource("update-two")],
            delete: false,
            expected: vec![operation_type::UPDATE_STACK, operation_type::CREATE_STACK],
        },
        Lifecycle {
            case: "a create, two updates and a delete",
            stack: "operation-events-many",
            templates: vec![
                one_resource("many-one"),
                one_resource("many-two"),
                one_resource("many-three"),
            ],
            delete: true,
            expected: vec![
                operation_type::DELETE_STACK,
                operation_type::UPDATE_STACK,
                operation_type::UPDATE_STACK,
                operation_type::CREATE_STACK,
            ],
        },
    ];

    for row in rows {
        let case = row.case;
        let router = router();
        let mut stack_id = String::new();
        for (index, template) in row.templates.iter().enumerate() {
            let (status_code, body) = if index == 0 {
                create_stack(&router, row.stack, template, &[]).await
            } else {
                call(
                    &router,
                    "UpdateStack",
                    &[
                        member("StackName", row.stack),
                        member("TemplateBody", template),
                    ],
                )
                .await
            };
            assert_eq!(
                status_code, 200,
                "case {case:?}: step {index} must be accepted, got {status_code} {body}"
            );
            if index == 0 {
                stack_id = xml_text(&body, "StackId").unwrap_or_else(|| {
                    panic!("case {case:?}: CreateStack answers the stack id, got {body}")
                });
            }
            wait_for_the_transition().await;
        }
        if row.delete {
            let (status_code, body) =
                call(&router, "DeleteStack", &[member("StackName", row.stack)]).await;
            assert_eq!(
                status_code, 200,
                "case {case:?}: DeleteStack must be accepted, got {status_code} {body}"
            );
            wait_for_the_transition().await;
        }
        // A deleted stack is addressable by its id alone, so every row reads by the id the
        // create answered.
        let (status_code, described) = describe_stack(&router, &stack_id).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: the stack must be readable, got {status_code} {described}"
        );

        let (status_code, body) =
            describe_events(&router, &[member("StackName", &stack_id)]).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: DescribeEvents must answer 200, got {status_code} {body}"
        );
        let events = parsed_events(case, &body);
        assert_events_belong_to_their_operation(case, &events, &stack_id);
        assert_newest_first_and_grouped(case, &events);

        // One run per operation the stack lived through, newest first, each of the type
        // that operation was.
        let runs = operation_runs(&events);
        let reported: Vec<&str> = runs
            .iter()
            .map(|operation_id| {
                events
                    .iter()
                    .find(|event| &event.operation_id == operation_id)
                    .map(|event| event.operation_type.as_str())
                    .unwrap_or_default()
            })
            .collect();
        assert_eq!(
            reported, row.expected,
            "case {case:?}: expected the operations {:?} newest first, got {reported:?} in \
             {events:?}",
            row.expected
        );
        // Every operation of these rows concluded, and each went through.
        for event in &events {
            assert_eq!(
                event.operation_status,
                operation_status::SUCCEEDED,
                "case {case:?}: an operation that completed reports SUCCEEDED, got {event:?}"
            );
            assert!(
                event.end_time.is_some(),
                "case {case:?}: an operation that concluded reports an EndTime, got {event:?}"
            );
        }
        // The stack's own status changes are STACK_EVENTs and its resources' are progress
        // events: one log, two kinds of record.
        let stack_events: Vec<&Event> = events
            .iter()
            .filter(|event| event.logical_id == row.stack)
            .collect();
        assert!(
            !stack_events.is_empty(),
            "case {case:?}: the stack's own events are reported too, got {events:?}"
        );
        for event in stack_events {
            assert_eq!(
                event.event_type, "STACK_EVENT",
                "case {case:?}: the stack's own status change is a STACK_EVENT, got {event:?}"
            );
        }
        for event in events.iter().filter(|event| event.logical_id != row.stack) {
            assert_eq!(
                event.event_type, "PROGRESS_EVENT",
                "case {case:?}: a resource transition that went through is a \
                 PROGRESS_EVENT, got {event:?}"
            );
        }
        // It is the SAME persisted log `DescribeStackEvents` reports, read a different way.
        let mut reported_ids: Vec<String> =
            events.iter().map(|event| event.event_id.clone()).collect();
        let mut stack_ids = stack_event_ids(case, &router, &stack_id).await;
        reported_ids.sort();
        stack_ids.sort();
        assert_eq!(
            reported_ids, stack_ids,
            "case {case:?}: both reads answer from one event log, got {reported_ids:?} \
             against {stack_ids:?}"
        );
    }
}

#[tokio::test]
async fn an_operation_id_selects_exactly_that_operations_events() {
    const CASE: &str = "the id of the update, among a create and two updates";
    let router = router();
    let (status_code, created) = create_stack(
        &router,
        "operation-events-by-id",
        &one_resource("by-id-one"),
        &[],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {CASE:?}: CreateStack must answer 200, got {status_code} {created}"
    );
    let create_operation = xml_text(&created, "OperationId")
        .unwrap_or_else(|| panic!("case {CASE:?}: CreateStack answers an OperationId"));
    wait_for_the_transition().await;

    let mut update_operations: Vec<String> = Vec::new();
    for suffix in ["by-id-two", "by-id-three"] {
        let (status_code, updated) = call(
            &router,
            "UpdateStack",
            &[
                member("StackName", "operation-events-by-id"),
                member("TemplateBody", &one_resource(suffix)),
            ],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {CASE:?}: UpdateStack must answer 200, got {status_code} {updated}"
        );
        update_operations.push(
            xml_text(&updated, "OperationId")
                .unwrap_or_else(|| panic!("case {CASE:?}: UpdateStack answers an OperationId")),
        );
        wait_for_the_transition().await;
    }

    // Each id names its own operation's events, and nothing else's.
    for (operation_id, expected_type) in [
        (create_operation.as_str(), operation_type::CREATE_STACK),
        (update_operations[0].as_str(), operation_type::UPDATE_STACK),
        (update_operations[1].as_str(), operation_type::UPDATE_STACK),
    ] {
        let (status_code, body) =
            describe_events(&router, &[member("OperationId", operation_id)]).await;
        assert_eq!(
            status_code, 200,
            "case {CASE:?}: DescribeEvents({operation_id}) must answer 200, got \
             {status_code} {body}"
        );
        let events = parsed_events(CASE, &body);
        assert!(
            !events.is_empty(),
            "case {CASE:?}: the operation {operation_id} recorded events, got {body}"
        );
        assert_newest_first_and_grouped(CASE, &events);
        for event in &events {
            assert_eq!(
                event.operation_id, operation_id,
                "case {CASE:?}: only the named operation's events are reported, got {event:?}"
            );
            assert_eq!(
                event.operation_type, expected_type,
                "case {CASE:?}: expected OperationType {expected_type}, got {event:?}"
            );
        }
    }

    // An id nothing recorded selects no events — the read is not a refusal.
    let (status_code, body) = describe_events(
        &router,
        &[member("OperationId", "ec26df4d-0000-4000-8000-000000000000")],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {CASE:?}: an unknown OperationId selects nothing, got {status_code} {body}"
    );
    assert!(
        parsed_events(CASE, &body).is_empty(),
        "case {CASE:?}: an unknown OperationId selects no events, got {body}"
    );
}

#[tokio::test]
async fn a_change_set_names_the_operation_that_created_it() {
    const CASE: &str = "a CREATE change set, then its execution";
    let router = router();
    let (status_code, body) = create_creating_change_set(
        &router,
        "operation-events-plan",
        "operation-events-plan-set",
        &one_resource("plan"),
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {CASE:?}: CreateChangeSet must answer 200, got {status_code} {body}"
    );
    wait_for_the_transition().await;

    // The change set's own creation is a stack operation of its own, and that is what the
    // change set names.
    let (status_code, body) = describe_events(
        &router,
        &[
            member("ChangeSetName", "operation-events-plan-set"),
            member("StackName", "operation-events-plan"),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {CASE:?}: DescribeEvents must answer 200, got {status_code} {body}"
    );
    let planned = parsed_events(CASE, &body);
    assert!(
        !planned.is_empty(),
        "case {CASE:?}: creating the change set recorded an event, got {body}"
    );
    for event in &planned {
        assert_eq!(
            event.operation_type,
            operation_type::CREATE_CHANGESET,
            "case {CASE:?}: the change set's events belong to its creation, got {event:?}"
        );
        assert_eq!(
            event.operation_status,
            operation_status::SUCCEEDED,
            "case {CASE:?}: a change set that was created reports a succeeded operation, \
             got {event:?}"
        );
        assert_eq!(
            event.resource_status,
            status::REVIEW_IN_PROGRESS,
            "case {CASE:?}: the stack a CREATE change set made is in review, got {event:?}"
        );
    }

    // Executing the change set is a SECOND operation on the stack: the stack's whole log
    // carries both, newest first, and the change set still names only its own.
    let (status_code, body) =
        execute_change_set(&router, "operation-events-plan-set", Some("operation-events-plan"))
            .await;
    assert_eq!(
        status_code, 200,
        "case {CASE:?}: ExecuteChangeSet must answer 200, got {status_code} {body}"
    );
    wait_for_the_transition().await;

    let (_, body) =
        describe_events(&router, &[member("StackName", "operation-events-plan")]).await;
    let all = parsed_events(CASE, &body);
    assert_newest_first_and_grouped(CASE, &all);
    let runs = operation_runs(&all);
    let reported: Vec<&str> = runs
        .iter()
        .map(|operation_id| {
            all.iter()
                .find(|event| &event.operation_id == operation_id)
                .map(|event| event.operation_type.as_str())
                .unwrap_or_default()
        })
        .collect();
    assert_eq!(
        reported,
        vec![operation_type::CREATE_STACK, operation_type::CREATE_CHANGESET],
        "case {CASE:?}: the execution is the newer operation and the change set's creation \
         the older, got {reported:?} in {all:?}"
    );
    let (_, body) = describe_events(
        &router,
        &[
            member("ChangeSetName", "operation-events-plan-set"),
            member("StackName", "operation-events-plan"),
        ],
    )
    .await;
    for event in parsed_events(CASE, &body) {
        assert_eq!(
            event.operation_type,
            operation_type::CREATE_CHANGESET,
            "case {CASE:?}: the change set names its own creation only, got {event:?}"
        );
    }
}

#[tokio::test]
async fn a_failed_operation_reports_its_failure_and_the_filter_selects_it() {
    const CASE: &str = "a create whose IAM role name the account already holds";
    let stack = "operation-events-refused";
    let role = "operation-events-refused-role";
    let router = router();

    // Whoever holds the role name first: a role a client created outside any stack.
    let (status_code, body) = iam(
        &router,
        "CreateRole",
        &[
            ("RoleName", role),
            ("AssumeRolePolicyDocument", &trust_policy().to_string()),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {CASE:?}: the role must be created first, got {status_code} {body}"
    );

    let (status_code, created) = call(
        &router,
        "CreateStack",
        &acknowledging_iam(&[
            member("StackName", stack),
            member("TemplateBody", &refused_resource("refused", role)),
            // The refusal this case is about is the OWNING SERVICE's, so the create has to
            // reach it: with the pre-deployment validations on, the existence check fails a
            // create whose declared name is already held before any resource is asked for
            // (`state::early_validation`).
            member("DisableValidation", "true"),
        ]),
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {CASE:?}: CreateStack must be accepted, got {status_code} {created}"
    );
    let stack_id = xml_text(&created, "StackId")
        .unwrap_or_else(|| panic!("case {CASE:?}: CreateStack answers the stack id"));
    // Twice: the failed create also owes the rollback its terminal status.
    wait_for_the_transition().await;
    wait_for_the_transition().await;
    let (_, described) = describe_stack(&router, &stack_id).await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::ROLLBACK_COMPLETE),
        "case {CASE:?}: the refused create must roll the stack back, got {described}"
    );

    let (status_code, body) = describe_events(&router, &[member("StackName", &stack_id)]).await;
    assert_eq!(
        status_code, 200,
        "case {CASE:?}: DescribeEvents must answer 200, got {status_code} {body}"
    );
    let events = parsed_events(CASE, &body);
    assert_events_belong_to_their_operation(CASE, &events, &stack_id);
    assert_newest_first_and_grouped(CASE, &events);
    // The attempt FAILED, and it stays failed through the rollback its failure owed.
    for event in &events {
        assert_eq!(
            event.operation_status,
            operation_status::FAILED,
            "case {CASE:?}: the create failed, so its events report a failed operation, \
             got {event:?}"
        );
        assert_eq!(
            event.operation_type,
            operation_type::CREATE_STACK,
            "case {CASE:?}: the rollback belongs to the create that owed it, got {event:?}"
        );
    }
    // The resource the owning service refused is reported as a provisioning error.
    let refused: Vec<&Event> = events
        .iter()
        .filter(|event| event.logical_id == "ZRole" && event.resource_status.ends_with("_FAILED"))
        .collect();
    assert!(
        !refused.is_empty(),
        "case {CASE:?}: the refused resource owes a failed event, got {events:?}"
    );
    for event in refused {
        assert_eq!(
            event.event_type, "PROVISIONING_ERROR",
            "case {CASE:?}: a resource transition an owning service refused is a \
             PROVISIONING_ERROR, got {event:?}"
        );
    }

    // "When set to true, only returns failed events within the operation."
    let (status_code, body) = describe_events(
        &router,
        &[
            member("StackName", &stack_id),
            member("Filters.FailedEvents", "true"),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {CASE:?}: the filtered read must answer 200, got {status_code} {body}"
    );
    let failed_only = parsed_events(CASE, &body);
    assert!(
        !failed_only.is_empty(),
        "case {CASE:?}: the operation failed, so the filter selects its failures, got {body}"
    );
    assert_newest_first_and_grouped(CASE, &failed_only);
    for event in &failed_only {
        assert!(
            event.resource_status.ends_with("_FAILED"),
            "case {CASE:?}: the filter selects only failed events, got {event:?}"
        );
    }
    assert!(
        failed_only.len() < events.len(),
        "case {CASE:?}: the filter is a subset of the log, got {} of {} in {failed_only:?}",
        failed_only.len(),
        events.len()
    );
    // A stack whose operations all went through has no failed event to select.
    let (status_code, healthy) = create_stack(
        &router,
        "operation-events-healthy",
        &one_resource("healthy"),
        &[],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {CASE:?}: the healthy stack must be created, got {status_code} {healthy}"
    );
    wait_for_the_transition().await;
    let (status_code, body) = describe_events(
        &router,
        &[
            member("StackName", "operation-events-healthy"),
            member("Filters.FailedEvents", "true"),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {CASE:?}: the filtered read must answer 200, got {status_code} {body}"
    );
    assert!(
        parsed_events(CASE, &body).is_empty(),
        "case {CASE:?}: nothing failed on that stack, so the filter selects nothing, got \
         {body}"
    );
}

/// One row of the refusal table: the criteria a request states, and the message the
/// refusal carries.
struct Refusal {
    case: &'static str,
    members: Vec<(String, String)>,
    message: String,
}

#[tokio::test]
async fn a_read_that_names_nothing_addressable_is_refused() {
    let router = router();
    let (status_code, created) = create_stack(
        &router,
        "operation-events-refusals",
        &one_resource("refusals"),
        &[],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "the stack the rows read around must be created, got {status_code} {created}"
    );
    wait_for_the_transition().await;

    let rows = vec![
        Refusal {
            case: "no query criteria at all",
            members: vec![],
            message: "One of ChangeSetName, OperationId or StackName must be specified."
                .to_string(),
        },
        Refusal {
            case: "no query criteria, only a filter",
            members: vec![member("Filters.FailedEvents", "true")],
            message: "One of ChangeSetName, OperationId or StackName must be specified."
                .to_string(),
        },
        Refusal {
            case: "a stack name nothing holds",
            members: vec![member("StackName", "operation-events-no-such-stack")],
            message: "Stack with id operation-events-no-such-stack does not exist".to_string(),
        },
        Refusal {
            case: "a change set name nothing holds, on a stack that exists",
            members: vec![
                member("ChangeSetName", "operation-events-no-such-set"),
                member("StackName", "operation-events-refusals"),
            ],
            message: "ChangeSet [operation-events-no-such-set] does not exist".to_string(),
        },
    ];

    for row in rows {
        let (status_code, body) = describe_events(&router, &row.members).await;
        assert_eq!(
            status_code, 400,
            "case {:?}: the read must be refused, got {status_code} {body}",
            row.case
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {:?}: CloudFormation refuses a bad request with ValidationError, got {body}",
            row.case
        );
        assert_eq!(
            xml_text(&body, "Message").as_deref(),
            Some(row.message.as_str()),
            "case {:?}: expected the message {:?}, got {body}",
            row.case,
            row.message
        );
    }

    // A bare change set NAME with no stack beside it is refused too: only an ARN names a
    // change set on its own.
    let (status_code, body) = describe_events(
        &router,
        &[member("ChangeSetName", "operation-events-no-such-set")],
    )
    .await;
    assert_eq!(
        status_code, 400,
        "case \"a bare change set name with no stack\": the read must be refused, got \
         {status_code} {body}"
    );
    assert_eq!(
        xml_text(&body, "Code").as_deref(),
        Some("ValidationError"),
        "case \"a bare change set name with no stack\": refused with ValidationError, got \
         {body}"
    );
}
