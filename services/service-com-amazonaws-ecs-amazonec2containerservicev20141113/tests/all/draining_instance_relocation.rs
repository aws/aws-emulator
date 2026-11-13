// SPDX-License-Identifier: CC0-1.0

//! DRAINING a container instance relocates the service tasks it hosts, driven end to end
//! over the shared wire harness in [`crate::ecs_wire`].
//!
//! The rule under test, over the whole class rather than one request:
//!
//! > A task placed on a cluster's own registered capacity records the container instance
//! > that hosts it (`Task$containerInstanceArn`: "the ARN of the container instance that
//! > hosts the task"), and `UpdateContainerInstancesState` with `DRAINING` takes that
//! > instance out of service: "Amazon ECS prevents new tasks from being scheduled for
//! > placement on the container instance and replacement service tasks are started on other
//! > container instances in the cluster if the resources are available." So the service
//! > tasks on a draining instance are stopped and replaced on an instance that is still
//! > `ACTIVE`, no placement lands on the draining one, and "any tasks that you run manually
//! > (RunTask) aren't affected by the container instance transitioning to DRAINING status".
//!
//! The cases span which instance is drained (the one hosting the service task, and the one
//! hosting nothing), what kind of task is on it (a service's and a standalone one), and the
//! return to `ACTIVE` that makes the instance eligible again.
//!
//! Nothing here reaches into the state struct: every value asserted is read back through
//! `ListTasks`, `DescribeTasks` and `DescribeContainerInstances`.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    create_cluster, list, ok, register_definition, register_instance, service, string,
};

/// The cluster the two container instances are registered into.
const CLUSTER: &str = "draincluster";

/// The task-definition family the service runs, and the one the standalone task runs: that
/// one's container performs a long program, so the task stays up for the whole case rather
/// than ending the work it was placed to do part-way through it.
const FAMILY: &str = "drainfamily";
const STANDALONE_FAMILY: &str = "drainstandalone";

/// The service whose one task is relocated.
const SERVICE_NAME: &str = "drainservice";

/// The two EC2 instance ids the cluster's capacity is registered under.
const FIRST_INSTANCE: &str = "i-0dra1n0000000001";
const SECOND_INSTANCE: &str = "i-0dra1n0000000002";

/// How often a poll re-reads the surface it is waiting on, and how long it keeps trying: a
/// stop and the placement that replaces it are two clock-owed transitions, with generous
/// margin for a loaded test machine.
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const POLL_BUDGET: Duration = Duration::from_secs(30);

/// How many times an INVARIANT — something the rule says does not change — is looked at
/// before the case accepts it. Each look is a fresh read through the front door, which
/// materialises whatever the clock owes, so the invariant is checked against progress rather
/// than against a wait.
const LOOKS_FOR_AN_INVARIANT: usize = 5;

/// The tasks of `cluster` that are still wanted running, as `DescribeTasks` reports them.
async fn live_tasks(router: &MultiServiceRouter, case: &str) -> Vec<Value> {
    let listed = ok(
        router,
        "ListTasks",
        json!({ "cluster": CLUSTER, "desiredStatus": "RUNNING" }),
        case,
    )
    .await;
    let arns = list(&listed, "taskArns", case);
    if arns.is_empty() {
        return Vec::new();
    }
    let described = ok(
        router,
        "DescribeTasks",
        json!({ "cluster": CLUSTER, "tasks": arns }),
        case,
    )
    .await;
    list(&described, "tasks", case)
}

