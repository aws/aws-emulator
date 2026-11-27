// SPDX-License-Identifier: CC0-1.0

//! The container instance a task of a cluster's OWN capacity is placed on: the placement the
//! task records, the capacity that placement is held to, and the address a service registry
//! publishes such a task at.
//!
//! Driven end to end over the shared wire harness in [`crate::ecs_wire`], with the Amazon EC2
//! service and the Cloud Map (service discovery) service mounted beside this one on ONE
//! engine — so the capacity every case below places onto is an instance LAUNCHED through
//! EC2's own API, and every value asserted is read back through `RunTask`, `StartTask`,
//! `DescribeTasks`, `DescribeContainerInstances` and Cloud Map's own `DiscoverInstances`.
//!
//! # The rules under test
//!
//! 1. > A task placed on the capacity registered to a cluster records the container instance
//! >   that hosts it: `Task$containerInstanceArn` is "The ARN of the container instances that
//! >   host the task", present from the moment the placement answers and for the task's whole
//! >   lifetime, in the same long form `ListContainerInstances` reports for that host — and
//! >   absent for a task with no host of its own, which is one on Fargate.
//!
//! 2. > A placement is made only on a container instance whose REMAINING capacity covers what
//! >   the task reserves — `ContainerInstance$remainingResources` is "the remaining CPU and
//! >   memory that wasn't already allocated to tasks" — and a run no instance of the cluster
//! >   has room for places nothing: it answers one `Failure` per candidate host, naming that
//! >   host by its id alone and carrying the structured `RESOURCE:<name>` code of the resource
//! >   that ran out.
//!
//! 3. > A `serviceRegistries` entry publishes each RUNNING task of its service at the address
//! >   the task is REACHED at: a task holding an elastic network interface of its own at that
//! >   interface's address, and a task whose network mode shares its host's network at the
//! >   private IPv4 address of the container instance that hosts it, on the HOST port its
//! >   container was bound to.
//!
//! # What the rows span
//!
//! Rule 1: the placement PATH (`RunTask`, `StartTask` naming the host itself, and the service
//! scheduler), the LAUNCH TYPE (`EC2`, which runs on the cluster's capacity, and `FARGATE`,
//! which runs on none) and the task's LIFECYCLE (a running task and the same task once it has
//! stopped). Rule 2: the resource that runs out (CPU and memory), and the reservation against
//! the host's remainder (less than what is left, exactly what is left, more than what is left,
//! and more than the host ever had). Rule 3: how the bound host port came about (the ephemeral
//! port a dynamic mapping is assigned, and a host port the revision declares).

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    assert_absent, cloud_map, cloud_map_discovered, create_cluster, ec2_element, ec2_form, list,
    ok, poll_until, string, ACCOUNT, SIGNED_REGION,
};

/// The cluster every case below launches its capacity into.
const CLUSTER: &str = "ec2hosted";

/// The AMI every launch boots. Well-formed, which is all a launch needs: which AMI carries
/// the container agent is not a fact this emulator publishes.
const IMAGE_ID: &str = "ami-0123456789abcdef0";

/// The instance type the capacity comes from — the resources a registration reports are the
/// type's, and they are READ BACK rather than written here.
const INSTANCE_TYPE: &str = "t3.micro";

/// The container every revision below runs and the port it publishes.
const CONTAINER: &str = "web";
const CONTAINER_PORT: i64 = 80;
const IMAGE: &str = "public.ecr.aws/nginx/nginx:latest";

/// `Task$lastStatus` of a task that has started, and of one that has stopped.
const RUNNING: &str = "RUNNING";
const STOPPED: &str = "STOPPED";

/// The private DNS namespace the Cloud Map services of rule 3 live in.
const NAMESPACE: &str = "ec2hosted.example.com";

/// A router carrying this service, Amazon EC2 and Cloud Map over ONE shared engine: the three
/// stores these rules read across — the machine the capacity is, the registration it earns and
/// the registry a service publishes its tasks into — mounted the way the production router
/// mounts them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_servicediscovery_route53autonaming_v20170314::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// Every value an awsQuery XML response carries for `tag`, deduplicated — an instance's
/// private address is reported both for the instance and for the interface that holds it, and
/// the two are one address.
fn xml_values(xml: &str, tag: &str) -> BTreeSet<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut found = BTreeSet::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        rest = &rest[start + open.len()..];
        let Some(end) = rest.find(&close) else {
            break;
        };
        found.insert(rest[..end].to_string());
        rest = &rest[end + close.len()..];
    }
    found
}

