// SPDX-License-Identifier: CC0-1.0

//! A registration another emulated service's SCHEDULER makes CANCELS a deregistration already
//! in progress for the same target — read back through `DescribeTargetHealth`.
//!
//! RULE: **a target-group registration is `draining` only while it is being deregistered —
//! `TargetHealth$Reason` words that state `Target.DeregistrationInProgress`, "the target is in
//! the process of being deregistered and the deregistration delay period has not expired" — so
//! registering the same target id and port again puts it back IN SERVICE: the deregistration is
//! cancelled and the registration is health-checked from that moment, rather than waiting out a
//! drain it is no longer in.** `RegisterTargets` already implements that for a caller's own
//! targets (`crate::target_deregistration_and_health_spans` asserts it over the wire); this
//! suite asserts the same of the state seam a scheduler registers a workload through
//! (`TargetGroupRecord::register_for`, what Amazon ECS calls for the tasks its `loadBalancers`
//! bind), because no operation of this service registers a workload.
//!
//! Why it matters: a rolling deployment replaces one task with another, and a replacement
//! placed at the address the deregistered task held is exactly this case. While the group kept
//! reporting that address `draining`, the load balancer sent it nothing for the whole
//! deregistration delay — a service in its own steady state reachable through no target. The
//! end-to-end path, from a CodeBuild-built image through an Amazon ECS rollout to the bytes the
//! load balancer answers, is asserted by
//! `core/router/tests/all/cicd_chain_rollout_settles_on_its_built_image.rs`.
//!
//! The rows differ along what the rule generalises over: WHOSE registration was draining (a
//! `RegisterTargets` caller's, and an earlier workload of the same scheduler's) and whether the
//! registration names a port of its own or takes the target group's.
//!
//! Every verdict is POLLED for until it holds, never slept for, and each deadline is generous: a
//! row asserts that the move HAPPENS, not how fast this host runs.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state as elb;
use smithy_server_runtime::MultiServiceRouter;

use crate::listener_rules::{create_listener, create_load_balancer};
use crate::provisioning_loop::{action, call, emulator, first_text, health_block, members, p, REGION};

/// How long a row waits for the verdict the group's own health-check cadence reaches, and how
/// often it looks. Generous on purpose — many times the cadence below — because the deadline is
/// only here so a failure is a failure rather than a hang.
const DEADLINE: Duration = Duration::from_secs(90);
const POLL: Duration = Duration::from_millis(50);

/// The health-check cadence every target group below is created with: the shortest
/// `HealthCheckIntervalSeconds` admits ("the range is 5-300"), so a suite waiting for a verdict
/// waits the least the configuration allows.
const INTERVAL_SECONDS: &str = "5";

/// The VPC the target groups below are created in. The account's Amazon EC2 inventory holds
/// nothing for it, so a check of these registrations observes nothing about the workload and the
/// verdict follows the registration alone.
const VPC: &str = "vpc-0c0un70000000004";

/// The address every registration below is made at, and the target group's own port.
const ADDRESS: &str = "10.81.1.11";
const GROUP_PORT: i32 = 80;

/// The port a registration that names one of its own receives traffic on — the other spelling
/// `RegisterTargets` admits ("you can override the port for a target when you register it").
const OVERRIDDEN_PORT: i32 = 8080;

/// The `TargetHealthStateEnum` values a row reads, and the `TargetHealth$Reason` the model gives
/// the `draining` state.
const DRAINING: &str = "draining";
const HEALTHY: &str = "healthy";
const DEREGISTRATION_IN_PROGRESS: &str = "Target.DeregistrationInProgress";

/// One row: whose registration is being deregistered when the scheduler registers the target
/// again, and which port that registration receives traffic on.
struct Row {
    case: &'static str,
    /// The target group's name, and the listener port that makes it one a load balancer
    /// forwards to.
    name: &'static str,
    listener_port: i32,
    /// Whether the draining registration was made by the same SCHEDULER (an earlier workload of
    /// its own) rather than by a `RegisterTargets` caller.
    drained_was_a_schedulers: bool,
    /// The port the registration names, or `None` for one that takes the group's own.
    port: Option<i32>,
}

