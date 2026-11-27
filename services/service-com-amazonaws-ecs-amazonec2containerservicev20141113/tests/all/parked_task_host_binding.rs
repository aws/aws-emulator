// SPDX-License-Identifier: CC0-1.0

//! WHICH container instance hosts a task that was PARKED until its capacity provider acquired
//! the capacity to place it on, driven end to end over the shared wire harness in
//! [`crate::ecs_wire`] with Amazon EC2 and Amazon EC2 Auto Scaling mounted beside this service
//! on one engine — so every host is an instance the account's own Auto Scaling group launched
//! and every value asserted is read back through `ListContainerInstances`,
//! `DescribeContainerInstances` and `DescribeTasks`.
//!
//! # The rule under test
//!
//! > A task whose infrastructure is the capacity registered to its cluster (the `EC2` and
//! > `EXTERNAL` launch types, which is what a capacity provider over a caller's Auto Scaling
//! > group places onto) is bound to ONE of that cluster's container instances the moment it
//! > leaves `PROVISIONING`, and reports it in `Task$containerInstanceArn` from then on. A task
//! > no registered instance has room for is not placed: it stays accepted-but-not-placed in
//! > `PROVISIONING` with no host, rather than reaching `PENDING` or `RUNNING` on no instance at
//! > all.
//!
//! `Task$containerInstanceArn` is "The ARN of the container instance that hosts the task" and
//! `Task$launchType` "The infrastructure where your task runs on"; "the EC2 launch type runs
//! your tasks on Amazon EC2 instances registered to your cluster", and that capacity is what an
//! Auto Scaling group capacity provider brings ("When managed scaling is turned on, Amazon ECS
//! manages the scale-in and scale-out actions of the Auto Scaling group"). The host is the
//! scheduler's choice among the instances that accept a placement and have the resources the
//! task reserves — `ContainerInstance$remainingResources` is "the remaining CPU and memory that
//! wasn't already allocated to tasks and is therefore available for new tasks" — so a task the
//! arriving capacity cannot take has no host to report and is not placed.
//!
//! # What the rows span
//!
//! The table differs along what the rule generalises over: how many tasks one run parks (one,
//! two, three), where the size each reserves is stated (the task level, the containers'), the
//! provider's own scaling step bounds (stated as the recorded shape states them, and left at
//! their defaults), how much capacity the group may hold (one instance, two) and therefore how
//! many instances the placements are spread over.
//!
//! The second rule, over the same estate
//! ([`a_parked_task_the_arriving_capacity_cannot_take_stays_provisioning_with_no_host`]): the
//! surplus of a run the arriving capacity has room for only part of stays `PROVISIONING` with
//! no host for as long as no instance can take it, while its siblings run on the host that did.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    auto_scaling, count, create_cluster, ec2_form, emulator_with_ec2, list, ok, poll_until, string,
    POLL_INTERVAL, SIGNED_REGION,
};

/// The AMI every group launches from. Well-formed (`ami-<17 hexadecimal digits>`), which is all
/// a launch needs; which AMI carries the container agent is not a fact this emulator publishes.
const IMAGE_ID: &str = "ami-0123456789abcdef0";

/// The instance type every host is launched as: two vCPUs (2048 CPU units) and 1024 MiB of
/// memory, as Amazon EC2 publishes them for it.
const INSTANCE_TYPE: &str = "t3.micro";

/// The memory, in MiB, the instance type above offers a cluster — the figure the rows size
/// their tasks against.
const HOST_MEMORY_MIB: i64 = 1_024;

/// `ContainerInstance$status` of a registered host.
const ACTIVE: &str = "ACTIVE";

/// The `Task$lastStatus` values of this rule: a task Amazon ECS has accepted but not placed,
/// one it is bringing up, and one that is up.
const PROVISIONING: &str = "PROVISIONING";
const PENDING: &str = "PENDING";
const RUNNING: &str = "RUNNING";

