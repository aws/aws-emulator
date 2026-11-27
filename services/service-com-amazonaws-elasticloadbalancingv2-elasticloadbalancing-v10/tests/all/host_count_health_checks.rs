// SPDX-License-Identifier: CC0-1.0

//! What one HEALTH CHECK of a registered target observes, WHEN the verdict those checks reach
//! is in, and the host counts it is counted into — driven over the wire exactly as a client
//! drives it.
//!
//! Two rules are asserted here, both about the same periodic check.
//!
//! **WHAT one check observes**: a load balancer CHECKS the registrations of every target group
//! it routes to, and a registration is reported healthy only while the emulator holds
//! something that stands behind it — an address a network interface of the account holds, a
//! running instance — while one it holds nothing behind is reported `unhealthy` with the
//! reason its check failed for; a registration nothing could be observed about keeps the
//! health the clock owes it. The counts those verdicts add up to are published as the
//! `HealthyHostCount` / `UnHealthyHostCount` series of the `(target group, load balancer)`
//! pair, and a target group no listener forwards to publishes neither.
//!
//! **WHEN the verdict is in**: a verdict is a number of consecutive CHECKS away, and the
//! checks of one registration are `HealthCheckIntervalSeconds` apart ("the approximate amount
//! of time, in seconds, between health checks of an individual target"), so a registration
//! nothing answers for is reported `unhealthy` — with the reason its check failed for and a
//! description — no sooner than `UnhealthyThresholdCount` ("the number of consecutive health
//! check failures required before considering a target unhealthy") times that interval after
//! health checking on it began, and is `initial` rather than `healthy` for the whole of that
//! time.
//!
//! The rows of each table differ along what its rule generalises over: whether the account's
//! inventory holds something behind the registered address, how many registrations the group
//! holds, whether the inventory knows the group's network at all, and whether a listener
//! forwards to the group, for the first; the interval configured, the threshold configured and
//! whether anything answers, for the second. The recorded configuration an issue happens to
//! mention is a single row of each.
//!
//! The inventory is seeded through the EC2 store's own writer seam, and the series is read
//! out of the CloudWatch store those datapoints live in — the two stores this service
//! reaches on the shared engine — so what a row asserts is what another service's own
//! readers would answer. The series as a CLIENT reads it, through
//! `cloudwatch:GetMetricStatistics`, is asserted by `core/router/tests/all/`.

