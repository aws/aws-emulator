// SPDX-License-Identifier: CC0-1.0

//! Routing an `ECS Task State Change` event by the STATUS the task moved into.
//!
//! The RULE under test: every status a task moves into is published as one `ECS Task State
//! Change` envelope whose `detail` is the task Amazon ECS holds at that instant — the
//! projection `DescribeTasks` answers with, so `detail.version` is the counter the task
//! bumps on every change, a container that has not ended reports no `exitCode`, and a stop
//! reports the `stopCode` and `stoppedReason` of that stop — and whose `resources` names
//! that task alone. EventBridge routes each envelope by its `EventPattern` ALONE, so a rule
//! naming ONE `detail.lastStatus` receives the envelope of every task that moved into that
//! status and no envelope of any other status: two tasks that pass through the same status
//! are JOINED on the rule naming it and SPLIT by the rules naming the exit statuses their
//! containers ended with.
//!
//! The rows span what the rule generalises over: WHICH status the rule names — every stage
//! of the ECS task lifecycle an `awsvpc` task passes through, from the one it is placed in
//! to the one it ends in — against two tasks of DIFFERENT task definitions whose containers
//! end with a failure status and a success status, placed into the one cluster at the same
//! time, so the routing is decided by the published envelopes alone.
//!
//! Every exit status below is produced by a REAL program run through the container data
//! plane, and every value asserted is read back off the delivered envelope or through
//! `DescribeTasks` — nothing here reaches into the state struct.
//!
//! EventBridge and Amazon SQS are mounted BESIDE Amazon ECS on the same engine
//! ([`crate::task_state_change_events::emulator`]), so a publication travels the real
//! in-process callback, through the real bus, to a real queue.

use std::time::Instant;

use serde_json::{json, Value};

use crate::ecs_wire::{list, ok};
use crate::task_state_change_events::{
    cluster_arn_of, describe_task_in, drain, emulator, events_of, exit_code_pattern, given_queue,
    given_rule, grant_delivery, place_fargate_task, EVENT_SOURCE, POLL_INTERVAL, RUNNING,
    STANDALONE_RUN_POLL_BUDGET, STOPPED, TASK_STATE_CHANGE,
};

/// The cluster every task below runs in.
const CLUSTER: &str = "statusrouting";

/// The image every container below starts from. Nothing is fetched from it: what runs is the
/// `command` the revision states.
const IMAGE: &str = "public.ecr.aws/docker/library/busybox:latest";

/// The stop an ESSENTIAL container's exit earns, from the model's `TaskStopCode` enum
/// (`smithy_query(query="shape", shape="com.amazonaws.ecs#TaskStopCode")` ->
/// `EssentialContainerExited`, `InfrastructureHealth`, `ServiceSchedulerInitiated`,
/// `SpotInterruption`, `TaskFailedToStart`, `TerminationNotice`, `UserInitiated`). Spelled
/// out here rather than imported, so a wrong constant in the implementation cannot make this
/// suite pass.
const ESSENTIAL_CONTAINER_EXITED: &str = "EssentialContainerExited";

/// One row: a stage of the ECS task lifecycle, and so the status named by the rule whose
/// target is to receive the envelope of that move.
struct StatusRow {
    case: &'static str,
    /// The `detail.lastStatus` the rule names.
    status: &'static str,
    /// Whether NOTHING of the task's containers can have ended by the time the task reports
    /// this status — so the envelope of this move carries no exit status at all.
    before_any_container_ended: bool,
}