const ROWS: [Row; 4] = [
    Row {
        case: "a RegisterTargets caller's registration, on the group's own port",
        name: "drain-callers-group-port",
        listener_port: 80,
        drained_was_a_schedulers: false,
        port: None,
    },
    Row {
        case: "an earlier workload of the same scheduler's, on the group's own port",
        name: "drain-schedulers-group-port",
        listener_port: 81,
        drained_was_a_schedulers: true,
        port: None,
    },
    Row {
        case: "a RegisterTargets caller's registration, on a port of its own",
        name: "drain-callers-own-port",
        listener_port: 82,
        drained_was_a_schedulers: false,
        port: Some(OVERRIDDEN_PORT),
    },
    Row {
        case: "an earlier workload of the same scheduler's, on a port of its own",
        name: "drain-schedulers-own-port",
        listener_port: 83,
        drained_was_a_schedulers: true,
        port: Some(OVERRIDDEN_PORT),
    },
];

/// Create the `ip` target group one row describes and answer its ARN.
async fn target_group(router: &MultiServiceRouter, row: &Row) -> String {
    let mut form = action("CreateTargetGroup");
    form.push(p("Name", row.name));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", &GROUP_PORT.to_string()));
    form.push(p("TargetType", "ip"));
    form.push(p("VpcId", VPC));
    form.push(p("HealthCheckIntervalSeconds", INTERVAL_SECONDS));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {:?}: expected 200 from CreateTargetGroup, got {} body {}",
        row.case, reply.status, reply.body
    );
    reply
        .text("TargetGroupArn")
        .unwrap_or_else(|| panic!("case {:?}: no TargetGroupArn in {}", row.case, reply.body))
}

/// The workload identifier a scheduler registers its target for: the form an Amazon ECS task
/// ARN takes, which is what tells one scheduler's registrations from another's.
fn workload(label: &str, index: usize) -> String {
    format!(
        "arn:aws:ecs:{REGION}:{}:task/cancelled-drain/{label}{index:014x}",
        crate::provisioning_loop::account()
    )
}

/// Drive one `RegisterTargets` or `DeregisterTargets` over the wire for [`ADDRESS`] on `port`.
async fn targets_call(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    arn: &str,
    port: Option<i32>,
) {
    let mut form = action(operation);
    form.push(p("TargetGroupArn", arn));
    form.push(p("Targets.member.1.Id", ADDRESS));
    if let Some(port) = port {
        form.push(p("Targets.member.1.Port", &port.to_string()));
    }
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from {operation}, got {} body {}",
        reply.status, reply.body
    );
}

/// Register [`ADDRESS`] with `arn` as the target of `workload` through the seam another
/// emulated service's scheduler registers a workload's target through, and answer whether that
/// call put a registration in service.
fn register_workload(
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    arn: &str,
    port: Option<i32>,
    workload: &str,
) -> bool {
    let state = elb::service_state(&**engine).expect("the shared engine holds the ELBv2 store");
    let mut store = state.lock();
    let group = store
        .target_groups
        .get_mut(arn)
        .unwrap_or_else(|| panic!("case {case:?}: the store holds the target group"));
    group.register_for(
        &elb::SchedulerRegistration {
            id: ADDRESS.to_string(),
            port,
            availability_zone: None,
            workload: workload.to_string(),
            // This suite's rule is about the DRAIN, so the registration declares neither the
            // ports its workload listens on nor the groups governing it: nothing is then
            // observed against it beyond the registration itself.
            serves_ports: Vec::new().into(),
            guarded_by: Vec::new().into(),
        },
        SystemTime::now(),
    )
}