use std::time::{Duration, Instant};

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state as cloudwatch;
use ::service_com_amazonaws_ec2_amazonec2::ops::types as ec2_types;
use ::service_com_amazonaws_ec2_amazonec2::state as ec2;
use service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state::host_counts::{
    APPLICATION_NAMESPACE, HEALTHY_HOST_COUNT, LOAD_BALANCER_DIMENSION, TARGET_GROUP_DIMENSION,
    UNHEALTHY_HOST_COUNT,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::listener_rules::{create_listener, create_load_balancer};
use crate::provisioning_loop::{action, call, emulator, first_text, health_block, members, p};

/// How long a row waits for the verdict the group's own health-check cadence reaches, and how
/// often it looks. Generous on purpose — many times the longest cadence a row configures —
/// because the deadline is only here so a failure is a failure rather than a hang.
const DEADLINE: Duration = Duration::from_secs(120);
const POLL: Duration = Duration::from_millis(50);

/// The health-check cadence the groups below are created with unless a row names its own: a
/// verdict takes `UnhealthyThresholdCount` consecutive failing checks, each one interval after
/// the last, so the shortest interval `HealthCheckIntervalSeconds` admits ("the range is
/// 5-300") is what a suite that waits for one asks for.
const INTERVAL_SECONDS: i32 = 5;

/// The VPC the seeded inventory holds, and the subnet of it the interface sits in.
const VPC: &str = "vpc-0c0un70000000001";
const SUBNET: &str = "subnet-0c0un70000000001";
const SUBNET_CIDR: &str = "10.60.1.0/24";

/// A VPC id the seeded inventory holds NOTHING for, so nothing can be observed in it.
const UNKNOWN_VPC: &str = "vpc-0c0un70000000002";

/// The address the seeded network interface holds, and two the inventory holds nothing for.
const SERVED: &str = "10.60.1.10";
const DEAD: [&str; 2] = ["10.60.1.200", "10.60.1.201"];

/// Seed the account's EC2 inventory with one VPC, one subnet of it, and one network
/// interface holding [`SERVED`].
///
/// Written through the inventory's own writer seam (`insert_network_interface`), so the
/// interface is the record EC2's own readers answer with rather than a value this suite made
/// up.
fn seed_inventory(engine: &std::sync::Arc<engine::InMemoryEngine>) {
    let state = ec2::service_state(&**engine).expect("the shared engine holds an EC2 inventory");
    let mut store = state.lock();
    store.vpcs.insert(
        VPC.to_string(),
        ec2_types::Vpc {
            vpc_id: Some(VPC.to_string()),
            cidr_block: Some("10.60.0.0/16".to_string()),
            ..ec2_types::Vpc::default()
        },
    );
    store.subnets.insert(
        SUBNET.to_string(),
        ec2_types::Subnet {
            subnet_id: Some(SUBNET.to_string()),
            vpc_id: Some(VPC.to_string()),
            cidr_block: Some(SUBNET_CIDR.to_string()),
            availability_zone: Some("us-east-1a".to_string()),
            ..ec2_types::Subnet::default()
        },
    );
    store
        .insert_network_interface(ec2::NetworkInterfaceSpec {
            subnet_id: SUBNET.to_string(),
            vpc_id: VPC.to_string(),
            availability_zone: "us-east-1a".to_string(),
            region: "us-east-1".to_string(),
            description: "a workload of the account".to_string(),
            interface_type: ec2::NETWORK_INTERFACE_TYPE_INTERFACE.to_string(),
            private_ip_addresses: vec![SERVED.to_string()],
            ..ec2::NetworkInterfaceSpec::default()
        })
        .expect("the inventory holds the interface");
}

/// Create an `ip` target group in `vpc`, checked at this suite's own cadence, and answer its
/// ARN.
async fn ip_target_group(router: &MultiServiceRouter, name: &str, vpc: &str) -> String {
    let mut form = action("CreateTargetGroup");
    form.push(p("Name", name));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", "80"));
    form.push(p("TargetType", "ip"));
    form.push(p("VpcId", vpc));
    form.push(p(
        "HealthCheckIntervalSeconds",
        &INTERVAL_SECONDS.to_string(),
    ));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "setup {name}: expected 200 from CreateTargetGroup, got {} body {}",
        reply.status, reply.body
    );
    reply
        .text("TargetGroupArn")
        .unwrap_or_else(|| panic!("setup {name}: no TargetGroupArn in {}", reply.body))
}

/// Register every address in `addresses` with the target group `arn` names.
async fn register(router: &MultiServiceRouter, case: &str, arn: &str, addresses: &[&str]) {
    let mut form = action("RegisterTargets");
    form.push(p("TargetGroupArn", arn));
    for (index, address) in addresses.iter().enumerate() {
        form.push(p(&format!("Targets.member.{}.Id", index + 1), address));
    }
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from RegisterTargets, got {} body {}",
        reply.status, reply.body
    );
}

/// `(state, reason, description)` for every registration the group reports.
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
                first_text(&health, "Description").unwrap_or_default(),
            )
        })
        .collect()
}

/// The health of `arn`'s registrations, once every one of them reports `wanted`.
async fn health_reaching(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
    wanted: &str,
) -> Vec<(String, String, String)> {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let reported = health(router, case, arn).await;
        let settled = !reported.is_empty()
            && reported.iter().all(|(state, _, _)| state == wanted);
        if settled || Instant::now() >= deadline {
            assert!(
                settled,
                "case {case:?}: expected every registration to reach {wanted}, got {reported:?}"
            );
            return reported;
        }
        ::tokio::time::sleep(POLL).await;
    }
}

/// The `<family>/<name>/<id>` suffix of a load balancer ARN, and the
/// `targetgroup/<name>/<id>` suffix of a target group's: how an Elastic Load Balancing
/// dimension names each of them.
fn dimension_value(arn: &str, resource: &str) -> String {
    let marker = format!(":{resource}/");
    let (_, suffix) = arn
        .split_once(marker.as_str())
        .unwrap_or_else(|| panic!("{arn} carries a {resource} resource field"));
    if resource == "loadbalancer" {
        suffix.to_string()
    } else {
        format!("{resource}/{suffix}")
    }
}

