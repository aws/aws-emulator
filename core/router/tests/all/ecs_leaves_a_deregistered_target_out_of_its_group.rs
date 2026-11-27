// SPDX-License-Identifier: CC0-1.0

//! What an Amazon ECS service's scheduler does — and does not do — to the Elastic Load Balancing
//! v2 registration of a task it placed, once an Elastic Load Balancing caller has changed the
//! group's membership itself.
//!
//! RULE: **a task's target registration is an EVENT of its placement: "When a task from this
//! service is placed ... the container instance and port combination is registered as a target in
//! the target group that's specified here" (`CreateService$loadBalancers`). So Amazon ECS
//! registers the task once it is placed and deregisters it when the task stops, and in between the
//! membership of the group is whatever the Elastic Load Balancing callers left there: a target one
//! of them deregistered stays out while its task runs — draining for the group's
//! `deregistration_delay.timeout_seconds` and then gone — and the load balancer goes on answering
//! its own `503` for a group with no target to send the request to. A caller's own
//! `RegisterTargets` is what puts such a target back, and the next task PLACEMENT is registered as
//! the first one was.**
//!
//! `DeregisterTargets` is the call that takes a target out, `TargetHealth$Reason`
//! `Target.DeregistrationInProgress` is "the target is in the process of being deregistered and
//! the deregistration delay period has not expired", and `deregistration_delay.timeout_seconds` is
//! "the amount of time, in seconds, for Elastic Load Balancing to wait before changing the state
//! of a deregistering target from `draining` to `unused`"
//! (`smithy_query(query="doc",shape="TargetGroupAttribute",member="Key")`).
//!
//! Two services have to agree for that to hold, which is why this suite lives here: Amazon ECS
//! places the task and owns the registration it makes for it, and Elastic Load Balancing v2 holds
//! the group, reports the registration's health and answers the client. Every fact is created
//! through the owning service's own API and read back through one, and every wait polls the
//! CONDITION rather than a fixed time.

use std::time::{Duration, Instant};

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

// The ONE fixture set for "an Application Load Balancer in front of an `awsvpc` Amazon ECS
// workload": the assembled emulator, the network, the revision, the front door, the service, the
// health reader and the two data-plane exchanges — from the suite that owns them.
use crate::ecs_alb_serves_its_awsvpc_tasks_container::{
    create_service, dialled, ecs_ok, elbv2, emulator, front_door, get, register_revision,
    reported_health, seed_network, serving_command, Network, ReportedHealth, CLUSTER, HEALTHY,
};

/// The port the container's server listens on, which is the `containerPort` the revision declares
/// and the binding names, and the target group's own port.
const PORT: i64 = 80;

/// The marker the container's own program writes into the document it serves, so a body anything
/// else produced fails an assertion.
const MARKER: &str = "DEREGISTERED-STAYS-OUT";

/// The statuses the client is answered with: the target's own for the document it holds, and the
/// load balancer's own for a group it has no target to send the request to.
const SERVED_STATUS: u16 = 200;
const NO_TARGET_STATUS: u16 = 503;

/// The `TargetHealth$State` and `TargetHealth$Reason` of a target a deregistration is draining.
const DRAINING: &str = "draining";
const DEREGISTRATION_IN_PROGRESS: &str = "Target.DeregistrationInProgress";

/// The target-group attribute that decides how long a deregistering target drains, and the two
/// values this suite drives it at: the shortest its documented range admits ("range 0-3600"), so a
/// deregistration takes effect at once, and the five documented minutes it defaults to, which is
/// long enough that a drain observed inside this suite is a drain that is still running.
const DEREGISTRATION_DELAY: &str = "deregistration_delay.timeout_seconds";
const NO_DRAIN: &str = "0";
const DEFAULT_DRAIN: &str = "300";

/// How long a clock-owed move — a task's placement, the registration it owes, the verdict the
/// group's checks reach — is waited for, and how often it is looked at. Generous on purpose: the
/// deadline is here so a failure is a failure rather than a hang.
const DEADLINE: Duration = Duration::from_secs(120);
const POLL: Duration = Duration::from_millis(100);

/// How long a state that must HOLD is looked at: long enough for the scheduler's own reconcile to
/// have run many times over (every read of a service, a task or a task listing drives one) rather
/// than one look that could land between two of them.
const HOLD: Duration = Duration::from_secs(6);

/// Drive the scheduler's own reconcile the way a client does: reading the service, its tasks and
/// the task listing is what makes Amazon ECS settle the registrations its placements owe.
async fn let_the_scheduler_run(router: &MultiServiceRouter, service: &str) {
    ecs_ok(
        router,
        "DescribeServices",
        json!({ "cluster": CLUSTER, "services": [service] }),
    )
    .await;
    let listed = ecs_ok(
        router,
        "ListTasks",
        json!({ "cluster": CLUSTER, "serviceName": service }),
    )
    .await;
    let arns: Vec<String> = listed["taskArns"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if !arns.is_empty() {
        ecs_ok(
            router,
            "DescribeTasks",
            json!({ "cluster": CLUSTER, "tasks": arns }),
        )
        .await;
    }
}

/// Look at `group` until it reports exactly the states `expected` names, in that order, and answer
/// what it then reports.
async fn reaching(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
    expected: &[&str],
) -> Vec<ReportedHealth> {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let reported = reported_health(router, group).await;
        let states: Vec<&str> = reported.iter().map(|entry| entry.state.as_str()).collect();
        if states == expected {
            return reported;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the group to report {expected:?}, got {reported:?}"
        );
        ::tokio::time::sleep(POLL).await;
    }
}

