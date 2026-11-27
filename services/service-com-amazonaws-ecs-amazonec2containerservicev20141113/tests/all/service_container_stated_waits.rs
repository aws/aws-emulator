// SPDX-License-Identifier: CC0-1.0

//! What a container of a SERVICE's task given nothing but a wait does to that service's steady
//! state.
//!
//! RULE under test: **a container given nothing but a wait is a container with nothing to do
//! but wait, so the seconds it names are the span it runs for — in either spelling of such a
//! program, the argument vector an image exec's and the SHELL spelling of it
//! (`["sh", "-c", "sleep 3600"]`) alike. So a service whose containers are given such a program
//! reaches the `desiredCount` its caller asked for and HOLDS it; a container that exited at once
//! would leave it replacing a task for ever and a `runningCount` it never reaches.**
//!
//! Why this suite is beside [`crate::container_whole_program_wait`], which pins the same
//! reading on a task run on its own: what a SERVICE does with such a container — the count it
//! then holds — is the premise every rollout of this crate rests on, and the one an Application
//! Auto Scaling movement of the `ecs:service:DesiredCount` dimension is fulfilled by. A wait
//! inside a program that does something ELSE is spent just the same, wherever it stands
//! ([`crate::container_program_waits`]), which for a service's task is what
//! [`crate::zz_tmp_diag`] pins.
//!
//! Sources: `smithy_query(query="operation",operation="DescribeServices")` for
//! `Service$runningCount` ("the number of tasks in the cluster that are in the RUNNING state")
//! and `Service$desiredCount` ("the desired number of instantiations of the task definition to
//! keep running on the service"); `smithy_query(query="operation",operation="UpdateService")`
//! for `UpdateService$desiredCount`; `smithy_query(query="operation",operation="DescribeTasks")`
//! for `Container$exitCode` ("the exit code returned from the container") and `Task$stopCode`;
//! `smithy_query(query="shape",shape="TaskStopCode")` for the code a task whose essential
//! container exited stops with. POSIX states what the wait means for the program ("the `sleep`
//! utility shall suspend execution for at least the integral number of seconds specified by
//! the `time` operand").
//!
//! The rows differ along what the rule generalises over: the FORM the program is written in
//! (the vector the image exec's, a shell program), HOW LONG the wait names (an hour, five
//! minutes, fractional seconds, no end of its own, one second round a loop) and HOW MANY tasks
//! the service holds while its containers are waiting.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{cluster_with_capacity, count, list, ok, present_count, service, string};

/// The cluster every service below runs in.
const CLUSTER: &str = "maintainedwaits";

/// The image every container below names. Nothing is fetched from it: what runs is the
/// `command` the revision states.
const IMAGE: &str = "public.ecr.aws/nginx/nginx:latest";

/// How often a poll re-reads, and how long it keeps trying. The budget is generous — every
/// task of a service comes up on the clock, one span at a time — and every poll stops the
/// instant its own condition holds.
const POLL_INTERVAL: Duration = Duration::from_millis(60);
const POLL_BUDGET: Duration = Duration::from_secs(90);

/// How long the services are watched once they hold the count they want, re-read INSIDE the
/// loop that spends it: a container whose wait was read as an account of work it did exits, and
/// the scheduler then replaces the task, so a count that is still held read after read is the
/// count being kept rather than one instant that happened to look right. Every row's wait is
/// far longer than this window.
const WATCH_WINDOW: Duration = Duration::from_secs(3);

/// `Task$stopCode` of a task whose essential container exited.
const ESSENTIAL_CONTAINER_EXITED: &str = "EssentialContainerExited";

/// One row: the `command` the service's container is given and how many tasks the service wants
/// while that program is waiting.
struct Row {
    case: &'static str,
    command: &'static [&'static str],
    desired: i64,
}

