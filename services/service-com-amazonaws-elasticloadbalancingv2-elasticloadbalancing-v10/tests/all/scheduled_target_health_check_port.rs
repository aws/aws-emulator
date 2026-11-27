// SPDX-License-Identifier: CC0-1.0

//! The health of a registration another emulated service's SCHEDULER owns follows whether
//! the workload behind it listens on the port the health check CONNECTS to.
//!
//! The rule asserted here: **a health check connects to the target group's effective
//! health-check port — `HealthCheckPort` when it names one, and the port the target
//! receives traffic on when it names `traffic-port` or nothing — so a registration whose
//! workload declares the ports it listens on is reported `healthy` only while that port is
//! one of them, and settles on `unhealthy` with the reason its check failed for, and a
//! description, when it is not. On the way to either verdict the registration is `initial`,
//! so one whose checks are failing is never reported healthy. A registration nothing
//! declares ports for is not failed on the port: nothing was observed about it.**
//!
//! The rows differ along what the rule generalises over: how the health-check port is
//! spelled (left out, `traffic-port`, a number equal to the traffic port, a number beside
//! it), how many ports the workload declares, whether it declares them one by one or as a
//! range, and whether it declares any at all. The configuration a recording happens to show
//! — a container port of 80 health-checked on 8888 — is a single row.
//!
//! The registrations are made through the state seam another service's scheduler registers
//! a workload through (`TargetGroupRecord::register_for`, what Amazon ECS calls for the
//! tasks its `loadBalancers` bind), because no operation of this service registers a
//! workload; everything asserted is read back over the wire through `DescribeTargetHealth`.
//! The end-to-end path from an ECS service to these verdicts is asserted by
//! `core/router/tests/all/ecs_service_target_group_health_check_port.rs`.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state as elb;
use smithy_server_runtime::MultiServiceRouter;

use crate::listener_rules::{create_listener, create_load_balancer};
use crate::provisioning_loop::{action, call, emulator, first_text, health_block, members, p, REGION};

/// How long a row waits for the verdict the group's own health-check cadence reaches, and how
/// often it looks. Generous on purpose — many times the cadence below — because the deadline
/// is only here so a failure is a failure rather than a hang.
const DEADLINE: Duration = Duration::from_secs(90);
const POLL: Duration = Duration::from_millis(50);

/// The health-check cadence every target group below is created with: a verdict takes
/// `UnhealthyThresholdCount` consecutive failing checks, each one interval after the last, so
/// the shortest interval `HealthCheckIntervalSeconds` admits ("the range is 5-300") is what a
/// suite that waits for one asks for.
const INTERVAL_SECONDS: &str = "5";

/// The VPC the target groups below are created in. The account's EC2 inventory holds nothing
/// for it, so the only thing a check of these registrations can observe is the port: the
/// workload itself runs in this emulator.
const VPC: &str = "vpc-0c0un70000000003";

/// The address each registration is made at, and the port it receives traffic on — the
/// `containerPort` form of a workload reached at its own interface.
const ADDRESS: &str = "10.80.1.10";
const TRAFFIC_PORT: i32 = 80;

/// One row: how the group's health check is pointed, what the workload declares it listens
/// on, and the verdict that implies.
struct Row {
    case: &'static str,
    /// The target group's name, and the listener port that makes it one a load balancer
    /// forwards to.
    name: &'static str,
    /// `HealthCheckPort` as the create spells it, or `None` for a create that leaves it out.
    health_check_port: Option<&'static str>,
    /// The ports the workload behind the registration declares it listens on, as inclusive
    /// ranges; empty for a workload that declares none.
    serves: &'static [(i32, i32)],
    /// The `TargetHealth$State` the registration must reach.
    state: &'static str,
    /// The `TargetHealth$Reason` that state is reported with, or `""` for a state reported
    /// without one.
    reason: &'static str,
    /// The `HealthCheckPort` the description must report: the port the check connects to.
    reported_port: &'static str,
}

/// The rows, in creation order. Each gets its own listener, one port apart from the first.
const ROWS: [Row; 8] = [
    Row {
        case: "no health-check port, so the traffic port, which the workload serves",
        name: "hcport-default",
        health_check_port: None,
        serves: &[(80, 80)],
        state: "healthy",
        reason: "",
        reported_port: "80",
    },
    Row {
        case: "the modelled traffic-port default spelled out",
        name: "hcport-traffic",
        health_check_port: Some("traffic-port"),
        serves: &[(80, 80)],
        state: "healthy",
        reason: "",
        reported_port: "80",
    },
    Row {
        case: "the traffic port named as a number",
        name: "hcport-same-number",
        health_check_port: Some("80"),
        serves: &[(80, 80)],
        state: "healthy",
        reason: "",
        reported_port: "80",
    },
    Row {
        case: "the recorded configuration: a port the workload does not listen on",
        name: "hcport-mismatch",
        health_check_port: Some("8888"),
        serves: &[(80, 80)],
        state: "unhealthy",
        reason: "Target.Timeout",
        reported_port: "8888",
    },
    Row {
        case: "another port the workload does not listen on, one away from one it does",
        name: "hcport-off-by-one",
        health_check_port: Some("81"),
        serves: &[(80, 80)],
        state: "unhealthy",
        reason: "Target.Timeout",
        reported_port: "81",
    },
    Row {
        case: "a second port the workload declares beside the traffic port",
        name: "hcport-second-port",
        health_check_port: Some("8080"),
        serves: &[(80, 80), (8080, 8080)],
        state: "healthy",
        reason: "",
        reported_port: "8080",
    },
    Row {
        case: "a port inside a range the workload declares",
        name: "hcport-in-range",
        health_check_port: Some("8050"),
        serves: &[(80, 80), (8000, 8100)],
        state: "healthy",
        reason: "",
        reported_port: "8050",
    },
    Row {
        case: "a workload that declares no ports at all, checked on a port beside the \
               traffic one",
        name: "hcport-undeclared",
        health_check_port: Some("8888"),
        serves: &[],
        state: "healthy",
        reason: "",
        reported_port: "8888",
    },
];

