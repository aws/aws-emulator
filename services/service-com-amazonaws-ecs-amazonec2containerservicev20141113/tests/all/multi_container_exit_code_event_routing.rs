// SPDX-License-Identifier: CC0-1.0

//! Routing a task of SEVERAL containers by the status ONE of them exited with.
//!
//! The RULE under test: an `ECS Task State Change` event's `detail` is the task Amazon ECS
//! holds at the instant it moved — the projection `DescribeTasks` answers with — so
//! `detail.containers` carries one entry per container of the task, each reporting that
//! container's OWN `exitCode`, essential or not, beside the task's `stopCode` and
//! `stoppedReason`. EventBridge then routes the envelope by its `EventPattern` alone, and a
//! pattern constraining a member nested inside an ARRAY selects an event when ANY element of
//! that array holds the value: a rule naming an exit status therefore receives every stopped
//! task one of whose containers ended with it, and no task whose containers all ended with
//! something else.
//!
//! The rows span what the rule generalises over: HOW MANY containers the task has (two,
//! three), whether their statuses are distinct or repeated, whether a success status sits
//! beside a failure, and whether the container that produced a status was essential. The
//! rules span the statuses named — one rule per status some row produces, each pointed at
//! its own queue, plus one naming a status no container of any row ends with, whose target
//! must stay empty.
//!
//! Every status below is produced by a REAL program run through the container data plane,
//! and every value asserted is read back off the delivered envelope or through
//! `DescribeTasks` — nothing here reaches into the state struct.
//!
//! EventBridge and Amazon SQS are mounted BESIDE Amazon ECS on the same engine
//! ([`crate::task_state_change_events::emulator`]), so a publication travels the real
//! in-process callback, through the real bus, to a real queue.

use std::collections::BTreeMap;
use std::time::Instant;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{list, ok};
use crate::task_state_change_events::{
    cluster_arn_of, describe_task_in, drain, emulator, events_of, exit_code_pattern, given_queue,
    given_rule, grant_delivery, place_fargate_task, EVENT_SOURCE, POLL_INTERVAL,
    STANDALONE_RUN_POLL_BUDGET, STOPPED, TASK_STATE_CHANGE,
};

/// The cluster every task below runs in.
const CLUSTER: &str = "multiexit";

/// The image every container below starts from. Nothing is fetched from it: what runs is the
/// `command` the revision states.
const IMAGE: &str = "public.ecr.aws/docker/library/busybox:latest";

/// One container of a row's task: the status its program ends with, and whether the task
/// treats it as essential.
struct ContainerRow {
    /// The status the container's `exit` ends with — a shell reports `0..=255`.
    exit_code: i64,
    essential: bool,
}

/// One row: a task and the containers it is made of.
struct Row {
    case: &'static str,
    containers: &'static [ContainerRow],
}

/// The table spans the SHAPE of the task's container list: how many containers it holds,
/// whether their statuses repeat, whether a success sits beside a failure, and whether the
/// container that produced a status was essential.
const ROWS: &[Row] = &[
    Row {
        case: "two containers that failed with different statuses",
        containers: &[
            ContainerRow {
                exit_code: 1,
                essential: true,
            },
            ContainerRow {
                exit_code: 2,
                essential: true,
            },
        ],
    },
    Row {
        case: "a container that succeeded beside one that failed",
        containers: &[
            ContainerRow {
                exit_code: 0,
                essential: true,
            },
            ContainerRow {
                exit_code: 9,
                essential: true,
            },
        ],
    },
    Row {
        case: "three containers, the failing one not essential",
        containers: &[
            ContainerRow {
                exit_code: 0,
                essential: true,
            },
            ContainerRow {
                exit_code: 0,
                essential: true,
            },
            ContainerRow {
                exit_code: 5,
                essential: false,
            },
        ],
    },
    Row {
        case: "two containers that failed with the same status",
        containers: &[
            ContainerRow {
                exit_code: 1,
                essential: true,
            },
            ContainerRow {
                exit_code: 1,
                essential: true,
            },
        ],
    },
];

/// The status named by the rule whose target must stay EMPTY: no container of any row above
/// ends with it, which the test asserts of the table before it places anything.
const UNPRODUCED_STATUS: i64 = 200;

/// The statuses a rule is created for: every status the rows produce, and the one none of
/// them does.
const NAMED_STATUSES: &[i64] = &[0, 1, 2, 5, 9, UNPRODUCED_STATUS];

/// The ARN of the cluster this suite runs its tasks in.
fn cluster_arn() -> String {
    cluster_arn_of(CLUSTER)
}

/// The name of the container at position `index` of a row's container list.
fn container_name(index: usize) -> String {
    format!("c{index}")
}