/// `(State, Reason, Port)` for every registration `arn` reports, through the modelled read.
async fn health(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
) -> Vec<(String, String, String)> {
    let mut form = action("DescribeTargetHealth");
    form.push(p("TargetGroupArn", arn));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from DescribeTargetHealth, got {} body {}",
        reply.status, reply.body
    );
    members(&reply.body, "TargetHealthDescriptions")
        .iter()
        .map(|description| {
            // The health's own element, because the override beside it carries members of
            // the same names ([`health_block`]).
            let health = health_block(description);
            (
                first_text(&health, "State").unwrap_or_default(),
                first_text(&health, "Reason").unwrap_or_default(),
                first_text(description, "Port").unwrap_or_default(),
            )
        })
        .collect()
}

/// Look again at `arn` until its one registration reports `wanted`, and answer every
/// registration the group then holds.
async fn health_reaching(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
    wanted: &str,
) -> Vec<(String, String, String)> {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let reported = health(router, case, arn).await;
        let settled = !reported.is_empty() && reported.iter().all(|(state, ..)| state == wanted);
        if settled {
            return reported;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the registration to reach {wanted}, got {reported:?}"
        );
        ::tokio::time::sleep(POLL).await;
    }
}

/// RULE: registering a target that is being deregistered cancels that deregistration — the
/// group stops reporting it `draining` and it reaches the verdict of the checks made of it from
/// that moment — whoever made the registration being drained and whichever port it receives
/// traffic on.
#[tokio::test]
async fn a_scheduler_registering_a_draining_target_cancels_its_deregistration() {
    let (router, engine) = emulator();
    let load_balancer = create_load_balancer(&router, "cancelled-drain-alb").await;

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let group = target_group(&router, row).await;
        // A listener forwarding to the group is what makes it one the load balancer checks, and
        // what gives a deregistration of one of its targets connections to drain at all.
        create_listener(&router, &load_balancer, &group, row.listener_port).await;

        // The registration that is going to be deregistered: a caller's own, or an earlier
        // workload of the scheduler's.
        if row.drained_was_a_schedulers {
            assert!(
                register_workload(&engine, case, &group, row.port, &workload("earlier", index)),
                "case {case:?}: the scheduler's first registration must be put in service"
            );
        } else {
            targets_call(&router, case, "RegisterTargets", &group, row.port).await;
        }
        let in_service = health_reaching(&router, case, &group, HEALTHY).await;
        assert_eq!(
            in_service.len(),
            1,
            "case {case:?}: expected one registration before the drain, got {in_service:?}"
        );

        // Deregistered: the group holds it, reporting the drain it is in.
        targets_call(&router, case, "DeregisterTargets", &group, row.port).await;
        let draining = health(&router, case, &group).await;
        assert_eq!(
            draining.len(),
            1,
            "case {case:?}: a draining registration is still in the group, got {draining:?}"
        );
        assert_eq!(
            (draining[0].0.as_str(), draining[0].1.as_str()),
            (DRAINING, DEREGISTRATION_IN_PROGRESS),
            "case {case:?}: expected the deregistration to be in progress, got {draining:?}"
        );

        // And registered again, for the workload the scheduler has just placed: the
        // deregistration is cancelled, so the registration is checked from now and reaches the
        // verdict of those checks instead of leaving the group when the drain runs out.
        assert!(
            register_workload(&engine, case, &group, row.port, &workload("placed", index)),
            "case {case:?}: registering a draining target must put it back in service"
        );
        let back = health_reaching(&router, case, &group, HEALTHY).await;
        assert_eq!(
            back.len(),
            1,
            "case {case:?}: expected the one registration back in service, got {back:?}"
        );
        assert_eq!(
            back[0].1, "",
            "case {case:?}: \"if the target state is healthy, a reason code is not provided\", \
             got {back:?}"
        );
        assert_eq!(
            back[0].2,
            row.port.unwrap_or(GROUP_PORT).to_string(),
            "case {case:?}: the registration receives traffic on the port it was registered \
             with, got {back:?}"
        );
    }
}
