// SPDX-License-Identifier: CC0-1.0

//! Routing an `ECS Task State Change` event by the status its CONTAINER exited with.
//!
//! The RULE under test: an `ECS Task State Change` event's `detail` is the task Amazon ECS
//! holds at the instant it moved — the same projection `DescribeTasks` answers with, so
//! `detail.containers[].exitCode`, `detail.stopCode` and `detail.stoppedReason` are the
//! task's own, and `detail.version` is the counter a task bumps on every change, so the
//! event of a later move carries a higher one than the event of an earlier move. A
//! container that has not ended has produced no exit status, so the event of a task that is
//! running carries none. EventBridge then routes each envelope by its `EventPattern`
//! ALONE: a pattern that constrains a member nested inside the `containers` array selects
//! the tasks whose containers hold that value and withholds the rest, so two rules
//! differing only in the exit status they name split the stopped tasks between their
//! targets and neither sees the other's.
//!
//! The rows span what the rule generalises over: the exit status itself — the success
//! status, the two failure statuses a shell's own utilities and a command's `exit` produce,
//! and the largest status a shell reports (`exit` takes its operand modulo 256, so
//! `0..=255` are the statuses a container can end with) — and HOW LONG the container was
//! live before it produced that status, from none at all to a program that runs for a span
//! first, so the status is produced while the task is already running. Each row's status is
//! named by its OWN rule, pointed at its OWN queue, and every row's task is placed in the
//! one cluster at the same time, so the routing is decided by the events alone.
//!
//! The exit statuses below are produced by REAL programs run through the container data
//! plane, and every value asserted is read back off the delivered envelope or through
//! `DescribeTasks` — nothing here reaches into the state struct.
//!
//! EventBridge and Amazon SQS are mounted BESIDE Amazon ECS on the same engine
//! ([`crate::task_state_change_events::emulator`]), so a publication travels the real
//! in-process callback, through the real bus, to a real queue.

use std::time::Instant;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{list, ok};
use crate::task_state_change_events::{
    cluster_arn_of, describe_task_in, drain, emulator, events_of, exit_code_pattern, given_queue,
    given_rule, grant_delivery, place_fargate_task, EVENT_SOURCE, POLL_INTERVAL, RUNNING,
    STANDALONE_RUN_POLL_BUDGET, STOPPED, TASK_STATE_CHANGE,
};

/// The cluster every task below runs in.
const CLUSTER: &str = "exitrouting";

/// The image every container below starts from. Nothing is fetched from it: what runs is
/// the `command` the revision states.
const IMAGE: &str = "public.ecr.aws/docker/library/busybox:latest";

/// One row: the status a task's container exits with, and so the status the rule that is to
/// receive that task's `STOPPED` event names.
struct Row {
    case: &'static str,
    /// The status the container's `exit` ends with — a shell reports `0..=255`.
    exit_code: i64,
}

/// The table spans the exit status the routing keys off.
const ROWS: &[Row] = &[
    Row {
        case: "a container that exits zero",
        exit_code: 0,
    },
    Row {
        case: "a container that exits one",
        exit_code: 1,
    },
    Row {
        case: "a container that exits seven",
        exit_code: 7,
    },
    Row {
        // The largest status a shell reports: `exit` takes its operand modulo 256.
        case: "a container that exits the largest status a shell reports",
        exit_code: 255,
    },
];

/// The ARN of the cluster this suite runs its tasks in.
fn cluster_arn() -> String {
    cluster_arn_of(CLUSTER)
}

/// The pattern of a rule that takes EVERY move of the cluster's tasks, whatever it moved
/// into: what the version and exit-status projection is read off.
fn every_move_pattern(cluster_arn: &str) -> Value {
    json!({
        "source": [EVENT_SOURCE],
        "detail-type": [TASK_STATE_CHANGE],
        "detail": { "clusterArn": [cluster_arn] },
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

/// The `detail.version` one event carries; `None` when it carries none.
fn version_of(event: &Value) -> Option<i64> {
    event["detail"]["version"].as_i64()
}

/// The first collected event reporting `status`.
fn event_in<'a>(events: &[&'a Value], status: &str) -> Option<&'a Value> {
    events
        .iter()
        .copied()
        .find(|event| event["detail"]["lastStatus"] == json!(status))
}

/// Register one FARGATE revision whose single, essential container runs `program` through a
/// shell, and place one task of it. Answers the task's ARN.
async fn place_task(
    router: &MultiServiceRouter,
    family: &str,
    program: &str,
    case: &str,
) -> String {
    place_fargate_task(
        router,
        CLUSTER,
        family,
        vec![json!({
            "name": "app",
            "image": IMAGE,
            "essential": true,
            "command": ["sh", "-c", program],
        })],
        case,
    )
    .await
}