/// The launch type a provider over a caller's Auto Scaling group places tasks under.
const LAUNCH_TYPE_EC2: &str = "EC2";

/// How many looks a case that must see NOTHING move takes. Together with
/// [`crate::ecs_wire::POLL_INTERVAL`] a window far longer than the spans a placement, a
/// scaling action and an agent registration each take, so a task still parked at the end of it
/// is one that stayed rather than one read too early.
const QUIET_LOOKS: usize = 80;

/// The user data a host is launched with: the documented shell line that appends the container
/// agent's `ECS_CLUSTER` assignment to its configuration file, base64 as a launch carries it.
fn launched_into(cluster: &str) -> String {
    use base64::Engine as _;

    base64::engine::general_purpose::STANDARD.encode(format!(
        "#!/bin/bash\necho ECS_CLUSTER={cluster} >> /etc/ecs/ecs.config\n"
    ))
}

/// Create the launch template and the Auto Scaling group one case scales, through each
/// service's own API: no capacity to begin with, and a maximum of `max_size` instances.
async fn create_group(
    router: &MultiServiceRouter,
    group: &str,
    cluster: &str,
    max_size: i32,
    case: &str,
) {
    let template = format!("{group}-lt");
    let user_data = launched_into(cluster);
    ec2_form(
        router,
        "CreateLaunchTemplate",
        &[
            ("LaunchTemplateName", template.as_str()),
            ("LaunchTemplateData.ImageId", IMAGE_ID),
            ("LaunchTemplateData.InstanceType", INSTANCE_TYPE),
            ("LaunchTemplateData.UserData", user_data.as_str()),
        ],
        case,
    )
    .await;
    let zone = format!("{SIGNED_REGION}a");
    let max_size = max_size.to_string();
    auto_scaling(
        router,
        "CreateAutoScalingGroup",
        &[
            ("AutoScalingGroupName", group),
            ("LaunchTemplate.LaunchTemplateName", template.as_str()),
            ("MinSize", "0"),
            ("MaxSize", max_size.as_str()),
            ("DesiredCapacity", "0"),
            ("AvailabilityZones.member.1", zone.as_str()),
        ],
        case,
    )
    .await;
}

/// Create the capacity provider over `group` with managed scaling ON — the capacity Amazon ECS
/// acquires itself — and associate it with `cluster`.
async fn create_provider(
    router: &MultiServiceRouter,
    provider: &str,
    group: &str,
    cluster: &str,
    steps: Option<(i32, i32)>,
    case: &str,
) {
    let mut scaling = json!({ "status": "ENABLED", "targetCapacity": 100 });
    if let Some((minimum, maximum)) = steps {
        scaling["minimumScalingStepSize"] = json!(minimum);
        scaling["maximumScalingStepSize"] = json!(maximum);
    }
    ok(
        router,
        "CreateCapacityProvider",
        json!({
            "name": provider,
            "autoScalingGroupProvider": {
                "autoScalingGroupArn": group,
                "managedScaling": scaling,
            },
        }),
        case,
    )
    .await;
    ok(
        router,
        "PutClusterCapacityProviders",
        json!({
            "cluster": cluster,
            "capacityProviders": [provider],
            "defaultCapacityProviderStrategy": [],
        }),
        case,
    )
    .await;
}

/// Register one revision the `EC2` launch type runs: the host's own networking (`bridge`) and
/// one essential container whose program keeps performing, sized either at the task level or on
/// the container itself.
async fn register_definition(
    router: &MultiServiceRouter,
    family: &str,
    task_size: Option<(&str, &str)>,
    container_memory: i32,
    case: &str,
) -> String {
    let mut request = json!({
        "family": family,
        "networkMode": "bridge",
        "requiresCompatibilities": ["EC2"],
        "containerDefinitions": [{
            "name": "app",
            "image": "public.ecr.aws/docker/library/busybox:1.36",
            "essential": true,
            "cpu": 64,
            "memory": container_memory,
            "command": ["sh", "-c", "sleep infinity"],
        }],
    });
    if let Some((cpu, memory)) = task_size {
        request["cpu"] = json!(cpu);
        request["memory"] = json!(memory);
    }
    let body = ok(router, "RegisterTaskDefinition", request, case).await;
    string(&body["taskDefinition"], "taskDefinitionArn", case)
}