/// The LAST datapoint of one host-count series as the metric store holds it, or `None` when
/// that store holds no such series at all.
fn published(
    engine: &std::sync::Arc<engine::InMemoryEngine>,
    metric: &str,
    target_group: &str,
    load_balancer: &str,
) -> Option<f64> {
    let state =
        cloudwatch::service_state(&**engine).expect("the shared engine holds a metric store");
    let store = state.lock();
    let key = cloudwatch::MetricKey {
        namespace: APPLICATION_NAMESPACE.to_string(),
        metric_name: metric.to_string(),
        dimensions: [
            (
                TARGET_GROUP_DIMENSION.to_string(),
                dimension_value(target_group, "targetgroup"),
            ),
            (
                LOAD_BALANCER_DIMENSION.to_string(),
                dimension_value(load_balancer, "loadbalancer"),
            ),
        ]
        .into_iter()
        .collect(),
    };
    let series = store.metrics.get(&key)?;
    series.last().map(|datum| datum.maximum)
}

/// The last datapoint of one host-count series, once it reports `wanted`.
///
/// The datapoint that carries a verdict the checks have just reached is the next one DUE
/// rather than the one already stored, and the series keeps the one-minute spacing the real
/// one has — which is what [`DEADLINE`] has to cover here as well as the verdict itself.
async fn published_reaching(
    engine: &std::sync::Arc<engine::InMemoryEngine>,
    case: &str,
    metric: &str,
    target_group: &str,
    load_balancer: &str,
    wanted: f64,
) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let found = published(engine, metric, target_group, load_balancer);
        if found == Some(wanted) || Instant::now() >= deadline {
            assert_eq!(
                found,
                Some(wanted),
                "case {case:?}: expected the {metric} series to reach {wanted}, got {found:?}"
            );
            return;
        }
        ::tokio::time::sleep(POLL).await;
    }
}

/// One row: a target group to build, what the inventory holds behind its registrations, and
/// the verdict and counts that implies.
struct Row {
    case: &'static str,
    name: &'static str,
    /// The VPC the group names, which is the inventory a check of it reads.
    vpc: &'static str,
    /// The addresses registered with it.
    addresses: &'static [&'static str],
    /// Whether a listener of the load balancer forwards to it.
    routed: bool,
    /// The `TargetHealth$State` every one of its registrations must reach.
    state: &'static str,
    /// The reason that state is reported with, or `""` for a state reported without one.
    reason: &'static str,
    /// The counts its series must carry, for a routed group.
    healthy: f64,
    unhealthy: f64,
}