/// The table spans the status the rule names: every stage of the ECS task lifecycle an
/// `awsvpc` task placed on its own passes through, in the order it passes through them (the
/// lifecycle the Amazon ECS developer guide publishes under "Task lifecycle", of which an
/// `awsvpc` task begins at `PROVISIONING` because Amazon ECS has an interface to put in
/// place before it can launch the task).
const STATUS_ROWS: &[StatusRow] = &[
    StatusRow {
        case: "a rule naming the status a task is placed in",
        status: "PROVISIONING",
        before_any_container_ended: true,
    },
    StatusRow {
        case: "a rule naming the status a placed task waits to start in",
        status: "PENDING",
        before_any_container_ended: true,
    },
    StatusRow {
        case: "a rule naming the status a started task runs in",
        status: RUNNING,
        before_any_container_ended: true,
    },
    StatusRow {
        case: "a rule naming the first status a task on its way out takes",
        status: "DEACTIVATING",
        // By this point the essential container's program has ended — that exit is WHY the
        // task is on its way out — so nothing is asserted about the exit statuses here.
        before_any_container_ended: false,
    },
    StatusRow {
        case: "a rule naming the status a task ends in",
        status: STOPPED,
        before_any_container_ended: false,
    },
];

/// One task placed into the cluster: the program its single essential container runs, and so
/// the status that container ends with.
struct TaskRow {
    case: &'static str,
    /// The family of the revision placed for this row — a revision of its OWN, so the two
    /// tasks share nothing but the cluster they run in.
    family: &'static str,
    /// The status the container's `exit` ends with — a shell reports `0..=255`.
    exit_code: i64,
}

/// The two tasks: one whose container fails, one whose container succeeds. Each runs for a
/// span before it ends, so its exit status is produced while the task is already running.
const TASK_ROWS: &[TaskRow] = &[
    TaskRow {
        case: "a task whose essential container fails",
        family: "statusroutingfail",
        exit_code: 7,
    },
    TaskRow {
        case: "a task whose essential container succeeds",
        family: "statusroutingpass",
        exit_code: 0,
    },
];

/// The pattern of a rule scoped to the tasks of ONE cluster that moved into `status`: the
/// only thing it says about the task is the status it now reports.
fn status_pattern(cluster_arn: &str, status: &str) -> Value {
    json!({
        "source": [EVENT_SOURCE],
        "detail-type": [TASK_STATE_CHANGE],
        "detail": { "clusterArn": [cluster_arn], "lastStatus": [status] },
    })
}

/// The exit statuses the containers of one event's `detail` report, in order. A container
/// that has produced none carries no `exitCode` member at all, so it contributes nothing.
fn exit_codes_of(event: &Value) -> Vec<i64> {
    event["detail"]["containers"]
        .as_array()
        .map(|containers| {
            containers
                .iter()
                .filter_map(|container| container["exitCode"].as_i64())
                .collect()
        })
        .unwrap_or_default()
}

/// The exit statuses one task's containers report through `DescribeTasks`.
fn reported_exit_codes(task: &Value, case: &str) -> Vec<i64> {
    list(task, "containers", case)
        .iter()
        .filter_map(|container| container["exitCode"].as_i64())
        .collect()
}

/// The `detail.version` one event carries; `None` when it carries none.
fn version_of(event: &Value) -> Option<i64> {
    event["detail"]["version"].as_i64()
}