/// The capacity one launched instance gave the cluster, as the cluster REPORTS it.
struct Capacity {
    /// The Amazon EC2 instance id of the machine.
    instance_id: String,
    /// Its private IPv4 address, read from Amazon EC2's own records.
    private_ipv4: String,
    /// The container-instance ARN `ListContainerInstances` reports for it, in the long form.
    arn: String,
    /// The container-instance id alone: the last segment of [`Capacity::arn`].
    id: String,
    /// The MiB of memory and the CPU units the registration reported.
    memory: i64,
    cpu: i64,
    /// The subnet the machine was launched into, which an `awsvpc` placement names.
    subnet: String,
    /// The VPC that subnet is in, which a private DNS namespace is created against.
    vpc: String,
}

impl Capacity {
    /// The container-instance ARN a FAILURE about this host names it by: the id alone under
    /// the resource type, with no cluster segment.
    fn failure_arn(&self) -> String {
        format!(
            "arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:container-instance/{}",
            self.id
        )
    }
}

/// Create the cluster, one VPC and subnet, and launch ONE instance into the cluster the
/// documented way — "echo ECS_CLUSTER=<cluster> >> /etc/ecs/ecs.config" in its user data —
/// then wait until the cluster reports the capacity its agent registered.
async fn capacity(router: &MultiServiceRouter, case: &str) -> Capacity {
    use ::base64::Engine as _;

    create_cluster(router, CLUSTER, case).await;
    let vpc = ec2_element(
        &ec2_form(router, "CreateVpc", &[("CidrBlock", "10.90.0.0/16")], case).await,
        "vpcId",
        case,
    );
    let subnet = ec2_element(
        &ec2_form(
            router,
            "CreateSubnet",
            &[
                ("VpcId", vpc.as_str()),
                ("CidrBlock", "10.90.0.0/20"),
                ("AvailabilityZone", format!("{SIGNED_REGION}a").as_str()),
            ],
            case,
        )
        .await,
        "subnetId",
        case,
    );
    let user_data = ::base64::engine::general_purpose::STANDARD.encode(format!(
        "#!/bin/bash\necho ECS_CLUSTER={CLUSTER} >> /etc/ecs/ecs.config\n"
    ));
    let instance_id = ec2_element(
        &ec2_form(
            router,
            "RunInstances",
            &[
                ("ImageId", IMAGE_ID),
                ("InstanceType", INSTANCE_TYPE),
                ("SubnetId", subnet.as_str()),
                ("MinCount", "1"),
                ("MaxCount", "1"),
                ("UserData", user_data.as_str()),
            ],
            case,
        )
        .await,
        "instanceId",
        case,
    );
    let described = ec2_form(
        router,
        "DescribeInstances",
        &[("InstanceId.1", instance_id.as_str())],
        case,
    )
    .await;
    let addresses = xml_values(&described, "privateIpAddress");
    assert_eq!(
        addresses.len(),
        1,
        "case {case:?}: expected the launched instance to hold one private address, got \
         {addresses:?}"
    );
    let private_ipv4 = addresses
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: the set holds one address"));

    // The agent on the instance registers it on its own clock, so the cluster is polled for
    // the capacity rather than waited on for a span.
    let listed = poll_until(
        case,
        "the launched instance to register as container-instance capacity",
        || ok(router, "ListContainerInstances", json!({ "cluster": CLUSTER }), case),
        |body| body["containerInstanceArns"].as_array().is_some_and(|arns| arns.len() == 1),
    )
    .await;
    let arn = list(&listed, "containerInstanceArns", case)
        .into_iter()
        .next()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| panic!("case {case:?}: expected one container-instance ARN"));
    let described = ok(
        router,
        "DescribeContainerInstances",
        json!({ "cluster": CLUSTER, "containerInstances": [arn.clone()] }),
        case,
    )
    .await;
    let instance = list(&described, "containerInstances", case)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: expected the registration back, got {described}"));
    assert_eq!(
        instance["agentConnected"].as_bool(),
        Some(true),
        "case {case:?}: expected the registration to report its agent connected, got {instance}"
    );
    let id = arn
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string();
    Capacity {
        instance_id,
        private_ipv4,
        arn,
        id,
        memory: registered(&instance, "MEMORY", case),
        cpu: registered(&instance, "CPU", case),
        subnet,
        vpc,
    }
}