/// One task, as `DescribeTasks` reports it.
async fn describe(router: &MultiServiceRouter, task_arn: &str, case: &str) -> Value {
    describe_task_in(router, CLUSTER, task_arn, case).await
}

#[tokio::test]
async fn a_stopped_task_event_reaches_only_the_rule_naming_its_container_exit_status() {
    let setup = "rules differing only in the exit status they name";
    let router = emulator();
    let cluster_arn = cluster_arn();
    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": CLUSTER }),
        setup,
    )
    .await;

    // One rule per row, each pointed at its own queue, and one rule taking every move of
    // the cluster's tasks.
    let mut scoped: Vec<String> = Vec::new();
    for row in ROWS {
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
        scoped.push(url);
    }
    let (every_move_url, every_move_arn) = given_queue(&router, setup, "every-move").await;
    grant_delivery(&router, setup, &every_move_url, &every_move_arn).await;
    given_rule(
        &router,
        setup,
        "every-task-move",
        every_move_pattern(&cluster_arn),
        &every_move_arn,
    )
    .await;

    // Every row's task is placed before any of them is waited on, so all of them run at
    // once and the routing is decided by the events alone.
    let mut task_arns: Vec<String> = Vec::new();
    for (index, row) in ROWS.iter().enumerate() {
        let family = format!("exitrouting{index}");
        let program = format!("exit {}", row.exit_code);
        task_arns.push(place_task(&router, &family, &program, row.case).await);
    }

    // ONLY the queues are polled from here: a client that watches a task through
    // EventBridge makes no further Amazon ECS call. The loop waits for the CONDITION — each
    // scoped queue holding its own task's stop, and every task's stop on the queue that
    // takes every move — and stops the instant it holds.
    let mut delivered: Vec<Vec<Value>> = ROWS.iter().map(|_| Vec::new()).collect();
    let mut moves: Vec<Value> = Vec::new();
    let deadline = Instant::now() + STANDALONE_RUN_POLL_BUDGET;
    loop {
        for (index, url) in scoped.iter().enumerate() {
            let batch = drain(&router, ROWS[index].case, url).await;
            delivered[index].extend(batch);
        }
        moves.extend(drain(&router, setup, &every_move_url).await);
        let settled = delivered.iter().all(|events| !events.is_empty())
            && task_arns.iter().all(|task_arn| {
                event_in(&events_of(&moves, task_arn), STOPPED).is_some()
                    && event_in(&events_of(&moves, task_arn), RUNNING).is_some()
            });
        if settled || Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let task_arn = &task_arns[index];
        let wanted = row.exit_code;
        let scoped_events = &delivered[index];
        assert!(
            !scoped_events.is_empty(),
            "case {case:?}: expected the rule naming exit status {wanted} to receive the \
             STOPPED event of task {task_arn}, got nothing"
        );
        // Every envelope this rule's target received is an ECS task state change about THIS
        // row's task, stopped, carrying THIS row's exit status: the sibling rules' tasks are
        // withheld from it.
        for event in scoped_events {
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
                event["detail"]["taskArn"],
                json!(task_arn),
                "case {case:?}: a rule naming exit status {wanted} must receive only the \
                 task that exited it ({task_arn}), got {event}"
            );
            assert_eq!(
                event["detail"]["lastStatus"],
                json!(STOPPED),
                "case {case:?}: expected a STOPPED event, got {event}"
            );
            assert_eq!(
                exit_codes_of(event),
                vec![wanted],
                "case {case:?}: expected the event's containers to report exit status \
                 {wanted}, got {event}"
            );
        }

        // What the event says about the stop is what the control plane says about it.
        let task = describe(&router, task_arn, case).await;
        let reported = reported_exit_codes(&task, case);
        let last = scoped_events
            .last()
            .unwrap_or_else(|| panic!("case {case:?}: expected a delivered STOPPED event"));
        assert_eq!(
            exit_codes_of(last),
            reported,
            "case {case:?}: the event's container exit statuses must be the task's, got \
             {last} and {task}"
        );
        assert_eq!(
            last["detail"]["stopCode"],
            task["stopCode"].clone(),
            "case {case:?}: the event's stopCode must be the task's, got {last} and {task}"
        );
        assert_eq!(
            last["detail"]["stoppedReason"],
            task["stoppedReason"].clone(),
            "case {case:?}: the event's stoppedReason must be the task's, got {last} and \
             {task}"
        );

        // The other rules' targets never saw this task.
        for (other, url) in scoped.iter().enumerate() {
            if other == index {
                continue;
            }
            let others = events_of(&delivered[other], task_arn);
            assert!(
                others.is_empty(),
                "case {case:?}: the rule naming exit status {} must not receive the task \
                 that exited {wanted} ({url}), got {others:?}",
                ROWS[other].exit_code
            );
        }

        // The run of this task, as the rule that takes every move saw it: the RUNNING event
        // carries a LOWER version than the STOPPED one — the counter the task bumps on
        // every change — and reports no exit status, because at that instant nothing of the
        // container had ended.
        let all = events_of(&moves, task_arn);
        let running = event_in(&all, RUNNING).unwrap_or_else(|| {
            panic!("case {case:?}: expected a RUNNING event for {task_arn}, got {all:?}")
        });
        let stopped = event_in(&all, STOPPED).unwrap_or_else(|| {
            panic!("case {case:?}: expected a STOPPED event for {task_arn}, got {all:?}")
        });
        let (running_version, stopped_version) = (version_of(running), version_of(stopped));
        assert!(
            running_version.is_some_and(|earlier| {
                stopped_version.is_some_and(|later| earlier < later)
            }),
            "case {case:?}: expected the RUNNING event's version below the STOPPED event's, \
             got {running_version:?} and {stopped_version:?} ({running} then {stopped})"
        );
        assert!(
            exit_codes_of(running).is_empty(),
            "case {case:?}: a task that is RUNNING has produced no exit status, got {running}"
        );
        assert_eq!(
            exit_codes_of(stopped),
            vec![wanted],
            "case {case:?}: expected the STOPPED event to report exit status {wanted}, got \
             {stopped}"
        );
    }
}