/// The `ACTIVE` container instances of `cluster`, as `ListContainerInstances` reports them —
/// the read the rule is stated against ("it equals one of the ARNs
/// `ListContainerInstances(status=ACTIVE)` returns for that cluster").
async fn active_hosts(router: &MultiServiceRouter, cluster: &str, case: &str) -> BTreeSet<String> {
    let listing = ok(
        router,
        "ListContainerInstances",
        json!({ "cluster": cluster, "status": ACTIVE }),
        case,
    )
    .await;
    list(&listing, "containerInstanceArns", case)
        .iter()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect()
}

/// Those hosts once `wanted` of them have registered: the registration is the agent's own and
/// owed by the clock, so the suite waits for the CONDITION rather than for a span.
async fn settled_hosts(
    router: &MultiServiceRouter,
    cluster: &str,
    wanted: usize,
    case: &str,
) -> BTreeSet<String> {
    let reading = poll_until(
        case,
        &format!("{wanted} ACTIVE container instance(s) to register"),
        || async {
            json!(active_hosts(router, cluster, case)
                .await
                .into_iter()
                .collect::<Vec<String>>())
        },
        |hosts| hosts.as_array().is_some_and(|hosts| hosts.len() >= wanted),
    )
    .await;
    reading
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect()
}

/// The tasks `arns` names, as `DescribeTasks` reports them.
async fn described_tasks(
    router: &MultiServiceRouter,
    cluster: &str,
    arns: &[String],
    case: &str,
) -> Vec<Value> {
    let body = ok(
        router,
        "DescribeTasks",
        json!({ "cluster": cluster, "tasks": arns }),
        case,
    )
    .await;
    let tasks = list(&body, "tasks", case);
    assert_eq!(
        tasks.len(),
        arns.len(),
        "case {case:?}: expected every task of the run back, got {body}"
    );
    tasks
}

/// The invariant this suite holds at EVERY look, whatever the clock has got to: a task of this
/// infrastructure that has left `PROVISIONING` names the container instance that hosts it.
fn assert_placed_tasks_name_their_host(tasks: &[Value], case: &str) {
    for task in tasks {
        let status = string(task, "lastStatus", case);
        if status != PENDING && status != RUNNING {
            continue;
        }
        let host = task["containerInstanceArn"].as_str();
        assert!(
            host.is_some(),
            "case {case:?}: expected the task reported {status} to name the container instance \
             that hosts it, got {task}"
        );
    }
}

/// The tasks of the run once every one of them is `RUNNING`, with the invariant above held at
/// every look: the start is owed by the clock, so the suite waits for the status.
async fn running_tasks(
    router: &MultiServiceRouter,
    cluster: &str,
    arns: &[String],
    case: &str,
) -> Vec<Value> {
    let reading = poll_until(
        case,
        "every task of the run to reach RUNNING on the capacity that arrived",
        || async {
            let tasks = described_tasks(router, cluster, arns, case).await;
            assert_placed_tasks_name_their_host(&tasks, case);
            json!(tasks)
        },
        |tasks| {
            tasks
                .as_array()
                .is_some_and(|tasks| tasks.iter().all(|task| task["lastStatus"] == json!(RUNNING)))
        },
    )
    .await;
    reading.as_array().cloned().unwrap_or_default()
}