/// The exit status each named container of `holder` reports — `holder` being an event's
/// `detail` or a task as `DescribeTasks` answers with it, which carry the same projection. A
/// container that has produced no status contributes no entry.
fn exit_codes_by_container(holder: &Value, case: &str) -> BTreeMap<String, i64> {
    list(holder, "containers", case)
        .iter()
        .filter_map(|container| {
            let name = container["name"].as_str()?.to_string();
            Some((name, container["exitCode"].as_i64()?))
        })
        .collect()
}

/// The exit status each container of one row is to report, keyed the same way.
fn wanted_exit_codes(row: &Row) -> BTreeMap<String, i64> {
    row.containers
        .iter()
        .enumerate()
        .map(|(index, container)| (container_name(index), container.exit_code))
        .collect()
}

/// Whether one row's task has a container that ended with `status`.
fn row_produces(row: &Row, status: i64) -> bool {
    row.containers
        .iter()
        .any(|container| container.exit_code == status)
}

/// Register one FARGATE revision whose containers are the row's, each running its own `exit`
/// through a shell, and place one task of it. Answers the task's ARN.
async fn place_task(router: &MultiServiceRouter, family: &str, row: &Row, case: &str) -> String {
    let containers: Vec<Value> = row
        .containers
        .iter()
        .enumerate()
        .map(|(index, container)| {
            json!({
                "name": container_name(index),
                "image": IMAGE,
                "essential": container.essential,
                "command": ["sh", "-c", format!("exit {}", container.exit_code)],
            })
        })
        .collect();
    place_fargate_task(router, CLUSTER, family, containers, case).await
}

/// One task, as `DescribeTasks` reports it.
async fn describe(router: &MultiServiceRouter, task_arn: &str, case: &str) -> Value {
    describe_task_in(router, CLUSTER, task_arn, case).await
}