/// The amount a named resource of a registration reports, out of `registeredResources`.
fn registered(instance: &Value, name: &str, case: &str) -> i64 {
    instance["registeredResources"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|resource| resource["name"].as_str() == Some(name))
        .and_then(|resource| resource["integerValue"].as_i64())
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected the registration to report {name}, got {instance}")
        })
}

/// What a container instance reports as LEFT of a named resource, out of
/// `remainingResources`.
async fn remaining(router: &MultiServiceRouter, host: &Capacity, name: &str, case: &str) -> i64 {
    let described = ok(
        router,
        "DescribeContainerInstances",
        json!({ "cluster": CLUSTER, "containerInstances": [host.arn.clone()] }),
        case,
    )
    .await;
    let instance = list(&described, "containerInstances", case)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: expected the registration back, got {described}"));
    instance["remainingResources"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|resource| resource["name"].as_str() == Some(name))
        .and_then(|resource| resource["integerValue"].as_i64())
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected the instance to report {name} left, got {instance}")
        })
}

/// Register one `bridge` revision of `family`, sized as `size` says and publishing
/// [`CONTAINER_PORT`] on `host_port` (`0` asks for a port out of the ephemeral range, which is
/// what omitting it means).
async fn register_bridge_revision(
    router: &MultiServiceRouter,
    family: &str,
    size: Value,
    host_port: i64,
    case: &str,
) {
    let mut request = json!({
        "family": family,
        "networkMode": "bridge",
        "requiresCompatibilities": ["EC2"],
        "containerDefinitions": [{
            "name": CONTAINER,
            "image": IMAGE,
            "memory": 128,
            "portMappings": [{
                "containerPort": CONTAINER_PORT,
                "hostPort": host_port,
                "protocol": "tcp",
            }],
        }],
    });
    for (member, value) in size.as_object().into_iter().flatten() {
        request[member] = value.clone();
    }
    ok(router, "RegisterTaskDefinition", request, case).await;
}

/// Run `count` task(s) of `family` with `launch_type`, answering the whole `RunTask` body.
async fn run_task(
    router: &MultiServiceRouter,
    family: &str,
    launch_type: &str,
    extra: Value,
    case: &str,
) -> Value {
    let mut request = json!({
        "cluster": CLUSTER,
        "taskDefinition": family,
        "count": 1,
        "launchType": launch_type,
    });
    for (member, value) in extra.as_object().into_iter().flatten() {
        request[member] = value.clone();
    }
    ok(router, "RunTask", request, case).await
}

/// The one task `RunTask` placed, out of a body that must carry exactly one and no failure.
fn placed(body: &Value, case: &str) -> Value {
    assert_eq!(
        list(body, "failures", case),
        Vec::<Value>::new(),
        "case {case:?}: expected a placement with no failures, got {body}"
    );
    let tasks = list(body, "tasks", case);
    assert_eq!(
        tasks.len(),
        1,
        "case {case:?}: expected exactly one placed task, got {body}"
    );
    tasks
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: the list holds one task"))
}

/// One task of this cluster, as `DescribeTasks` reports it now.
async fn described_task(router: &MultiServiceRouter, task_arn: &str, case: &str) -> Value {
    let body = ok(
        router,
        "DescribeTasks",
        json!({ "cluster": CLUSTER, "tasks": [task_arn] }),
        case,
    )
    .await;
    list(&body, "tasks", case)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: expected the task back, got {body}"))
}

/// That same task once it reports `status`, looked for until it does: where a task is in its
/// lifecycle is the CLOCK's, so a case about it waits for the status rather than for a span.
async fn task_at_status(
    router: &MultiServiceRouter,
    task_arn: &str,
    status: &str,
    case: &str,
) -> Value {
    poll_until(
        case,
        &format!("the task to reach {status:?}"),
        || described_task(router, task_arn, case),
        |task| task["lastStatus"].as_str() == Some(status),
    )
    .await
}