/// Look at `group` and at what the load balancer answers across [`HOLD`], driving the scheduler's
/// reconcile on every look: the states must stay exactly `expected` and the answer exactly
/// `status`, so a membership the scheduler put back part-way through fails here rather than being
/// missed by a single read.
async fn stays(
    router: &MultiServiceRouter,
    case: &str,
    service: &str,
    group: &str,
    dns_name: &str,
    expected: &[&str],
    status: u16,
) {
    let until = Instant::now() + HOLD;
    loop {
        let_the_scheduler_run(router, service).await;
        let reported = reported_health(router, group).await;
        let states: Vec<&str> = reported.iter().map(|entry| entry.state.as_str()).collect();
        assert_eq!(
            states, expected,
            "case {case:?}: expected the group to go on reporting {expected:?} on every look, got \
             {reported:?}"
        );
        let answered = get(router, dns_name, "/").await;
        assert_eq!(
            answered.status, status,
            "case {case:?}: expected {status} on every look, got {} with body {:?}",
            answered.status, answered.body
        );
        if status == NO_TARGET_STATUS {
            assert!(
                !answered.body.contains(MARKER),
                "case {case:?}: the load balancer's own status carries no document of the target, \
                 got {:?}",
                answered.body
            );
        }
        if Instant::now() >= until {
            return;
        }
        ::tokio::time::sleep(POLL).await;
    }
}

/// Write `value` to the group's [`DEREGISTRATION_DELAY`].
async fn drains_for(router: &MultiServiceRouter, group: &str, value: &str) {
    elbv2(
        router,
        "ModifyTargetGroupAttributes",
        &[
            ("TargetGroupArn", group),
            ("Attributes.member.1.Key", DEREGISTRATION_DELAY),
            ("Attributes.member.1.Value", value),
        ],
    )
    .await;
}

/// `DeregisterTargets` the one registration `address` names, as a client of Elastic Load Balancing
/// does.
async fn deregister(router: &MultiServiceRouter, group: &str, address: &str) {
    elbv2(
        router,
        "DeregisterTargets",
        &[
            ("TargetGroupArn", group),
            ("Targets.member.1.Id", address),
            ("Targets.member.1.Port", &PORT.to_string()),
        ],
    )
    .await;
}

/// `RegisterTargets` that same address back, as a client of Elastic Load Balancing does.
async fn register(router: &MultiServiceRouter, group: &str, address: &str) {
    elbv2(
        router,
        "RegisterTargets",
        &[
            ("TargetGroupArn", group),
            ("Targets.member.1.Id", address),
            ("Targets.member.1.Port", &PORT.to_string()),
        ],
    )
    .await;
}

/// Stand the topology up: the revision whose container serves [`MARKER`], the front door and the
/// one-task service bound to it — answered with the group, the published address and the address
/// the task holds.
async fn fronted_service(
    router: &MultiServiceRouter,
    network: &Network,
    case: &str,
    family: &str,
    group_name: &str,
    load_balancer_name: &str,
    service_name: &str,
) -> (String, String, String) {
    let command = serving_command(MARKER);
    let borrowed: Vec<&str> = command.iter().map(String::as_str).collect();
    let revision = register_revision(router, family, PORT, &borrowed).await;
    let (group, dns_name) =
        front_door(router, network, case, group_name, load_balancer_name, PORT).await;
    create_service(router, network, service_name, &revision, &group, PORT).await;
    let healthy = reaching(router, case, &group, &[HEALTHY]).await;
    let address = healthy[0].id.clone();
    assert_eq!(
        healthy[0].port,
        PORT.to_string(),
        "case {case:?}: the placement registers the task on the binding's own port, got {healthy:?}"
    );
    let serving = get(router, &dns_name, "/").await;
    assert_eq!(
        (serving.status, serving.body.trim()),
        (SERVED_STATUS, MARKER),
        "case {case:?}: the registered task's own document is what the load balancer answers, got \
         {} with {:?}",
        serving.status,
        serving.body
    );
    (group, dns_name, address)
}