#[tokio::test]
async fn a_rule_naming_an_exit_status_receives_every_task_one_container_of_which_ended_with_it() {
    let setup = "rules naming one container exit status each";
    let router = emulator();
    let cluster_arn = cluster_arn();

    // The table's own premise: the status the empty-target rule names is produced by no row.
    assert!(
        ROWS.iter().all(|row| !row_produces(row, UNPRODUCED_STATUS)),
        "case {setup:?}: the status named by the rule that must stay empty \
         ({UNPRODUCED_STATUS}) is produced by one of the rows"
    );

    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": CLUSTER }),
        setup,
    )
    .await;

    // One rule per named status, each pointed at its own queue.
    let mut queues: Vec<String> = Vec::new();
    for status in NAMED_STATUSES {
        let name = format!("multiexit-{status}");
        let (url, queue_arn) = given_queue(&router, setup, &name).await;
        grant_delivery(&router, setup, &url, &queue_arn).await;
        given_rule(
            &router,
            setup,
            &format!("stopped-with-{name}"),
            exit_code_pattern(&cluster_arn, *status),
            &queue_arn,
        )
        .await;
        queues.push(url);
    }

    // Every row's task is placed before any of them is waited on, so all of them run at once
    // in the one cluster and the routing is decided by the events alone.
    let mut task_arns: Vec<String> = Vec::new();
    for (index, row) in ROWS.iter().enumerate() {
        task_arns.push(place_task(&router, &format!("multiexit{index}"), row, row.case).await);
    }

    // Which tasks each rule's target is to receive: those with a container that ended with
    // the status the rule names.
    let expected: Vec<Vec<String>> = NAMED_STATUSES
        .iter()
        .map(|status| {
            ROWS.iter()
                .enumerate()
                .filter(|(_, row)| row_produces(row, *status))
                .map(|(index, _)| task_arns[index].clone())
                .collect()
        })
        .collect();

    // ONLY the queues are polled from here: a client that watches a task through EventBridge
    // makes no further Amazon ECS call. The loop waits for the CONDITION — every expected
    // delivery present — and stops the instant it holds.
    let mut delivered: Vec<Vec<Value>> = NAMED_STATUSES.iter().map(|_| Vec::new()).collect();
    let deadline = Instant::now() + STANDALONE_RUN_POLL_BUDGET;
    loop {
        for (index, url) in queues.iter().enumerate() {
            let batch = drain(&router, setup, url).await;
            delivered[index].extend(batch);
        }
        let settled = expected.iter().enumerate().all(|(index, wanted)| {
            wanted
                .iter()
                .all(|task_arn| !events_of(&delivered[index], task_arn).is_empty())
        });
        if settled || Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    // What the control plane reports about each task, read once the events have arrived: what
    // the delivered envelopes are held against below. The read waits for the CONDITION that
    // every task is TERMINAL, so no further move of any of them is owed and no further event
    // of them can be published — which is what makes the emptiness asserted below a
    // statement about the routing rather than about how far the run had got.
    let mut reported: Vec<Value> = Vec::new();
    let deadline = Instant::now() + STANDALONE_RUN_POLL_BUDGET;
    loop {
        reported.clear();
        for (index, row) in ROWS.iter().enumerate() {
            reported.push(describe(&router, &task_arns[index], row.case).await);
        }
        let settled = reported
            .iter()
            .all(|task| task["lastStatus"] == json!(STOPPED));
        if settled || Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    // One last look at every queue, after the last publication any of these tasks can earn:
    // an envelope a rule should not have received is collected here even when it arrived
    // after the expected ones.
    for (index, url) in queues.iter().enumerate() {
        let batch = drain(&router, setup, url).await;
        delivered[index].extend(batch);
    }

    for (rule, status) in NAMED_STATUSES.iter().enumerate() {
        let events = &delivered[rule];
        if *status == UNPRODUCED_STATUS {
            assert!(
                events.is_empty(),
                "case {setup:?}: the rule naming exit status {status}, which no container of \
                 any task ended with, must receive nothing, got {events:?}"
            );
            continue;
        }
        // Every envelope this rule's target received is an ECS task state change about a
        // stopped task one of whose containers ended with the status the rule names — the
        // other tasks are withheld from it.
        for event in events {
            assert_eq!(
                event["source"],
                json!(EVENT_SOURCE),
                "case {setup:?}: expected source {EVENT_SOURCE}, got {event}"
            );
            assert_eq!(
                event["detail-type"],
                json!(TASK_STATE_CHANGE),
                "case {setup:?}: expected detail-type {TASK_STATE_CHANGE}, got {event}"
            );
            assert_eq!(
                event["detail"]["lastStatus"],
                json!(STOPPED),
                "case {setup:?}: expected a STOPPED event, got {event}"
            );
            let codes = exit_codes_by_container(&event["detail"], setup);
            assert!(
                codes.values().any(|reported| reported == status),
                "case {setup:?}: the rule naming exit status {status} must receive only a task \
                 a container of which ended with it, got {codes:?} in {event}"
            );
            let task_arn = event["detail"]["taskArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {setup:?}: no detail.taskArn in {event}"))
                .to_string();
            assert!(
                expected[rule].contains(&task_arn),
                "case {setup:?}: the rule naming exit status {status} must not receive task \
                 {task_arn}, got {event}"
            );
        }
        // And it received each of the tasks that did end with it.
        for task_arn in &expected[rule] {
            assert!(
                !events_of(events, task_arn).is_empty(),
                "case {setup:?}: expected the rule naming exit status {status} to receive the \
                 STOPPED event of task {task_arn}, got {events:?}"
            );
        }
    }

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let task_arn = &task_arns[index];
        let task = &reported[index];
        let wanted = wanted_exit_codes(row);
        // The task ran to its end, and every container of it ran its own program and reports
        // the status that program ended with.
        assert_eq!(
            task["lastStatus"],
            json!(STOPPED),
            "case {case:?}: expected the task to reach STOPPED, got {task}"
        );
        assert_eq!(
            exit_codes_by_container(task, case),
            wanted,
            "case {case:?}: expected the task's containers to report {wanted:?}, got {task}"
        );
        // And every rule naming one of those statuses was handed the task AS THE CONTROL
        // PLANE REPORTS IT: one entry per container, each with its own status, beside the
        // task's own account of the stop.
        for (rule, status) in NAMED_STATUSES.iter().enumerate() {
            if !row_produces(row, *status) {
                continue;
            }
            let mine = events_of(&delivered[rule], task_arn);
            let stopped = mine.first().copied().unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected the rule naming exit status {status} to receive \
                     the STOPPED event of task {task_arn}, got {:?}",
                    delivered[rule]
                )
            });
            assert_eq!(
                exit_codes_by_container(&stopped["detail"], case),
                wanted,
                "case {case:?}: the event's per-container exit statuses must be the task's, \
                 got {stopped} and {task}"
            );
            assert_eq!(
                stopped["detail"]["stopCode"],
                task["stopCode"].clone(),
                "case {case:?}: the event's stopCode must be the task's, got {stopped} and \
                 {task}"
            );
            assert_eq!(
                stopped["detail"]["stoppedReason"],
                task["stoppedReason"].clone(),
                "case {case:?}: the event's stoppedReason must be the task's, got {stopped} \
                 and {task}"
            );
        }
    }
}