/// How many tasks each host of `cluster` counts: `runningTasksCount` plus `pendingTasksCount`,
/// read back through `DescribeContainerInstances`, so what the task says and what the instance
/// says can be compared.
async fn tasks_counted_by_hosts(
    router: &MultiServiceRouter,
    cluster: &str,
    hosts: &BTreeSet<String>,
    case: &str,
) -> i64 {
    let arns: Vec<String> = hosts.iter().cloned().collect();
    let body = ok(
        router,
        "DescribeContainerInstances",
        json!({ "cluster": cluster, "containerInstances": arns }),
        case,
    )
    .await;
    list(&body, "containerInstances", case)
        .iter()
        .map(|instance| {
            count(instance, "runningTasksCount", case) + count(instance, "pendingTasksCount", case)
        })
        .sum()
}

/// Run `count` tasks of `definition` through `provider` and answer their ARNs, asserting the
/// answer itself: a run whose capacity does not exist yet is accepted in `PROVISIONING` with no
/// host, because there is none to name.
async fn run_parked_tasks(
    router: &MultiServiceRouter,
    cluster: &str,
    definition: &str,
    provider: &str,
    count: i64,
    case: &str,
) -> Vec<String> {
    let body = ok(
        router,
        "RunTask",
        json!({
            "cluster": cluster,
            "taskDefinition": definition,
            "count": count,
            "capacityProviderStrategy": [{ "capacityProvider": provider, "weight": 1 }],
        }),
        case,
    )
    .await;
    let placed = list(&body, "tasks", case);
    assert_eq!(
        placed.len() as i64,
        count,
        "case {case:?}: expected the run to be accepted while its capacity is acquired, got \
         {body}"
    );
    for task in &placed {
        assert_eq!(
            string(task, "lastStatus", case),
            PROVISIONING,
            "case {case:?}: expected a task whose capacity is still being acquired to be \
             PROVISIONING, got {task}"
        );
        assert_eq!(
            task["containerInstanceArn"].as_str(),
            None,
            "case {case:?}: expected a task with no capacity yet to name no host, got {task}"
        );
    }
    placed
        .iter()
        .map(|task| string(task, "taskArn", case))
        .collect()
}

/// One row: how big each task of the run is, how many the run asks for, what the provider's
/// step bounds are, how much capacity its group may hold, and how many instances the placement
/// therefore has to be spread over.
struct Row {
    case: &'static str,
    /// `cpu`/`memory` at the TASK level, where the row states them there.
    task_size: Option<(&'static str, &'static str)>,
    /// The `memory` its one container declares, which a row stating no task-level size
    /// reserves instead.
    container_memory: i32,
    /// How many tasks the run asks for.
    count: i64,
    /// `minimumScalingStepSize`/`maximumScalingStepSize`, where the row states them.
    steps: Option<(i32, i32)>,
    /// The group's own `MaxSize`.
    max_size: i32,
    /// How many container instances must end up registered and hosting the run.
    expected_hosts: usize,
}

const ROWS: &[Row] = &[
    Row {
        // The recorded shape: one task sized at the task level, placed through a provider whose
        // scaling steps are both 1.
        case: "one task, sized at the task level, on one instance",
        task_size: Some(("256", "256")),
        container_memory: 64,
        count: 1,
        steps: Some((1, 1)),
        max_size: 2,
        expected_hosts: 1,
    },
    Row {
        case: "two tasks sized on their container, over the two instances the shortfall asks for",
        task_size: None,
        container_memory: (HOST_MEMORY_MIB / 2) as i32,
        count: 2,
        steps: None,
        max_size: 2,
        expected_hosts: 2,
    },
    Row {
        case: "three tasks that all fit on the one instance the group may hold",
        task_size: None,
        container_memory: 128,
        count: 3,
        steps: Some((1, 1)),
        max_size: 1,
        expected_hosts: 1,
    },
];