const ROWS: &[Row] = &[
    Row {
        // The recorded shape: a shell program that holds nothing but an hour's wait.
        case: "a shell program that holds nothing but an hour's wait",
        command: &["/bin/sh", "-c", "sleep 3600"],
        desired: 1,
    },
    Row {
        // The same program written as the vector the image exec's.
        case: "the same wait as the vector the image exec's",
        command: &["sleep", "3600"],
        desired: 2,
    },
    Row {
        case: "a shell program that holds nothing but a wait in fractional seconds",
        command: &["/bin/sh", "-c", "sleep 240.5"],
        desired: 1,
    },
    Row {
        case: "a shell program that holds nothing but a wait with no end of its own",
        command: &["sh", "-c", "sleep infinity"],
        desired: 2,
    },
    Row {
        case: "a wait the program goes round a loop on",
        command: &["sh", "-c", "while true; do sleep 1; done"],
        desired: 1,
    },
];

/// Register one revision of `family` whose single essential container runs `command`.
async fn register(router: &MultiServiceRouter, family: &str, command: &[&str], case: &str) {
    ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
            "containerDefinitions": [{
                "name": "app",
                "image": IMAGE,
                "memory": 128,
                "essential": true,
                "command": command,
            }],
        }),
        case,
    )
    .await;
}

/// Create one service of `family` that wants `desired` tasks.
async fn create_service(
    router: &MultiServiceRouter,
    name: &str,
    family: &str,
    desired: i64,
    case: &str,
) {
    ok(
        router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": name,
            "taskDefinition": family,
            "desiredCount": desired,
            "launchType": "EC2",
        }),
        case,
    )
    .await;
}

/// The service `DescribeServices` reports.
async fn describe(router: &MultiServiceRouter, name: &str, case: &str) -> Value {
    let body = ok(
        router,
        "DescribeServices",
        json!({ "cluster": CLUSTER, "services": [name] }),
        case,
    )
    .await;
    list(&body, "services", case)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: expected the service back, got {body}"))
}

/// The counts a service reports: what it wants, and what it is running.
async fn counts(router: &MultiServiceRouter, name: &str, case: &str) -> (i64, i64) {
    let view = describe(router, name, case).await;
    (
        count(&view, "desiredCount", case),
        count(&view, "runningCount", case),
    )
}

/// Every task of one service, as `DescribeTasks` reports it — the ones it still wants running
/// and the ones it has stopped.
async fn tasks_of(router: &MultiServiceRouter, name: &str, case: &str) -> Vec<Value> {
    let mut described = Vec::new();
    for desired_status in ["RUNNING", "STOPPED"] {
        let listed = ok(
            router,
            "ListTasks",
            json!({
                "cluster": CLUSTER,
                "serviceName": name,
                "desiredStatus": desired_status,
            }),
            case,
        )
        .await;
        let arns: Vec<Value> = list(&listed, "taskArns", case);
        if arns.is_empty() {
            continue;
        }
        let body = ok(
            router,
            "DescribeTasks",
            json!({ "cluster": CLUSTER, "tasks": arns }),
            case,
        )
        .await;
        described.extend(list(&body, "tasks", case));
    }
    described
}

/// Assert that no task of the service reported an exit status and none was stopped because an
/// essential container of it exited.
async fn assert_nothing_exited(router: &MultiServiceRouter, name: &str, case: &str) {
    for task in tasks_of(router, name, case).await {
        let stop_code = task["stopCode"].as_str().unwrap_or_default().to_string();
        assert_ne!(
            stop_code, ESSENTIAL_CONTAINER_EXITED,
            "case {case:?}: a container that is still waiting has not exited, so no task of \
             the service is stopped for one, got {task}"
        );
        for container in list(&task, "containers", case) {
            assert_eq!(
                present_count(&container, "exitCode", case),
                None,
                "case {case:?}: no exit status is reported for a program that has not ended, \
                 got {container}"
            );
        }
    }
}