/// The one task the service `name` is running, once it reports `RUNNING` — read through
/// `ListTasks` and `DescribeTasks`, which is also the call that materialises the Cloud Map
/// registrations the clock has made due.
async fn service_task(router: &MultiServiceRouter, name: &str, case: &str) -> Value {
    poll_until(
        case,
        "the service's task to be running",
        || async {
            let listed = ok(
                router,
                "ListTasks",
                json!({ "cluster": CLUSTER, "serviceName": name }),
                case,
            )
            .await;
            let arns = list(&listed, "taskArns", case);
            if arns.is_empty() {
                return json!({});
            }
            let described = ok(
                router,
                "DescribeTasks",
                json!({ "cluster": CLUSTER, "tasks": arns }),
                case,
            )
            .await;
            list(&described, "tasks", case)
                .into_iter()
                .next()
                .unwrap_or_else(|| json!({}))
        },
        |task| task["lastStatus"].as_str() == Some(RUNNING),
    )
    .await
}

/// The host port the one container of `task` was bound to on its host.
fn bound_host_port(task: &Value, case: &str) -> i64 {
    task["containers"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|container| container["networkBindings"].as_array().into_iter().flatten())
        .find_map(|binding| binding["hostPort"].as_i64())
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected the container to be bound to a host port, got {task}")
        })
}

#[tokio::test]
async fn a_task_of_a_cluster_s_own_capacity_names_the_container_instance_that_hosts_it() {
    let case = "ec2 placement";
    let router = emulator();
    let host = capacity(&router, case).await;
    register_bridge_revision(&router, "hostedbridge", json!({ "memory": "128" }), 0, case).await;

    // 1. `RunTask`'s own 200 body already names the placement.
    let ran = run_task(&router, "hostedbridge", "EC2", json!({}), case).await;
    let task = placed(&ran, case);
    assert_eq!(
        task["containerInstanceArn"].as_str(),
        Some(host.arn.as_str()),
        "case {case:?}: RunTask must name the container instance that hosts the task — the \
         registration the machine {} earned, whose ARN ListContainerInstances reports as {} — \
         got {task}",
        host.instance_id,
        host.arn
    );

    // 2. And so does every later read of that task, for its whole lifetime — including once
    //    it has stopped.
    let task_arn = string(&task, "taskArn", case);
    let running = task_at_status(&router, &task_arn, RUNNING, case).await;
    assert_eq!(
        running["containerInstanceArn"].as_str(),
        Some(host.arn.as_str()),
        "case {case:?}: DescribeTasks must report the same host for the running task, got \
         {running}"
    );
    ok(
        &router,
        "StopTask",
        json!({ "cluster": CLUSTER, "task": task_arn, "reason": "making room" }),
        case,
    )
    .await;
    let stopped = task_at_status(&router, &task_arn, STOPPED, case).await;
    assert_eq!(
        stopped["containerInstanceArn"].as_str(),
        Some(host.arn.as_str()),
        "case {case:?}: a stopped task keeps the host it ran on, got {stopped}"
    );

    // 3. `StartTask`, the placement whose caller NAMES the host, records the same thing.
    let started = ok(
        &router,
        "StartTask",
        json!({
            "cluster": CLUSTER,
            "taskDefinition": "hostedbridge",
            "containerInstances": [host.id.clone()],
        }),
        case,
    )
    .await;
    let started = placed(&started, case);
    assert_eq!(
        started["containerInstanceArn"].as_str(),
        Some(host.arn.as_str()),
        "case {case:?}: StartTask must record the instance the caller named, got {started}"
    );
    ok(
        &router,
        "StopTask",
        json!({ "cluster": CLUSTER, "task": string(&started, "taskArn", case) }),
        case,
    )
    .await;

    // 4. The SERVICE scheduler's own placement records it too.
    ok(
        &router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": "hostedsvc",
            "taskDefinition": "hostedbridge",
            "launchType": "EC2",
            "desiredCount": 1,
        }),
        case,
    )
    .await;
    let scheduled = service_task(&router, "hostedsvc", case).await;
    assert_eq!(
        scheduled["containerInstanceArn"].as_str(),
        Some(host.arn.as_str()),
        "case {case:?}: a service's task must name the container instance the scheduler chose, \
         got {scheduled}"
    );

    // 5. And a task with no host of its own reports none: a Fargate placement runs on
    //    capacity AWS owns, which no registration of this cluster reports.
    ok(
        &router,
        "RegisterTaskDefinition",
        json!({
            "family": "hostedfargate",
            "networkMode": "awsvpc",
            "requiresCompatibilities": ["FARGATE"],
            "cpu": "256",
            "memory": "512",
            "containerDefinitions": [{ "name": CONTAINER, "image": IMAGE }],
        }),
        case,
    )
    .await;
    let fargate = run_task(
        &router,
        "hostedfargate",
        "FARGATE",
        json!({
            "networkConfiguration": {
                "awsvpcConfiguration": { "subnets": [host.subnet.clone()] },
            },
        }),
        case,
    )
    .await;
    let fargate = placed(&fargate, case);
    assert_absent(&fargate, "containerInstanceArn", case);
    assert_eq!(
        fargate["launchType"].as_str(),
        Some("FARGATE"),
        "case {case:?}: the Fargate placement is the one with no host, got {fargate}"
    );
}