#[tokio::test]
async fn a_registration_is_healthy_only_while_the_emulator_holds_something_behind_it() {
    let (router, engine) = emulator();
    seed_inventory(&engine);

    let rows: [Row; 4] = [
        Row {
            case: "an address a network interface of the account holds",
            name: "probe-served",
            vpc: VPC,
            addresses: &[SERVED],
            routed: true,
            state: "healthy",
            reason: "",
            healthy: 1.0,
            unhealthy: 0.0,
        },
        Row {
            case: "the recorded configuration: two addresses the inventory holds nothing for",
            name: "probe-dead",
            vpc: VPC,
            addresses: &DEAD,
            routed: true,
            state: "unhealthy",
            reason: "Target.Timeout",
            healthy: 0.0,
            unhealthy: 2.0,
        },
        Row {
            case: "a group whose network the inventory knows nothing about",
            name: "probe-unknown-vpc",
            vpc: UNKNOWN_VPC,
            addresses: &[DEAD[0]],
            routed: true,
            // Nothing was observed about the registration, so it keeps the health the clock
            // owes it rather than being failed on silence.
            state: "healthy",
            reason: "",
            healthy: 1.0,
            unhealthy: 0.0,
        },
        Row {
            case: "a group no listener forwards to",
            name: "probe-unrouted",
            vpc: VPC,
            addresses: &DEAD,
            routed: false,
            state: "unused",
            reason: "Target.NotInUse",
            healthy: 0.0,
            unhealthy: 0.0,
        },
    ];

    let load_balancer = create_load_balancer(&router, "probe-alb").await;
    let mut groups: Vec<String> = Vec::new();
    for row in &rows {
        let group = ip_target_group(&router, row.name, row.vpc).await;
        register(&router, row.case, &group, row.addresses).await;
        groups.push(group);
    }
    // One listener per routed group, each forwarding to it: what makes the group one the
    // load balancer checks.
    let mut port = 80;
    for (group, row) in groups.iter().zip(&rows) {
        if row.routed {
            create_listener(&router, &load_balancer, group, port).await;
            port += 1;
        }
    }

    for (group, row) in groups.iter().zip(&rows) {
        let reported = health_reaching(&router, row.case, group, row.state).await;
        assert_eq!(
            reported.len(),
            row.addresses.len(),
            "case {:?}: expected one description per registration, got {reported:?}",
            row.case
        );
        for (state, reason, description) in &reported {
            assert_eq!(
                (state.as_str(), reason.as_str()),
                (row.state, row.reason),
                "case {:?}: expected {}/{} , got {state}/{reason}",
                row.case,
                row.state,
                row.reason
            );
            // "A reason code is not provided" for a healthy target, and a non-healthy state
            // carries the detail its own reason is described by.
            assert_eq!(
                description.is_empty(),
                row.reason.is_empty(),
                "case {:?}: a state reported with a reason carries a description, got \
                 {state}/{reason}/{description:?}",
                row.case
            );
        }

        for (metric, expected) in [
            (HEALTHY_HOST_COUNT, row.healthy),
            (UNHEALTHY_HOST_COUNT, row.unhealthy),
        ] {
            if row.routed {
                published_reaching(
                    &engine,
                    row.case,
                    metric,
                    group,
                    &load_balancer,
                    expected,
                )
                .await;
                continue;
            }
            // A target group no listener forwards to is checked by no load balancer, so it
            // publishes no series at all under this one's dimension — not even a zero.
            let found = published(&engine, metric, group, &load_balancer);
            assert_eq!(
                found, None,
                "case {:?}: expected no {metric} datapoint, got {found:?}",
                row.case
            );
        }
    }

    // The counts are the group's OWN: a deregistration that empties a routed group takes
    // both of its counts to zero, at the instant the clock owes the removal.
    let dead_group = groups[1].clone();
    let mut form = action("DeregisterTargets");
    form.push(p("TargetGroupArn", dead_group.as_str()));
    for (index, address) in DEAD.iter().enumerate() {
        form.push(p(&format!("Targets.member.{}.Id", index + 1), address));
    }
    let reply = call(&router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case \"a deregistration of every registration\": expected 200, got {} body {}",
        reply.status, reply.body
    );
    let deadline = Instant::now() + DEADLINE;
    loop {
        let counts = published(&engine, UNHEALTHY_HOST_COUNT, &dead_group, &load_balancer);
        if counts == Some(0.0) || Instant::now() >= deadline {
            assert_eq!(
                counts,
                Some(0.0),
                "case \"a deregistration of every registration\": expected the \
                 {UNHEALTHY_HOST_COUNT} series to reach 0, got {counts:?}"
            );
            break;
        }
        ::tokio::time::sleep(POLL).await;
    }
}

/// The `TargetHealthReasonEnum` values `TargetHealth$Reason` documents for the `unhealthy`
/// state: "`Target.ResponseCodeMismatch`", "`Target.Timeout`", "`Target.FailedHealthChecks`",
/// "`Elb.InternalError`" — of which a check nothing answered at all is one of the middle two.
const UNANSWERED_REASONS: [&str; 2] = ["Target.Timeout", "Target.FailedHealthChecks"];

/// The state a registration is in while its checks have not decided its health yet, which is
/// what it is reported as for the whole of the span before its verdict is due.
const INITIAL: &str = "initial";
const HEALTHY: &str = "healthy";
const UNHEALTHY: &str = "unhealthy";

/// One row of the CADENCE table: the schedule a target group is created with, what the
/// emulator holds behind the address registered with it, and the verdict that implies.
struct CadenceRow {
    case: &'static str,
    /// The target group's name.
    name: &'static str,
    /// `HealthCheckIntervalSeconds`, the span between two checks of one registration.
    interval_seconds: u64,
    /// `UnhealthyThresholdCount`, how many consecutive failing checks the verdict takes.
    unhealthy_threshold: u32,
    /// The address registered with the group: one no network interface of the account holds,
    /// so every check of it is answered by nothing, or the one an interface does hold.
    address: &'static str,
    /// The `TargetHealth$State` the registration must settle on.
    reaches: &'static str,
}