/// One row of the table below: the program a container runs, and the status that program
/// ends with — so the status named by the rule that is to receive the task's `STOPPED` event.
struct RanThenExitedRow {
    case: &'static str,
    /// The operand the container's shell runs. The container is LIVE for as long as the
    /// program waits before it ends, so the status it reports is produced after the task has
    /// already been running rather than at the instant it started.
    program: &'static str,
    /// The status that program ends with — a shell reports `0..=255`.
    exit_code: i64,
}

/// The table spans how long the container is live before it produces its status — not at
/// all, a span, a longer span — against the status it then ends with, success and failure
/// alike. Every wait is the container program's own, performed by the shell session behind
/// the container; nothing here waits on the test's thread outside a poll loop.
const RAN_THEN_EXITED_ROWS: &[RanThenExitedRow] = &[
    RanThenExitedRow {
        case: "a container that ends the moment it starts",
        program: "exit 4",
        exit_code: 4,
    },
    RanThenExitedRow {
        case: "a container that runs for a span and then fails",
        program: "sleep 1; exit 9",
        exit_code: 9,
    },
    RanThenExitedRow {
        case: "a container that runs for a longer span and then succeeds",
        program: "sleep 2; exit 0",
        exit_code: 0,
    },
];

/// The exit statuses one task's containers report through `DescribeTasks`.
fn reported_exit_codes(task: &Value, case: &str) -> Vec<i64> {
    list(task, "containers", case)
        .iter()
        .filter_map(|container| container["exitCode"].as_i64())
        .collect()
}