/// How much memory one row's revision states, against what the host has LEFT when that row
/// runs — the rows are run in order against one host, so each is expressed over the remainder
/// the rows before it left rather than over a figure written here.
#[derive(Debug, Clone, Copy)]
enum Wanted {
    /// All of the remainder but this many MiB (and at least one MiB, for a host with nothing
    /// left at all).
    LeftLess(i64),
    /// Exactly the remainder.
    AllOfIt,
    /// This many MiB more than the remainder.
    MoreThanLeft(i64),
    /// More than half of the remainder, so two tasks of this size do not both fit on it.
    MoreThanHalf,
}

impl Wanted {
    /// The MiB a revision of this row states, given the `left` the host still has.
    fn mib(self, left: i64) -> i64 {
        match self {
            Self::LeftLess(less) => (left - less).max(1),
            Self::AllOfIt => left.max(1),
            Self::MoreThanLeft(more) => left + more,
            Self::MoreThanHalf => left / 2 + 1,
        }
    }
}

/// One row of the capacity table: what the run asks for, how much of it is placed, and the
/// resource each task it could not place ran out of.
struct Reservation {
    case: &'static str,
    family: &'static str,
    /// The task-level `cpu` the revision states, or `None` for one that states none.
    cpu: Option<i64>,
    /// The task-level `memory` the revision states.
    memory: Wanted,
    /// `RunTask$count`: "The number of instantiations of the specified task to place".
    count: i64,
    /// How many of them the host has room for, and so how many the run places.
    places: i64,
    /// `Failure$reason` each task it could NOT place earns, or `None` for a run that placed
    /// every task it asked for.
    refused_with: Option<&'static str>,
}