impl CadenceRow {
    /// The shortest span this row's schedule can reach its verdict in: the FIRST check is
    /// owed when health checking on the registration begins and every later one is one
    /// interval after the last, so the threshold of consecutive checks spans one interval
    /// fewer than that threshold.
    fn soonest_verdict(&self) -> Duration {
        Duration::from_secs(self.interval_seconds)
            .saturating_mul(self.unhealthy_threshold.saturating_sub(1))
    }
}

const CADENCE_ROWS: [CadenceRow; 4] = [
    CadenceRow {
        case: "the recorded schedule: a threshold of two checks five seconds apart",
        name: "cadence-two-by-five",
        interval_seconds: 5,
        unhealthy_threshold: 2,
        address: DEAD[0],
        reaches: UNHEALTHY,
    },
    CadenceRow {
        case: "a threshold of three checks at the same cadence",
        name: "cadence-three-by-five",
        interval_seconds: 5,
        unhealthy_threshold: 3,
        address: DEAD[0],
        reaches: UNHEALTHY,
    },
    CadenceRow {
        case: "a cadence one second longer, at the same threshold",
        name: "cadence-two-by-six",
        interval_seconds: 6,
        unhealthy_threshold: 2,
        address: DEAD[0],
        reaches: UNHEALTHY,
    },
    CadenceRow {
        case: "an address a network interface of the account holds",
        name: "cadence-served",
        interval_seconds: 5,
        unhealthy_threshold: 2,
        address: SERVED,
        reaches: HEALTHY,
    },
];

/// Create the `ip` target group one cadence row describes and answer its ARN, having asserted
/// the create reports back the schedule it was asked for: what this table then times the
/// verdict against is the target group's own.
async fn cadence_target_group(router: &MultiServiceRouter, row: &CadenceRow) -> String {
    let mut form = action("CreateTargetGroup");
    form.push(p("Name", row.name));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", "80"));
    form.push(p("TargetType", "ip"));
    form.push(p("VpcId", VPC));
    form.push(p("HealthCheckProtocol", "HTTP"));
    form.push(p("HealthCheckPath", "/"));
    form.push(p(
        "HealthCheckIntervalSeconds",
        &row.interval_seconds.to_string(),
    ));
    form.push(p(
        "UnhealthyThresholdCount",
        &row.unhealthy_threshold.to_string(),
    ));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {:?}: expected 200 from CreateTargetGroup, got {} body {}",
        row.case, reply.status, reply.body
    );
    for (member, expected) in [
        (
            "HealthCheckIntervalSeconds",
            row.interval_seconds.to_string(),
        ),
        (
            "UnhealthyThresholdCount",
            row.unhealthy_threshold.to_string(),
        ),
    ] {
        let reported = reply.text(member);
        assert_eq!(
            reported.as_deref(),
            Some(expected.as_str()),
            "case {:?}: expected {member} {expected}, got {reported:?}",
            row.case
        );
    }
    reply
        .text("TargetGroupArn")
        .unwrap_or_else(|| panic!("case {:?}: no TargetGroupArn in {}", row.case, reply.body))
}

/// What one cadence row settled on: the first health it reported that was no longer
/// `initial`, how long after `started` that reading was taken, and every state it reported on
/// the way there.
struct Settled {
    reported: Vec<(String, String, String)>,
    after: Duration,
    seen: Vec<String>,
}

/// Look at `arn`'s health until it reports something other than `initial`, or [`DEADLINE`]
/// passes — the CONDITION, never a fixed wait.
async fn settling(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
    started: Instant,
) -> Settled {
    let deadline = Instant::now() + DEADLINE;
    let mut seen: Vec<String> = Vec::new();
    loop {
        let reported = health(router, case, arn).await;
        let after = started.elapsed();
        for (state, _, _) in &reported {
            if seen.last().map(String::as_str) != Some(state.as_str()) {
                seen.push(state.clone());
            }
        }
        let settled = !reported.is_empty() && reported.iter().all(|(state, _, _)| state != INITIAL);
        if settled || Instant::now() >= deadline {
            assert!(
                settled,
                "case {case:?}: expected every registration to leave {INITIAL}, got {reported:?} \
                 (states seen: {seen:?})"
            );
            return Settled {
                reported,
                after,
                seen,
            };
        }
        ::tokio::time::sleep(POLL).await;
    }
}