#[tokio::test]
async fn a_task_placed_by_the_capacity_its_provider_acquires_names_the_host_it_runs_on() {
    let router = emulator_with_ec2();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        // Each row gets its own cluster, group and provider, so the capacity one row acquires
        // is never the capacity another row's placement finds.
        let cluster = format!("parkedhosts{index}");
        let group = format!("parked-hosts-asg-{index}");
        let provider = format!("parkedhostsprovider{index}");
        let family = format!("parkedhostsfamily{index}");

        create_cluster(&router, &cluster, case).await;
        create_group(&router, &group, &cluster, row.max_size, case).await;
        create_provider(&router, &provider, &group, &cluster, row.steps, case).await;
        let definition =
            register_definition(&router, &family, row.task_size, row.container_memory, case).await;
        assert!(
            active_hosts(&router, &cluster, case).await.is_empty(),
            "case {case:?}: expected the cluster to hold no capacity before the run"
        );

        let arns =
            run_parked_tasks(&router, &cluster, &definition, &provider, row.count, case).await;

        // The capacity arrives on its own — Amazon ECS raises the group, the instances register
        // — and is read the way a client reads it: the ARNs `ListContainerInstances` answers.
        let hosts = settled_hosts(&router, &cluster, row.expected_hosts, case).await;
        assert_eq!(
            hosts.len(),
            row.expected_hosts,
            "case {case:?}: expected the capacity Amazon ECS acquired to be {} container \
             instance(s) of the cluster, got {hosts:?}",
            row.expected_hosts
        );

        let tasks = running_tasks(&router, &cluster, &arns, case).await;
        let mut taken: BTreeSet<String> = BTreeSet::new();
        for task in &tasks {
            assert_eq!(
                string(task, "launchType", case),
                LAUNCH_TYPE_EC2,
                "case {case:?}: expected the task to report the infrastructure its provider \
                 binds onto, got {task}"
            );
            let host = task["containerInstanceArn"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: expected the running task to name the container instance \
                         that hosts it, got {task}"
                    )
                });
            assert!(
                hosts.contains(&host),
                "case {case:?}: expected the host to be one of the cluster's ACTIVE container \
                 instances {hosts:?}, got {host:?}"
            );
            taken.insert(host);
        }
        assert_eq!(
            taken.len(),
            row.expected_hosts,
            "case {case:?}: expected the run's tasks to be spread over {} host(s), got {taken:?}",
            row.expected_hosts
        );
        // And the hosts say the same thing back, which is what makes the binding the cluster's
        // own rather than a value on the task alone.
        let counted = tasks_counted_by_hosts(&router, &cluster, &hosts, case).await;
        assert_eq!(
            counted, row.count,
            "case {case:?}: expected the cluster's hosts to count the {} task(s) placed on them, \
             got {counted}",
            row.count
        );
    }
}

/// The cluster, group, provider and family of the case below.
const SURPLUS_CLUSTER: &str = "parkedsurplus";
const SURPLUS_GROUP: &str = "parked-surplus-asg";
const SURPLUS_PROVIDER: &str = "parkedsurplusprovider";
const SURPLUS_FAMILY: &str = "parkedsurplusfamily";

/// How many tasks that case runs, and the memory each reserves: two fit on the one instance the
/// group may hold ([`HOST_MEMORY_MIB`]), the third does not.
const SURPLUS_COUNT: i64 = 3;
const SURPLUS_MEMORY_MIB: i32 = 400;