#[tokio::test]
async fn a_run_is_placed_only_on_capacity_the_cluster_has_left() {
    let case = "capacity accounting";
    let router = emulator();
    let host = capacity(&router, case).await;

    // Nothing is placed yet, so the whole registration is still available.
    let left = remaining(&router, &host, "MEMORY", case).await;
    assert_eq!(
        left, host.memory,
        "case {case:?}: an instance running nothing has its whole registration left, expected \
         {} MiB, got {left}",
        host.memory
    );

    // The rows walk the host's memory DOWN, so each reservation is weighed against what the
    // rows before it left. The CPU row comes first, while the memory is still free: the
    // memory is answered first for a run short of both.
    let rows = [
        Reservation {
            case: "more CPU units than the host ever registered",
            family: "capcpu",
            cpu: Some(host.cpu + 1024),
            memory: Wanted::LeftLess(128),
            count: 1,
            places: 0,
            refused_with: Some("RESOURCE:CPU"),
        },
        Reservation {
            case: "two tasks against a host with room for one",
            family: "caphalf",
            cpu: None,
            memory: Wanted::MoreThanHalf,
            count: 2,
            places: 1,
            refused_with: Some("RESOURCE:MEMORY"),
        },
        Reservation {
            case: "less memory than the host has left",
            family: "capfits",
            cpu: None,
            memory: Wanted::LeftLess(128),
            count: 1,
            places: 1,
            refused_with: None,
        },
        Reservation {
            case: "more memory than the host has left",
            family: "capover",
            cpu: None,
            memory: Wanted::MoreThanLeft(1),
            count: 1,
            places: 0,
            refused_with: Some("RESOURCE:MEMORY"),
        },
        Reservation {
            case: "exactly the memory the host has left",
            family: "capexact",
            cpu: None,
            memory: Wanted::AllOfIt,
            count: 1,
            places: 1,
            refused_with: None,
        },
        Reservation {
            case: "a single MiB with nothing left at all",
            family: "capnone",
            cpu: None,
            memory: Wanted::AllOfIt,
            count: 1,
            places: 0,
            refused_with: Some("RESOURCE:MEMORY"),
        },
    ];

    let mut reserved = 0_i64;
    for row in &rows {
        let case = row.case;
        let wanted = row.memory.mib(host.memory - reserved);
        let mut size = json!({ "memory": wanted.to_string() });
        if let Some(cpu) = row.cpu {
            size["cpu"] = json!(cpu.to_string());
        }
        register_bridge_revision(&router, row.family, size, 0, case).await;
        let answered = run_task(
            &router,
            row.family,
            "EC2",
            json!({ "count": row.count }),
            case,
        )
        .await;

        // The tasks the run placed: each on the host that had room for it.
        let tasks = list(&answered, "tasks", case);
        assert_eq!(
            tasks.len() as i64,
            row.places,
            "case {case:?}: expected {} of {} task(s) to be placed, got {answered}",
            row.places,
            row.count
        );
        for task in &tasks {
            assert_eq!(
                task["containerInstanceArn"].as_str(),
                Some(host.arn.as_str()),
                "case {case:?}: a task the host had room for is placed on it, got {task}"
            );
        }
        reserved += wanted * row.places;
        let now_left = remaining(&router, &host, "MEMORY", case).await;
        assert_eq!(
            now_left,
            host.memory - reserved,
            "case {case:?}: the instance must report the memory its tasks have not taken, \
             expected {} MiB, got {now_left}",
            host.memory - reserved
        );

        // And one failure per task it could NOT place, naming the host that had no room and
        // the resource that ran out there.
        let failures = list(&answered, "failures", case);
        assert_eq!(
            failures.len() as i64,
            row.count - row.places,
            "case {case:?}: expected one failure per unplaced task, got {answered}"
        );
        for failure in &failures {
            assert_eq!(
                failure["reason"].as_str(),
                row.refused_with,
                "case {case:?}: expected the structured code of the resource that ran out, \
                 got {failure}"
            );
            assert_eq!(
                failure["arn"].as_str(),
                Some(host.failure_arn().as_str()),
                "case {case:?}: a failure names the host by its id alone, expected {}, got \
                 {failure}",
                host.failure_arn()
            );
        }
    }
}

/// A Cloud Map service in [`NAMESPACE`] publishing an `SRV` record — the only record type a
/// task reached through its host's network can be registered in — with the custom health
/// checking a registrar supplies the initial status of, and its ARN.
async fn srv_registry(router: &MultiServiceRouter, name: &str, vpc: &str, case: &str) -> String {
    let created = cloud_map(
        router,
        "CreatePrivateDnsNamespace",
        json!({ "Name": NAMESPACE, "Vpc": vpc }),
        case,
    )
    .await;
    let filed = cloud_map(
        router,
        "GetOperation",
        json!({ "OperationId": string(&created, "OperationId", case) }),
        case,
    )
    .await;
    let namespace_id = string(&filed["Operation"]["Targets"], "NAMESPACE", case);
    let service = cloud_map(
        router,
        "CreateService",
        json!({
            "Name": name,
            "NamespaceId": namespace_id,
            "DnsConfig": { "DnsRecords": [{ "Type": "SRV", "TTL": 60 }] },
            "HealthCheckCustomConfig": { "FailureThreshold": 1 },
        }),
        case,
    )
    .await;
    string(&service["Service"], "Arn", case)
}