#[tokio::test]
async fn a_task_whose_container_ran_before_ending_is_routed_by_the_status_it_ended_with() {
    let setup = "rules naming the status a container that ran for a span ended with";
    let router = emulator();
    let cluster_arn = cluster_arn();
    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": CLUSTER }),
        setup,
    )
    .await;

    // One rule per row, each scoped to its own exit status and pointed at its own queue.
    let mut scoped: Vec<String> = Vec::new();
    for row in RAN_THEN_EXITED_ROWS {
        let name = format!("ran-exit-{}", row.exit_code);
        let (url, queue_arn) = given_queue(&router, row.case, &name).await;
        grant_delivery(&router, row.case, &url, &queue_arn).await;
        given_rule(
            &router,
            row.case,
            &format!("stopped-{name}"),
            exit_code_pattern(&cluster_arn, row.exit_code),
            &queue_arn,
        )
        .await;
        scoped.push(url);
    }

    // Every row's task is placed before any of them is waited on, so they run at once in the
    // one cluster and the routing is decided by the events alone.
    let mut task_arns: Vec<String> = Vec::new();
    for (index, row) in RAN_THEN_EXITED_ROWS.iter().enumerate() {
        let family = format!("ranexit{index}");
        task_arns.push(place_task(&router, &family, row.program, row.case).await);
    }

    // The CONTROL PLANE half, watched the way a client watches it: `DescribeTasks` is polled
    // until every task reports STOPPED, and what it then reports is what the delivered events
    // are held against below.
    let mut reported: Vec<Value> = Vec::new();
    let deadline = Instant::now() + STANDALONE_RUN_POLL_BUDGET;
    loop {
        reported.clear();
        for (index, task_arn) in task_arns.iter().enumerate() {
            reported.push(describe(&router, task_arn, RAN_THEN_EXITED_ROWS[index].case).await);
        }
        let settled = reported
            .iter()
            .all(|task| task["lastStatus"] == json!(STOPPED));
        if settled || Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    for (index, row) in RAN_THEN_EXITED_ROWS.iter().enumerate() {
        let (case, task) = (row.case, &reported[index]);
        assert_eq!(
            task["lastStatus"],
            json!(STOPPED),
            "case {case:?}: expected the task to reach STOPPED, got {task}"
        );
        assert_eq!(
            reported_exit_codes(task, case),
            vec![row.exit_code],
            "case {case:?}: expected the container to report exit status {}, got {task}",
            row.exit_code
        );
    }

    // The BUS half: each task's stop is on the queue of the rule naming the status its
    // container ended with, and on no other rule's queue.
    let mut delivered: Vec<Vec<Value>> = RAN_THEN_EXITED_ROWS.iter().map(|_| Vec::new()).collect();
    let deadline = Instant::now() + STANDALONE_RUN_POLL_BUDGET;
    loop {
        for (index, url) in scoped.iter().enumerate() {
            let batch = drain(&router, RAN_THEN_EXITED_ROWS[index].case, url).await;
            delivered[index].extend(batch);
        }
        let settled = delivered.iter().enumerate().all(|(index, events)| {
            event_in(&events_of(events, &task_arns[index]), STOPPED).is_some()
        });
        if settled || Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    for (index, row) in RAN_THEN_EXITED_ROWS.iter().enumerate() {
        let case = row.case;
        let task_arn = &task_arns[index];
        let wanted = row.exit_code;
        let mine = events_of(&delivered[index], task_arn);
        let stopped = event_in(&mine, STOPPED).unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected the rule naming exit status {wanted} to receive the \
                 STOPPED event of task {task_arn}, got {:?}",
                delivered[index]
            )
        });
        assert_eq!(
            stopped["source"],
            json!(EVENT_SOURCE),
            "case {case:?}: expected source {EVENT_SOURCE}, got {stopped}"
        );
        assert_eq!(
            stopped["detail-type"],
            json!(TASK_STATE_CHANGE),
            "case {case:?}: expected detail-type {TASK_STATE_CHANGE}, got {stopped}"
        );
        assert_eq!(
            exit_codes_of(stopped),
            vec![wanted],
            "case {case:?}: expected the STOPPED event to report exit status {wanted}, got \
             {stopped}"
        );

        // What the event reports about the stop is what the control plane reports about it.
        let task = &reported[index];
        assert_eq!(
            exit_codes_of(stopped),
            reported_exit_codes(task, case),
            "case {case:?}: the event's container exit statuses must be the task's, got \
             {stopped} and {task}"
        );
        assert_eq!(
            stopped["detail"]["stopCode"],
            task["stopCode"].clone(),
            "case {case:?}: the event's stopCode must be the task's, got {stopped} and {task}"
        );
        assert_eq!(
            stopped["detail"]["stoppedReason"],
            task["stoppedReason"].clone(),
            "case {case:?}: the event's stoppedReason must be the task's, got {stopped} and \
             {task}"
        );

        // Every envelope this rule's target received is about THIS row's task, and the
        // sibling rules' targets never saw it.
        for event in &delivered[index] {
            assert_eq!(
                event["detail"]["taskArn"],
                json!(task_arn),
                "case {case:?}: a rule naming exit status {wanted} must receive only the task \
                 that ended with it ({task_arn}), got {event}"
            );
        }
        for (other, url) in scoped.iter().enumerate() {
            if other == index {
                continue;
            }
            let others = events_of(&delivered[other], task_arn);
            assert!(
                others.is_empty(),
                "case {case:?}: the rule naming exit status {} must not receive the task that \
                 ended with {wanted} ({url}), got {others:?}",
                RAN_THEN_EXITED_ROWS[other].exit_code
            );
        }
    }
}