/// The other half of the rule: the surplus of a run the arriving capacity has room for only
/// part of is NOT placed. "The EC2 launch type runs your tasks on Amazon EC2 instances
/// registered to your cluster", and `ContainerInstance$remainingResources` is what is left for
/// new tasks, so a task the single instance cannot take stays accepted-but-not-placed —
/// `PROVISIONING` with no host — rather than reaching `PENDING` or `RUNNING` on no instance.
#[tokio::test]
async fn a_parked_task_the_arriving_capacity_cannot_take_stays_provisioning_with_no_host() {
    let case = "a parked task no arriving instance has room for";
    let router = emulator_with_ec2();

    create_cluster(&router, SURPLUS_CLUSTER, case).await;
    // One instance at most: the capacity that arrives is all the capacity this cluster can ever
    // hold, so the task it has no room for has nowhere to go.
    create_group(&router, SURPLUS_GROUP, SURPLUS_CLUSTER, 1, case).await;
    create_provider(
        &router,
        SURPLUS_PROVIDER,
        SURPLUS_GROUP,
        SURPLUS_CLUSTER,
        Some((1, 1)),
        case,
    )
    .await;
    let definition = register_definition(
        &router,
        SURPLUS_FAMILY,
        None,
        SURPLUS_MEMORY_MIB,
        case,
    )
    .await;

    let arns = run_parked_tasks(
        &router,
        SURPLUS_CLUSTER,
        &definition,
        SURPLUS_PROVIDER,
        SURPLUS_COUNT,
        case,
    )
    .await;
    let hosts = settled_hosts(&router, SURPLUS_CLUSTER, 1, case).await;

    // How many of the run the one instance has room for, from the figures the rule is decided
    // on: the memory it registered and the memory each task reserves.
    let admitted = usize::try_from(HOST_MEMORY_MIB / i64::from(SURPLUS_MEMORY_MIB)).unwrap_or(0);
    assert!(
        admitted > 0 && admitted < usize::try_from(SURPLUS_COUNT).unwrap_or(0),
        "case {case:?}: expected the host to have room for some but not all of the run, got \
         {admitted}"
    );

    // The tasks the capacity took are running on it; the surplus is still waiting. Looked at
    // again until that holds, with the invariant held at every look.
    let settled = poll_until(
        case,
        &format!("{admitted} of the run to run on the host that arrived"),
        || async {
            let tasks = described_tasks(&router, SURPLUS_CLUSTER, &arns, case).await;
            assert_placed_tasks_name_their_host(&tasks, case);
            json!(tasks)
        },
        |tasks| {
            tasks.as_array().is_some_and(|tasks| {
                tasks
                    .iter()
                    .filter(|task| task["lastStatus"] == json!(RUNNING))
                    .count()
                    == admitted
            })
        },
    )
    .await;
    let settled = settled.as_array().cloned().unwrap_or_default();
    for task in &settled {
        let status = string(task, "lastStatus", case);
        let host = task["containerInstanceArn"].as_str().map(str::to_string);
        match status.as_str() {
            RUNNING => {
                let host = host.unwrap_or_else(|| {
                    panic!("case {case:?}: expected the running task to name its host, got {task}")
                });
                assert!(
                    hosts.contains(&host),
                    "case {case:?}: expected the host to be one of the cluster's ACTIVE container \
                     instances {hosts:?}, got {host:?}"
                );
            }
            PROVISIONING => assert_eq!(
                host, None,
                "case {case:?}: expected the task nothing has placed to name no host, got {task}"
            ),
            other => panic!(
                "case {case:?}: expected every task to be RUNNING on a host or still \
                 PROVISIONING, got {other} in {task}"
            ),
        }
    }

    // And the surplus STAYS where it is: nothing places a task the cluster has no room for, so
    // a window far longer than the spans a placement, a scaling action and a registration take
    // finds it still waiting, still naming no host.
    for _ in 0..QUIET_LOOKS {
        tokio::time::sleep(POLL_INTERVAL).await;
        let tasks = described_tasks(&router, SURPLUS_CLUSTER, &arns, case).await;
        assert_placed_tasks_name_their_host(&tasks, case);
        let waiting = tasks
            .iter()
            .filter(|task| task["lastStatus"] == json!(PROVISIONING))
            .count();
        assert_eq!(
            waiting,
            usize::try_from(SURPLUS_COUNT).unwrap_or(0) - admitted,
            "case {case:?}: expected the task the capacity cannot take to stay PROVISIONING, got \
             {tasks:?}"
        );
    }
    // The host counts only the tasks it really hosts.
    let counted = tasks_counted_by_hosts(&router, SURPLUS_CLUSTER, &hosts, case).await;
    assert_eq!(
        counted,
        i64::try_from(admitted).unwrap_or(0),
        "case {case:?}: expected the host to count only the tasks placed on it, got {counted}"
    );
}
