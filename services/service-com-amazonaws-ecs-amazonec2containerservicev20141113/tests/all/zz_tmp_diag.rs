// SPDX-License-Identifier: CC0-1.0

//! WHICH container program lets an Amazon ECS service keep the task it placed, driven end to
//! end over the shared wire harness in [`crate::ecs_wire`].
//!
//! The rule under test:
//!
//! > A service's rollout is stable only while the tasks it placed are RUNNING, and a task runs
//! > only while its essential container's program does. Every wait a container's program names
//! > is one the container is up for: a program that goes round a LOOP on a wait and one that
//! > ENDS on a wait of so many seconds both keep their container up, and the service's rollout
//! > comes to rest with its desired count running either way. (The same reading on a task run
//! > on its OWN is what `crate::container_program_waits` pins.)
//!
//! The docs the rule is read off:
//!
//! * `Task$stopCode` -> `TaskStopCode` (`smithy_query(query="shape",shape="TaskStopCode")`:
//!   TaskFailedToStart, EssentialContainerExited, UserInitiated, ServiceSchedulerInitiated,
//!   SpotInterruption, TerminationNotice, InfrastructureHealth) and `Task$stoppedReason`,
//!   "The reason that the task was stopped" — the stop of a container that really ended, which
//!   is what neither program below earns.
//! * `Container$exitCode`, "The exit code returned from the container".
//! * `CreateService`, "runs and maintains your desired number of tasks from a specified task
//!   definition" — what makes the end of such a container nobody's verdict.
//! * `ServiceDeployment$status` -> `ServiceDeploymentStatus`, whose `SUCCESSFUL` is where a
//!   rollout that stabilised comes to rest, and `Service$runningCount`, "The number of tasks in
//!   the cluster that are in the RUNNING state".
//!
//! The rows differ along the FORM the program's wait takes — one the program goes round a loop
//! on, and one the program ends on after a step of its own — and each is asserted on a
//! CONDITION a poll waits for, never on how long anything took. The sibling suite
//! `crate::container_program_waits` pins the same reading on a task run on its own; this one
//! pins what a program's wait means for the SERVICE that placed the task, which is the premise
//! every rollout suite of this crate rests on.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{cluster_with_capacity, list, ok, poll_until, service, string};

/// The cluster both services below run in.
const CLUSTER: &str = "staysupcluster";

/// A program whose wait is one it goes round a LOOP on, which is the container that stays up.
const LOOPS: &str = "echo up; while true; do sleep 1; done";

/// A program that ENDS on a wait of so many seconds: the container is up for the seconds it
/// named, so the service holds the task it placed throughout.
const STATES_A_WAIT: &str = "echo up; sleep 600";

/// The `ServiceDeploymentStatus` a rollout that stabilised comes to rest in.
const SUCCESSFUL: &str = "SUCCESSFUL";

/// Register one revision of `family` whose single essential container runs `program` under a
/// shell, and answer its ARN.
async fn register(
    router: &MultiServiceRouter,
    family: &str,
    program: &str,
    case: &str,
) -> String {
    let body = ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
            "containerDefinitions": [{
                "name": "app",
                "image": "public.ecr.aws/nginx/nginx:latest",
                // "at least one of 'memory' or 'memoryReservation' must be specified" for a
                // container of a revision this service places.
                "memory": 128,
                "essential": true,
                "command": ["sh", "-c", program],
            }],
        }),
        case,
    )
    .await;
    string(&body["taskDefinition"], "taskDefinitionArn", case)
}

/// Create a service of one task on `definition`, and answer the ARN of the service deployment
/// its release is recorded as.
async fn create_service(
    router: &MultiServiceRouter,
    name: &str,
    definition: &str,
    case: &str,
) -> String {
    let body = ok(
        router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": name,
            "taskDefinition": definition,
            "desiredCount": 1,
            "launchType": "EC2",
        }),
        case,
    )
    .await;
    string(&body["service"], "currentServiceDeployment", case)
}