/// The `(taskArn, containerInstanceArn)` pairs of the service's live tasks, and the pairs of
/// the standalone ones, each sorted.
async fn hosted(router: &MultiServiceRouter, case: &str) -> (Vec<(String, String)>, Vec<String>) {
    let tasks = live_tasks(router, case).await;
    let mut service_tasks: Vec<(String, String)> = Vec::new();
    let mut standalone: Vec<String> = Vec::new();
    for task in &tasks {
        let arn = string(task, "taskArn", case);
        let host = task["containerInstanceArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if task["group"]
            .as_str()
            .is_some_and(|group| group == format!("service:{SERVICE_NAME}"))
        {
            service_tasks.push((arn, host));
        } else {
            standalone.push(arn);
        }
    }
    service_tasks.sort();
    standalone.sort();
    (service_tasks, standalone)
}

/// Wait until the service holds exactly one RUNNING task and answer `(taskArn, host)`.
async fn one_running_service_task(router: &MultiServiceRouter, case: &str) -> (String, String) {
    let deadline = Instant::now() + POLL_BUDGET;
    loop {
        let tasks = live_tasks(router, case).await;
        let running: Vec<&Value> = tasks
            .iter()
            .filter(|task| {
                task["lastStatus"].as_str() == Some("RUNNING")
                    && task["group"]
                        .as_str()
                        .is_some_and(|group| group == format!("service:{SERVICE_NAME}"))
            })
            .collect();
        if running.len() == 1 || Instant::now() >= deadline {
            assert_eq!(
                running.len(),
                1,
                "case {case:?}: expected the service to hold one running task, got {tasks:?}"
            );
            let task = running[0];
            let host = task["containerInstanceArn"].as_str().unwrap_or_default();
            assert!(
                !host.is_empty(),
                "case {case:?}: a task placed on the cluster's own capacity records the \
                 instance that hosts it, got {task}"
            );
            return (string(task, "taskArn", case), host.to_string());
        }
        ::tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Move `instance` to `status`, asserting the call reports the status it stored.
async fn set_state(router: &MultiServiceRouter, instance: &str, status: &str, case: &str) {
    let updated = ok(
        router,
        "UpdateContainerInstancesState",
        json!({
            "cluster": CLUSTER,
            "containerInstances": [instance],
            "status": status,
        }),
        case,
    )
    .await;
    let instances = list(&updated, "containerInstances", case);
    assert_eq!(
        instances.len(),
        1,
        "case {case:?}: the named instance must be answered, got {updated}"
    );
    assert_eq!(
        instances[0]["status"].as_str(),
        Some(status),
        "case {case:?}: the call reports the status it stored, got {updated}"
    );
}

#[tokio::test]
async fn draining_a_container_instance_relocates_the_service_task_it_hosts() {
    let router = service();
    let case = "setup";
    create_cluster(&router, CLUSTER, case).await;
    let first = register_instance(&router, CLUSTER, FIRST_INSTANCE, case).await;
    let second = register_instance(&router, CLUSTER, SECOND_INSTANCE, case).await;
    let instances: BTreeSet<String> = [first.clone(), second.clone()].into_iter().collect();
    assert_eq!(
        instances.len(),
        2,
        "case {case:?}: the cluster holds two distinct container instances, got {instances:?}"
    );
    register_definition(&router, FAMILY, case).await;
    ok(
        &router,
        "RegisterTaskDefinition",
        json!({
            "family": STANDALONE_FAMILY,
            "containerDefinitions": [{
                "name": "worker",
                "image": "public.ecr.aws/nginx/nginx:latest",
                "memory": 128,
                "command": ["sh", "-c", "sleep 300"],
            }],
        }),
        case,
    )
    .await;

    // One standalone task, which the drain must NOT touch, and a service of one task.
    let standalone_run = ok(
        &router,
        "RunTask",
        json!({ "cluster": CLUSTER, "taskDefinition": STANDALONE_FAMILY, "count": 1 }),
        case,
    )
    .await;
    let standalone_arn = string(&list(&standalone_run, "tasks", case)[0], "taskArn", case);
    ok(
        &router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": SERVICE_NAME,
            "taskDefinition": FAMILY,
            "desiredCount": 1,
            "launchType": "EC2",
        }),
        case,
    )
    .await;

    let (first_task, host) = one_running_service_task(&router, "the task before the drain").await;
    assert!(
        instances.contains(&host),
        "case {case:?}: the task is hosted by one of the cluster's instances, got {host:?}"
    );
    let other_host = instances
        .iter()
        .find(|instance| **instance != host)
        .cloned()
        .expect("the cluster holds a second instance");

    // Draining the instance that hosts NOTHING of the service moves nothing: the rule is
    // about the tasks ON the draining instance. The drain is made by the call, so the
    // invariant is asserted over repeated READS — each of which materialises whatever the
    // clock owes — rather than after a wait.
    let case = "draining an instance that hosts no service task";
    set_state(&router, &other_host, "DRAINING", case).await;
    for look in 0..LOOKS_FOR_AN_INVARIANT {
        let (still, _) = hosted(&router, case).await;
        assert_eq!(
            still,
            vec![(first_task.clone(), host.clone())],
            "case {case:?}: the service's task stays where it is (look {look})"
        );
    }
    set_state(&router, &other_host, "ACTIVE", case).await;

    // Draining the HOST of the task stops it and the scheduler replaces it on the instance
    // that is still ACTIVE — with no further call from anybody.
    let case = "draining the instance that hosts the service task";
    set_state(&router, &host, "DRAINING", case).await;
    let deadline = Instant::now() + POLL_BUDGET;
    let relocated = loop {
        let (service_tasks, _) = hosted(&router, case).await;
        let relocated = service_tasks
            .iter()
            .find(|(arn, placed_on)| *arn != first_task && *placed_on == other_host)
            .cloned();
        if relocated.is_some() || Instant::now() >= deadline {
            assert!(
                relocated.is_some(),
                "case {case:?}: expected a replacement task on the remaining ACTIVE \
                 instance, got {service_tasks:?}"
            );
            break relocated.expect("just asserted");
        }
        ::tokio::time::sleep(POLL_INTERVAL).await;
    };

    // The task that was on the draining instance is on its way out, with the reason the
    // drain gives it, and the service holds exactly its one replacement.
    let described = ok(
        &router,
        "DescribeTasks",
        json!({ "cluster": CLUSTER, "tasks": [first_task.clone()] }),
        case,
    )
    .await;
    let stopped = &list(&described, "tasks", case)[0];
    assert_eq!(
        stopped["desiredStatus"].as_str(),
        Some("STOPPED"),
        "case {case:?}: the task on the draining instance is asked to stop, got {stopped}"
    );
    assert_eq!(
        stopped["stopCode"].as_str(),
        Some("ServiceSchedulerInitiated"),
        "case {case:?}: the scheduler is what stopped it, got {stopped}"
    );
    assert!(
        stopped["stoppedReason"]
            .as_str()
            .is_some_and(|reason| reason.contains("draining")),
        "case {case:?}: the reason says why it was stopped, got {stopped}"
    );
    let (service_tasks, standalone) = hosted(&router, case).await;
    assert_eq!(
        service_tasks,
        vec![relocated],
        "case {case:?}: the service holds exactly its replacement task"
    );
    // "Any tasks that you run manually (RunTask) aren't affected by the container instance
    // transitioning to DRAINING status."
    assert!(
        standalone.contains(&standalone_arn),
        "case {case:?}: a standalone task is not stopped by the drain, got {standalone:?}"
    );
}