/// Re-read one service until it is running the count it wants, and answer the counts it
/// settled on.
async fn until_converged(
    router: &MultiServiceRouter,
    name: &str,
    desired: i64,
    case: &str,
) -> (i64, i64) {
    let deadline = Instant::now() + POLL_BUDGET;
    loop {
        let reading = counts(router, name, case).await;
        if reading == (desired, desired) {
            return reading;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the service to run the {desired} task(s) it wants, got \
             {reading:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// RULE: a service whose containers are given nothing but a wait holds the count its caller
/// asked for, in either form that program is written in.
#[tokio::test]
async fn a_service_of_waiting_containers_holds_the_count_it_was_asked_for() {
    let router = service();
    cluster_with_capacity(&router, CLUSTER, "maintained waits").await;

    // Every row's service is created FIRST and read afterwards: the containers run in parallel
    // in the one emulator, so the suite waits for the slowest of them rather than for their
    // sum.
    let mut created: Vec<(&Row, String)> = Vec::new();
    for (index, row) in ROWS.iter().enumerate() {
        let family = format!("maintainedwait{index}");
        let name = format!("maintained{index}");
        register(&router, &family, row.command, row.case).await;
        create_service(&router, &name, &family, row.desired, row.case).await;
        created.push((row, name));
    }

    for (row, name) in &created {
        let settled = until_converged(&router, name, row.desired, row.case).await;
        assert_eq!(
            settled,
            (row.desired, row.desired),
            "case {:?}: expected the service to run the count it wants, got {settled:?}",
            row.case
        );
    }

    // And it HOLDS that count: watched in ONE loop over every row, so the window is spent once
    // rather than once per row, and re-read inside it.
    let until = Instant::now() + WATCH_WINDOW;
    while Instant::now() < until {
        tokio::time::sleep(POLL_INTERVAL).await;
        for (row, name) in &created {
            let case = row.case;
            let reading = counts(&router, name, case).await;
            assert_eq!(
                reading,
                (row.desired, row.desired),
                "case {case:?}: a container that is still waiting stays up, so the service \
                 goes on running the count it wants, got {reading:?}"
            );
            assert_nothing_exited(&router, name, case).await;
        }
    }
}

/// The count a service is SCALED to is the count it then holds — the path Application Auto
/// Scaling drives through `UpdateService$desiredCount` ("the number of instantiations of the
/// task definition to place and keep running in your service").
#[tokio::test]
async fn a_service_scaled_up_holds_the_count_it_was_scaled_to() {
    let case = "a service of waiting containers scaled from one task to two";
    let router = service();
    cluster_with_capacity(&router, CLUSTER, case).await;
    register(&router, "scaledwait", &["/bin/sh", "-c", "sleep 3600"], case).await;
    create_service(&router, "scaled", "scaledwait", 1, case).await;
    until_converged(&router, "scaled", 1, case).await;

    let updated = ok(
        &router,
        "UpdateService",
        json!({ "cluster": CLUSTER, "service": "scaled", "desiredCount": 2 }),
        case,
    )
    .await;
    assert_eq!(
        count(&updated["service"], "desiredCount", case),
        2,
        "case {case:?}: the update keeps the count the caller asked for, got {updated}"
    );
    assert_eq!(
        string(&updated["service"], "serviceName", case),
        "scaled",
        "case {case:?}: the update answers the service it moved, got {updated}"
    );

    let settled = until_converged(&router, "scaled", 2, case).await;
    assert_eq!(
        settled,
        (2, 2),
        "case {case:?}: expected the scaled service to run both tasks, got {settled:?}"
    );
    let until = Instant::now() + WATCH_WINDOW;
    while Instant::now() < until {
        tokio::time::sleep(POLL_INTERVAL).await;
        let reading = counts(&router, "scaled", case).await;
        assert_eq!(
            reading,
            (2, 2),
            "case {case:?}: expected the scaled service to hold both tasks, got {reading:?}"
        );
        assert_nothing_exited(&router, "scaled", case).await;
    }
}