/// The service, as `DescribeServices` reports it.
async fn describe_service(router: &MultiServiceRouter, name: &str, case: &str) -> Value {
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

/// The rollout, as `DescribeServiceDeployments` reports it.
async fn describe_deployment(router: &MultiServiceRouter, arn: &str, case: &str) -> Value {
    let body = ok(
        router,
        "DescribeServiceDeployments",
        json!({ "serviceDeploymentArns": [arn] }),
        case,
    )
    .await;
    list(&body, "serviceDeployments", case)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: expected the rollout {arn} back, got {body}"))
}

/// The STOPPED tasks of the service, as `ListTasks` reports them: what a container that really
/// ended would leave behind.
async fn stopped_tasks(router: &MultiServiceRouter, name: &str, case: &str) -> Vec<String> {
    let listed = ok(
        router,
        "ListTasks",
        json!({ "cluster": CLUSTER, "family": name, "desiredStatus": "STOPPED" }),
        case,
    )
    .await;
    listed["taskArns"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|arn| arn.as_str().map(str::to_string))
        .collect()
}

/// RULE: a wait the program goes round a LOOP on keeps the container up, so the service's
/// rollout comes to rest SUCCESSFUL with its desired count running.
#[tokio::test]
async fn a_rollout_stabilises_on_a_program_whose_wait_is_a_loop() {
    let case = "a program whose wait is one it goes round a loop on";
    let router = service();
    cluster_with_capacity(&router, CLUSTER, case).await;
    let definition = register(&router, "loopingfamily", LOOPS, case).await;
    let released = create_service(&router, "loopingfamily", &definition, case).await;

    let deployment = poll_until(
        case,
        &format!("the rollout to reach {SUCCESSFUL:?}"),
        || describe_deployment(&router, &released, case),
        |deployment| deployment["status"] == json!(SUCCESSFUL),
    )
    .await;
    assert_eq!(
        deployment["targetServiceRevision"]["runningTaskCount"].as_i64(),
        Some(1),
        "case {case:?}: expected the rollout's revision to hold its one running task, got \
         {deployment}"
    );
    let described = describe_service(&router, "loopingfamily", case).await;
    assert_eq!(
        described["desiredCount"].as_i64(),
        Some(1),
        "case {case:?}: expected the service to want its one task, got {described}"
    );
    assert_eq!(
        described["runningCount"].as_i64(),
        Some(1),
        "case {case:?}: expected the service to run that one task, got {described}"
    );
}

/// RULE: a program that ENDS on a wait of so many seconds keeps its container up for them, so
/// that rollout comes to rest with its task running and NO task of the service stops with
/// `EssentialContainerExited`.
#[tokio::test]
async fn a_rollout_stabilises_on_a_program_that_ends_on_a_wait() {
    let case = "a program that ends on a wait of so many seconds";
    let router = service();
    cluster_with_capacity(&router, CLUSTER, case).await;
    let definition = register(&router, "statedfamily", STATES_A_WAIT, case).await;
    let released = create_service(&router, "statedfamily", &definition, case).await;

    let deployment = poll_until(
        case,
        &format!("the rollout to reach {SUCCESSFUL:?}"),
        || describe_deployment(&router, &released, case),
        |deployment| deployment["status"] == json!(SUCCESSFUL),
    )
    .await;
    assert_eq!(
        deployment["targetServiceRevision"]["runningTaskCount"].as_i64(),
        Some(1),
        "case {case:?}: expected the rollout's revision to hold its one running task, got \
         {deployment}"
    );
    let described = describe_service(&router, "statedfamily", case).await;
    assert_eq!(
        described["runningCount"].as_i64(),
        Some(1),
        "case {case:?}: expected the service to run the task it placed, got {described}"
    );
    let stopped = stopped_tasks(&router, "statedfamily", case).await;
    assert!(
        stopped.is_empty(),
        "case {case:?}: expected no stopped task of the service — its container's program is \
         still waiting — got {stopped:?}"
    );
}