#[tokio::test]
async fn a_rule_naming_one_task_status_receives_every_task_that_moved_into_it_and_nothing_else() {
    let setup = "rules differing only in the task status they name";
    let router = emulator();
    let cluster_arn = cluster_arn_of(CLUSTER);
    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": CLUSTER }),
        setup,
    )
    .await;

    // One rule per status, each pointed at its own queue.
    let mut status_queues: Vec<String> = Vec::new();
    for row in STATUS_ROWS {
        let name = row.status.to_ascii_lowercase();
        let (url, queue_arn) = given_queue(&router, row.case, &format!("status-{name}")).await;
        grant_delivery(&router, row.case, &url, &queue_arn).await;
        given_rule(
            &router,
            row.case,
            &format!("moved-to-{name}"),
            status_pattern(&cluster_arn, row.status),
            &queue_arn,
        )
        .await;
        status_queues.push(url);
    }

    // And one rule per task, scoped to the stop of a task whose container ended with THAT
    // task's exit status: the rules the two tasks are split by.
    let mut exit_queues: Vec<String> = Vec::new();
    for row in TASK_ROWS {
        let name = format!("exit-{}", row.exit_code);
        let (url, queue_arn) = given_queue(&router, row.case, &name).await;
        grant_delivery(&router, row.case, &url, &queue_arn).await;
        given_rule(
            &router,
            row.case,
            &format!("stopped-with-{name}"),
            exit_code_pattern(&cluster_arn, row.exit_code),
            &queue_arn,
        )
        .await;
        exit_queues.push(url);
    }

    // Both tasks are placed before either is waited on, so they run at the same time in the
    // one cluster and every rule's routing is decided by the events alone.
    let mut task_arns: Vec<String> = Vec::new();
    for row in TASK_ROWS {
        let task_arn = place_fargate_task(
            &router,
            CLUSTER,
            row.family,
            vec![json!({
                "name": "app",
                "image": IMAGE,
                "essential": true,
                "command": ["sh", "-c", format!("sleep 1; exit {}", row.exit_code)],
            })],
            row.case,
        )
        .await;
        task_arns.push(task_arn);
    }

    // ONLY the queues are polled from here: a client that watches a task through
    // EventBridge makes no further Amazon ECS call. The loop waits for the CONDITION — every
    // status rule holding both tasks' move into its status, and every exit rule holding its
    // own task's stop — and stops the instant it holds.
    let mut by_status: Vec<Vec<Value>> = STATUS_ROWS.iter().map(|_| Vec::new()).collect();
    let mut by_exit: Vec<Vec<Value>> = TASK_ROWS.iter().map(|_| Vec::new()).collect();
    let deadline = Instant::now() + STANDALONE_RUN_POLL_BUDGET;
    loop {
        for (index, url) in status_queues.iter().enumerate() {
            let batch = drain(&router, STATUS_ROWS[index].case, url).await;
            by_status[index].extend(batch);
        }
        for (index, url) in exit_queues.iter().enumerate() {
            let batch = drain(&router, TASK_ROWS[index].case, url).await;
            by_exit[index].extend(batch);
        }
        let statuses_settled = by_status.iter().all(|delivered| {
            task_arns
                .iter()
                .all(|task_arn| !events_of(delivered, task_arn).is_empty())
        });
        let exits_settled = by_exit
            .iter()
            .enumerate()
            .all(|(index, delivered)| !events_of(delivered, &task_arns[index]).is_empty());
        if (statuses_settled && exits_settled) || Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    // Each status rule took the move of BOTH tasks into the status it names, and took no
    // move into any other status: the join.
    for (status_index, status_row) in STATUS_ROWS.iter().enumerate() {
        let case = status_row.case;
        let wanted = status_row.status;
        let delivered = &by_status[status_index];
        for event in delivered {
            assert_eq!(
                event["source"],
                json!(EVENT_SOURCE),
                "case {case:?}: expected source {EVENT_SOURCE}, got {event}"
            );
            assert_eq!(
                event["detail-type"],
                json!(TASK_STATE_CHANGE),
                "case {case:?}: expected detail-type {TASK_STATE_CHANGE}, got {event}"
            );
            assert_eq!(
                event["detail"]["lastStatus"],
                json!(wanted),
                "case {case:?}: a rule naming {wanted} must receive no move into any other \
                 status, got {event}"
            );
            assert_eq!(
                event["detail"]["clusterArn"],
                json!(cluster_arn.clone()),
                "case {case:?}: expected detail.clusterArn {cluster_arn}, got {event}"
            );
        }
        for (task_index, task_row) in TASK_ROWS.iter().enumerate() {
            let task_arn = &task_arns[task_index];
            let mine = events_of(delivered, task_arn);
            assert_eq!(
                mine.len(),
                1,
                "case {case:?}/{}: expected the rule naming {wanted} to receive exactly one \
                 envelope for task {task_arn}, got {mine:?}",
                task_row.case
            );
            let event = mine[0];
            assert_eq!(
                event["resources"],
                json!([task_arn.clone()]),
                "case {case:?}/{}: expected the task's ARN alone in resources, got {event}",
                task_row.case
            );
            if status_row.before_any_container_ended {
                assert!(
                    exit_codes_of(event).is_empty(),
                    "case {case:?}/{}: a task reporting {wanted} has produced no exit status, \
                     got {event}",
                    task_row.case
                );
            }
        }
    }

    // Each task's own run, read off the envelopes the status rules delivered: the version
    // the task reports RISES with every move, so an earlier stage's envelope carries a lower
    // one than a later stage's.
    for (task_index, task_row) in TASK_ROWS.iter().enumerate() {
        let case = task_row.case;
        let task_arn = &task_arns[task_index];
        let mut previous: Option<(&str, i64)> = None;
        for (status_index, status_row) in STATUS_ROWS.iter().enumerate() {
            let mine = events_of(&by_status[status_index], task_arn);
            let event = mine[0];
            let version = version_of(event).unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected the {} envelope to report a version, got {event}",
                    status_row.status
                )
            });
            if let Some((earlier_status, earlier)) = previous {
                assert!(
                    earlier < version,
                    "case {case:?}: expected the {earlier_status} envelope's version below \
                     the {} envelope's, got {earlier} and {version}",
                    status_row.status
                );
            }
            previous = Some((status_row.status, version));
        }
    }

    // Each exit rule took the stop of ITS task and nothing of the other's: the split. What
    // the stop envelope says about the stop is what the control plane says about it.
    for (task_index, task_row) in TASK_ROWS.iter().enumerate() {
        let case = task_row.case;
        let task_arn = &task_arns[task_index];
        let wanted = task_row.exit_code;
        let mine = events_of(&by_exit[task_index], task_arn);
        assert_eq!(
            mine.len(),
            1,
            "case {case:?}: expected the rule naming exit status {wanted} to receive the one \
             STOPPED envelope of task {task_arn}, got {:?}",
            by_exit[task_index]
        );
        let stopped = mine[0];
        assert_eq!(
            stopped["detail"]["lastStatus"],
            json!(STOPPED),
            "case {case:?}: expected a STOPPED envelope, got {stopped}"
        );
        assert_eq!(
            exit_codes_of(stopped),
            vec![wanted],
            "case {case:?}: expected the envelope's containers to report exit status {wanted}, \
             got {stopped}"
        );

        let task = describe_task_in(&router, CLUSTER, task_arn, case).await;
        assert_eq!(
            exit_codes_of(stopped),
            reported_exit_codes(&task, case),
            "case {case:?}: the envelope's container exit statuses must be the task's, got \
             {stopped} and {task}"
        );
        assert_eq!(
            stopped["detail"]["stopCode"],
            json!(ESSENTIAL_CONTAINER_EXITED),
            "case {case:?}: a task whose ESSENTIAL container exited stops with \
             {ESSENTIAL_CONTAINER_EXITED}, got {stopped}"
        );
        assert_eq!(
            stopped["detail"]["stopCode"],
            task["stopCode"].clone(),
            "case {case:?}: the envelope's stopCode must be the task's, got {stopped} and {task}"
        );
        assert_eq!(
            stopped["detail"]["stoppedReason"],
            task["stoppedReason"].clone(),
            "case {case:?}: the envelope's stoppedReason must be the task's, got {stopped} and \
             {task}"
        );

        // The sibling exit rule's target never saw this task.
        for (other, url) in exit_queues.iter().enumerate() {
            if other == task_index {
                continue;
            }
            let others = events_of(&by_exit[other], task_arn);
            assert!(
                others.is_empty(),
                "case {case:?}: the rule naming exit status {} must not receive the task that \
                 exited {wanted} ({url}), got {others:?}",
                TASK_ROWS[other].exit_code
            );
        }
    }
}