#[tokio::test]
async fn a_registry_publishes_a_host_networked_task_at_its_host_s_address_and_port() {
    let case = "registry over the host network";
    let router = emulator();
    let host = capacity(&router, case).await;

    // The rows differ along how the bound host port came about: one the ephemeral range
    // assigned to a dynamic mapping, and one the revision declares.
    let rows: [(&str, &str, &str, i64); 2] = [
        (
            "a dynamic host port out of the ephemeral range",
            "regdynamic",
            "regdynamicsvc",
            0,
        ),
        (
            "a host port the revision declares",
            "regstatic",
            "regstaticsvc",
            8080,
        ),
    ];
    let registry_arn = srv_registry(&router, "hostednetwork", &host.vpc, case).await;

    for (row, family, service_name, host_port) in rows {
        register_bridge_revision(&router, family, json!({ "memory": "128" }), host_port, row).await;
        ok(
            &router,
            "CreateService",
            json!({
                "cluster": CLUSTER,
                "serviceName": service_name,
                "taskDefinition": family,
                "launchType": "EC2",
                "desiredCount": 1,
                "serviceRegistries": [{
                    "registryArn": registry_arn.clone(),
                    "containerName": CONTAINER,
                    "containerPort": CONTAINER_PORT,
                }],
            }),
            row,
        )
        .await;
        // The task's own read is what shows WHERE it is reached: its host, and the host port
        // its container was bound to. That read is also the call that materialises the
        // registrations the clock has made due.
        let task = service_task(&router, service_name, row).await;
        assert_eq!(
            task["containerInstanceArn"].as_str(),
            Some(host.arn.as_str()),
            "case {row:?}: the task runs on the cluster's capacity, got {task}"
        );
        let bound = bound_host_port(&task, row);
        if host_port > 0 {
            assert_eq!(
                bound, host_port,
                "case {row:?}: a declared host port is the port bound, got {task}"
            );
        } else {
            assert!(
                bound >= 49153,
                "case {row:?}: a dynamic mapping is bound to a port of the ephemeral range, \
                 got {bound}"
            );
        }

        // And Cloud Map holds exactly that: one instance per running task, at the HOST's
        // address and the port the task answers on there.
        let instances = cloud_map_discovered(&router, NAMESPACE, "hostednetwork", row).await;
        let registered: Vec<&Value> = instances
            .iter()
            .filter(|instance| {
                instance["Attributes"]["ECS_SERVICE_NAME"].as_str() == Some(service_name)
            })
            .collect();
        assert_eq!(
            registered.len(),
            1,
            "case {row:?}: expected one discoverable instance for {service_name}, got \
             {instances:?}"
        );
        let attributes = &registered[0]["Attributes"];
        assert_eq!(
            attributes["AWS_INSTANCE_IPV4"].as_str(),
            Some(host.private_ipv4.as_str()),
            "case {row:?}: a task that shares its host's network is registered at that host's \
             private address ({}), got {attributes}",
            host.private_ipv4
        );
        assert_eq!(
            attributes["AWS_INSTANCE_PORT"].as_str(),
            Some(bound.to_string().as_str()),
            "case {row:?}: it is registered on the HOST port its container was bound to \
             ({bound}), got {attributes}"
        );
        assert_ne!(
            attributes["AWS_INSTANCE_PORT"].as_str(),
            Some(CONTAINER_PORT.to_string().as_str()),
            "case {row:?}: the container port is not the port such a task answers on, got \
             {attributes}"
        );
        // And the attributes Amazon ECS creates beside the endpoint say WHERE the task runs:
        // the cluster, the service and the family it belongs to, the Region and zone it runs
        // in, and the Amazon EC2 instance of the host it shares.
        for (attribute, expected) in [
            ("ECS_CLUSTER_NAME", CLUSTER.to_string()),
            ("ECS_SERVICE_NAME", service_name.to_string()),
            ("ECS_TASK_DEFINITION_FAMILY", family.to_string()),
            ("REGION", SIGNED_REGION.to_string()),
            ("AVAILABILITY_ZONE", format!("{SIGNED_REGION}a")),
            ("EC2_INSTANCE_ID", host.instance_id.clone()),
            // The initial status the registration supplied, for a registry whose health
            // checking is the custom kind.
            ("AWS_INIT_HEALTH_STATUS", "HEALTHY".to_string()),
        ] {
            assert_eq!(
                attributes[attribute].as_str(),
                Some(expected.as_str()),
                "case {row:?}: expected {attribute} {expected:?}, got {attributes}"
            );
        }
    }
}