/// The span each row asserts is a MINIMUM, which is what the rule states: a loaded or slow
/// host takes longer to reach the verdict and the assertion holds all the more easily, and
/// nothing here asserts an upper bound on it — the deadline the polls give up after is many
/// times the longest cadence a row configures.
#[tokio::test]
async fn a_verdict_takes_its_threshold_of_checks_at_the_configured_interval() {
    let (router, engine) = emulator();
    seed_inventory(&engine);
    let load_balancer = create_load_balancer(&router, "cadence-alb").await;

    // Every row's group is attached before any verdict is waited for, so the one clock drives
    // all of their checks over the same span rather than one row's span after another's.
    let mut attached: Vec<(String, Instant)> = Vec::new();
    for (index, row) in CADENCE_ROWS.iter().enumerate() {
        let group = cadence_target_group(&router, row).await;
        register(&router, row.case, &group, &[row.address]).await;
        // The listener forwarding to the group is what starts the health checks, so the span
        // a verdict is owed runs from here. Taken BEFORE the request, so the span measured
        // can only be shorter than the real one and the minimum asserted below can never be
        // met by the emulator being quick.
        let started = Instant::now();
        create_listener(
            &router,
            &load_balancer,
            &group,
            90 + i32::try_from(index).expect("the row count fits an i32"),
        )
        .await;
        attached.push((group, started));
    }

    for (row, (group, started)) in CADENCE_ROWS.iter().zip(&attached) {
        let settled = settling(&router, row.case, group, *started).await;
        assert_eq!(
            settled.reported.len(),
            1,
            "case {:?}: expected one description per registration, got {:?}",
            row.case,
            settled.reported
        );
        for (state, reason, description) in &settled.reported {
            assert_eq!(
                state, row.reaches,
                "case {:?}: expected {}, got {state}/{reason}",
                row.case, row.reaches
            );
            if row.reaches == UNHEALTHY {
                // The verdict carries the reason the model names for a check nothing
                // answered, and the detail a non-healthy state is described by.
                assert!(
                    UNANSWERED_REASONS.contains(&reason.as_str()),
                    "case {:?}: expected one of {UNANSWERED_REASONS:?}, got {reason:?}",
                    row.case
                );
                assert!(
                    !description.is_empty(),
                    "case {:?}: a non-healthy state carries a description, got \
                     {state}/{reason}/{description:?}",
                    row.case
                );
                // The span the schedule takes: a verdict cannot be in before the threshold of
                // checks is, and the checks are one interval apart from the first one on.
                assert!(
                    settled.after >= row.soonest_verdict(),
                    "case {:?}: a verdict takes {:?} at this cadence, got it after {:?}",
                    row.case,
                    row.soonest_verdict(),
                    settled.after
                );
                // Nothing answered this registration from its first check on, so it was never
                // reported healthy on the way to the verdict.
                assert!(
                    !settled.seen.iter().any(|state| state == HEALTHY),
                    "case {:?}: a registration whose checks fail must never be reported \
                     {HEALTHY}, got the states {:?}",
                    row.case,
                    settled.seen
                );
                continue;
            }
            // "If the target state is healthy, a reason code is not provided", and no
            // description comes with it either.
            assert!(
                reason.is_empty() && description.is_empty(),
                "case {:?}: a healthy registration is reported without a reason or a \
                 description, got {reason:?}/{description:?}",
                row.case
            );
            assert!(
                !settled.seen.iter().any(|state| state == UNHEALTHY),
                "case {:?}: a registration whose checks are answered must never be reported \
                 {UNHEALTHY}, got the states {:?}",
                row.case,
                settled.seen
            );
        }
    }

    // The registration whose checks ARE answered is still healthy once every other row has
    // reached its verdict — many checks of it later, each one answered.
    let row = &CADENCE_ROWS[CADENCE_ROWS.len() - 1];
    let (group, _) = &attached[CADENCE_ROWS.len() - 1];
    let reported = health(&router, row.case, group).await;
    for (state, reason, description) in &reported {
        assert_eq!(
            (state.as_str(), reason.as_str(), description.as_str()),
            (HEALTHY, "", ""),
            "case {:?}: expected the answered registration to stay {HEALTHY} with no reason, \
             got {state}/{reason}/{description}",
            row.case
        );
    }
}