/// Create the `ip` target group one row describes and answer its ARN.
async fn target_group(router: &MultiServiceRouter, row: &Row) -> String {
    let mut form = action("CreateTargetGroup");
    form.push(p("Name", row.name));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", &TRAFFIC_PORT.to_string()));
    form.push(p("TargetType", "ip"));
    form.push(p("VpcId", VPC));
    form.push(p("HealthCheckIntervalSeconds", INTERVAL_SECONDS));
    if let Some(port) = row.health_check_port {
        form.push(p("HealthCheckPort", port));
    }
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
fn workload(index: usize) -> String {
    format!(
        "arn:aws:ecs:{REGION}:{}:task/health-check-port/{index:016x}",
        crate::provisioning_loop::account()
    )
}

/// Register [`ADDRESS`] with `arn` as the target of `workload`, declaring `serves` as the
/// ports that workload listens on — the seam another emulated service's scheduler registers
/// a workload's target through.
fn register_workload(
    engine: &Arc<engine::InMemoryEngine>,
    row: &Row,
    arn: &str,
    index: usize,
    serves: &[(i32, i32)],
) {
    let state = elb::service_state(&**engine).expect("the shared engine holds the ELBv2 store");
    let mut store = state.lock();
    let group = store
        .target_groups
        .get_mut(arn)
        .unwrap_or_else(|| panic!("case {:?}: the store holds the target group", row.case));
    let added = group.register_for(
        &elb::SchedulerRegistration {
            id: ADDRESS.to_string(),
            port: Some(TRAFFIC_PORT),
            availability_zone: None,
            workload: workload(index),
            serves_ports: serves.iter().map(|(first, last)| *first..=*last).collect(),
            // This suite's rule is about the PORT, so it declares no security groups: the
            // account's EC2 inventory holds none for this VPC either, so nothing can be
            // observed about whether a check of these registrations arrives.
            guarded_by: Vec::new().into(),
        },
        SystemTime::now(),
    );
    assert!(
        added,
        "case {:?}: the scheduler's registration must be added",
        row.case
    );
}

/// `(state, reason, description, health-check port)` for every registration the group
/// reports.
async fn health(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
) -> Vec<(String, String, String, String)> {
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
                first_text(&health, "Description").unwrap_or_default(),
                first_text(description, "HealthCheckPort").unwrap_or_default(),
            )
        })
        .collect()
}

/// The health of `arn`'s one registration, once it reports `wanted` — and every state it
/// reported on the way there, so a row can assert what it must never have reported.
async fn health_reaching(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
    wanted: &str,
) -> (Vec<(String, String, String, String)>, Vec<String>) {
    let deadline = Instant::now() + DEADLINE;
    let mut seen: Vec<String> = Vec::new();
    loop {
        let reported = health(router, case, arn).await;
        for (state, _, _, _) in &reported {
            if seen.last().map(String::as_str) != Some(state.as_str()) {
                seen.push(state.clone());
            }
        }
        let settled = !reported.is_empty() && reported.iter().all(|(state, ..)| state == wanted);
        if settled || Instant::now() >= deadline {
            assert!(
                settled,
                "case {case:?}: expected the registration to reach {wanted}, got {reported:?}"
            );
            return (reported, seen);
        }
        ::tokio::time::sleep(POLL).await;
    }
}

#[tokio::test]
async fn a_scheduled_registration_is_healthy_only_while_its_workload_serves_the_checked_port() {
    let (router, engine) = emulator();
    let load_balancer = create_load_balancer(&router, "hcport-alb").await;

    let mut groups: Vec<String> = Vec::new();
    for (index, row) in ROWS.iter().enumerate() {
        let group = target_group(&router, row).await;
        // A listener forwarding to the group is what makes it one the load balancer checks.
        create_listener(
            &router,
            &load_balancer,
            &group,
            TRAFFIC_PORT + i32::try_from(index).expect("the row count fits an i32"),
        )
        .await;
        register_workload(&engine, row, &group, index, row.serves);
        groups.push(group);
    }

    for (group, row) in groups.iter().zip(&ROWS) {
        let (reported, seen) = health_reaching(&router, row.case, group, row.state).await;
        assert_eq!(
            reported.len(),
            1,
            "case {:?}: expected one description per registration, got {reported:?}",
            row.case
        );
        for (state, reason, description, port) in &reported {
            assert_eq!(
                (state.as_str(), reason.as_str()),
                (row.state, row.reason),
                "case {:?}: expected {}/{}, got {state}/{reason}",
                row.case,
                row.state,
                row.reason
            );
            // "If the target state is healthy, a reason code is not provided", and a state
            // reported with a reason carries the detail that reason is described by.
            assert_eq!(
                description.is_empty(),
                row.reason.is_empty(),
                "case {:?}: a state reported with a reason carries a description, got \
                 {state}/{reason}/{description:?}",
                row.case
            );
            // The port the check connects to is the one the description reports.
            assert_eq!(
                port, row.reported_port,
                "case {:?}: expected the health check on port {}, got {port:?}",
                row.case, row.reported_port
            );
        }
        if row.state != "healthy" {
            assert!(
                !seen.iter().any(|state| state == "healthy"),
                "case {:?}: a registration whose checks fail must never be reported \
                 healthy, got the states {seen:?}",
                row.case
            );
        }
    }
}