/// RULE: a target an Elastic Load Balancing caller deregistered stays out of the group while its
/// task runs — gone where the group drains for no time, and `draining` where it drains for the
/// documented default — and the load balancer answers its own `503` for the whole time the group
/// has no target to send a request to; the caller's own `RegisterTargets` is what puts it back.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_deregistered_task_target_is_not_put_back_by_the_scheduler() {
    let router = emulator();
    let network = seed_network(&router).await;
    ecs_ok(&router, "CreateCluster", json!({ "clusterName": CLUSTER })).await;
    let case = "a one-task service whose placement registered its only target";
    let (group, dns_name, address) = fronted_service(
        &router,
        &network,
        case,
        "staysoutfamily",
        "staysout-tg",
        "staysout-lb",
        "staysout",
    )
    .await;

    // GONE: a group that drains for no time has the deregistration take effect at once, so it
    // holds no registration at all — and the scheduler whose placement made that registration does
    // not make it again while the task runs, so the load balancer has no target for this request
    // for as long as the group is left that way.
    let gone = "the only target deregistered out of a group that drains for no time";
    drains_for(&router, &group, NO_DRAIN).await;
    deregister(&router, &group, &address).await;
    reaching(&router, gone, &group, &[]).await;
    stays(
        &router,
        gone,
        "staysout",
        &group,
        &dns_name,
        &[],
        NO_TARGET_STATUS,
    )
    .await;
    let off_the_socket = dialled(gone, &dns_name, "/").await;
    assert_eq!(
        off_the_socket.status, NO_TARGET_STATUS,
        "case {gone:?}: expected the load balancer's own {NO_TARGET_STATUS} on the published \
         address, got {} with body {:?}",
        off_the_socket.status, off_the_socket.body
    );

    // PUT BACK BY THE CALLER: `RegisterTargets` is what returns the address to the group, and the
    // task's own server answers through it again.
    let back = "the same address registered again by a caller";
    register(&router, &group, &address).await;
    let healthy = reaching(&router, back, &group, &[HEALTHY]).await;
    assert_eq!(
        healthy[0].id, address,
        "case {back:?}: the caller's registration names the same address, got {healthy:?}"
    );
    let serving = get(&router, &dns_name, "/").await;
    assert_eq!(
        (serving.status, serving.body.trim()),
        (SERVED_STATUS, MARKER),
        "case {back:?}: the task's own document is answered again, got {} with {:?}",
        serving.status,
        serving.body
    );

    // DRAINING: a group that drains for the documented default keeps the deregistering target
    // while the drain runs — reported `draining` with the model's own reason for it, receiving no
    // new request — and the scheduler does not return it to service inside that drain either.
    let drained = "the only target deregistered out of a group that drains for five minutes";
    drains_for(&router, &group, DEFAULT_DRAIN).await;
    deregister(&router, &group, &address).await;
    let reported = reaching(&router, drained, &group, &[DRAINING]).await;
    assert_eq!(
        reported[0].reason, DEREGISTRATION_IN_PROGRESS,
        "case {drained:?}: a deregistering target carries the model's own reason, got {reported:?}"
    );
    stays(
        &router,
        drained,
        "staysout",
        &group,
        &dns_name,
        &[DRAINING],
        NO_TARGET_STATUS,
    )
    .await;
}

/// RULE (the other half): registering is what a PLACEMENT does, so a task placed after a caller
/// emptied the group is registered exactly as the first one was — the rule leaves the scheduler's
/// own registration of a placement in place, it only stops it re-making one a caller removed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_task_placed_after_a_caller_emptied_the_group_is_registered() {
    let router = emulator();
    let network = seed_network(&router).await;
    ecs_ok(&router, "CreateCluster", json!({ "clusterName": CLUSTER })).await;
    let case = "a one-task service whose placement registered its only target";
    let (group, dns_name, first) = fronted_service(
        &router,
        &network,
        case,
        "placedagainfamily",
        "placedagain-tg",
        "placedagain-lb",
        "placedagain",
    )
    .await;

    // The caller empties the group, and it stays empty while that task runs.
    let emptied = "the only target deregistered";
    drains_for(&router, &group, NO_DRAIN).await;
    deregister(&router, &group, &first).await;
    reaching(&router, emptied, &group, &[]).await;
    stays(
        &router,
        emptied,
        "placedagain",
        &group,
        &dns_name,
        &[],
        NO_TARGET_STATUS,
    )
    .await;

    // A SECOND PLACEMENT: the service is asked for one more task, and that task's placement
    // registers its own address — so the group holds a registration again, at an address of the
    // newly placed task, and the load balancer serves the request through it.
    let placed = "a second task placed by the same service";
    ecs_ok(
        &router,
        "UpdateService",
        json!({ "cluster": CLUSTER, "service": "placedagain", "desiredCount": 2 }),
    )
    .await;
    let deadline = Instant::now() + DEADLINE;
    let second = loop {
        let reported = reported_health(&router, &group).await;
        let healthy = reported.iter().find(|entry| entry.state == HEALTHY);
        if let Some(entry) = healthy {
            break entry.id.clone();
        }
        assert!(
            Instant::now() < deadline,
            "case {placed:?}: expected the second placement to be registered and healthy, got \
             {reported:?}"
        );
        ::tokio::time::sleep(POLL).await;
    };
    assert_ne!(
        second, first,
        "case {placed:?}: the second placement holds an address of its own, got {second} twice"
    );
    let serving = get(&router, &dns_name, "/").await;
    assert_eq!(
        (serving.status, serving.body.trim()),
        (SERVED_STATUS, MARKER),
        "case {placed:?}: the newly placed task's own document is answered, got {} with {:?}",
        serving.status,
        serving.body
    );
}
