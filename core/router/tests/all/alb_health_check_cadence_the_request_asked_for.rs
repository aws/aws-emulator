// SPDX-License-Identifier: CC0-1.0

//! WHEN the health checks of an Application Load Balancer's target group reach their verdict,
//! over the cadence the request that created the group asked for.
//!
//! RULE: **a health check of a registration comes due one `HealthCheckIntervalSeconds` after the
//! last where the REQUEST asked for that cadence — the duration a caller configured keeps its
//! value for as long as the verdict it decides stays observable — and one span of this emulator's
//! own clock-owed transitions after the last where the request asked for none, because then the
//! wait between two checks is a span this emulator chose rather than one a client asked to
//! observe. The verdict itself stays a number of CHECKS
//! away, the counts the two thresholds carry, and what a READ reports is untouched: a group
//! created with no health-check members answers the documented defaults for every one of them.
//! So every target group reaches the verdicts its checks find — `unhealthy` with
//! `Target.ResponseCodeMismatch` for an answer its `Matcher` rejects, `healthy` again once the
//! checked path is repaired — and the load balancer still fails open to an all-unhealthy group
//! and answers its own 503 only once it has no target to send the request to.**
//!
//! `HealthCheckIntervalSeconds` is "the approximate amount of time, in seconds, between health
//! checks of an individual target ... If the target group protocol is TCP, TLS, UDP, TCP_UDP,
//! QUIC, TCP_QUIC, HTTP or HTTPS, the default is 30 seconds"
//! (`smithy_query(query="doc",shape="CreateTargetGroupInput",member="HealthCheckIntervalSeconds")`),
//! `HealthyThresholdCount` "the number of consecutive health check successes required before
//! considering a target healthy ... the default is 5" and `UnhealthyThresholdCount` "the number
//! of consecutive health check failures required before considering a target unhealthy ... the
//! default is 2". `TargetHealth$Reason` gives the verdict's reason:
//! "`Target.ResponseCodeMismatch` - The health checks did not return an expected HTTP code".
//!
//! The table spans what the rule generalises over — WHAT cadence the request asked for: none at
//! all, the shortest the member admits, and the longest it admits. The row that asks for nothing
//! is the one the rule is about; the two that ask for a cadence are the contrast, and they pin
//! that a configured duration is still the one the checks are spaced by.
//!
//! Three crates have to agree for any of it to hold, which is why this suite lives here: Amazon
//! ECS runs the container and records the server it listens with at the task's own address, EC2
//! owns the network and the rule that lets a check arrive, and Elastic Load Balancing v2 checks
//! the registration, reports its health and answers the client. The fixtures for that topology
//! belong to [`crate::ecs_alb_serves_its_awsvpc_tasks_container`] and are called from here
//! rather than copied. Every wait polls the CONDITION rather than a fixed time.

use std::time::{Duration, Instant};

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

// The ONE element readers this test crate keeps for awsQuery suites (`all/query_wire.rs`).
use crate::query_wire::{element, elements, members_of, opt_element, target_health_of};
// The fixtures for "an Application Load Balancer in front of an `awsvpc` Amazon ECS service":
// the emulator over one engine, the network, the revision, the service, the awsQuery sender and
// the two ways a client reaches the published address.
use crate::ecs_alb_serves_its_awsvpc_tasks_container::{
    create_service, dialled, ecs_ok, elbv2, emulator, get, register_revision, seed_network,
    serving_command, Network, CLUSTER,
};

/// The port the containers' servers listen on, which is the `containerPort` every revision
/// declares, the port every binding names and every target group's own.
const CONTAINER_PORT: i64 = 80;

/// The path the containers' own programs write a document at, and a path they hold nothing at —
/// which a row that measures a failing check is pointed at first.
const SERVED_PATH: &str = "/";
const MISSING_PATH: &str = "/not-deployed";

/// The statuses a client is answered with: the target's own for the document it holds, and the
/// load balancer's own for a group it has no target to send the request to.
const SERVED_STATUS: u16 = 200;
const NO_TARGET_STATUS: u16 = 503;

/// The status a static server answers a path its document root holds nothing at, which is what a
/// health check pointed at [`MISSING_PATH`] earns.
const NOT_FOUND_STATUS: u16 = 404;

/// The `TargetHealth$State` values this suite reads back, and the reason an answer the matcher
/// rejects carries.
const HEALTHY: &str = "healthy";
const UNHEALTHY: &str = "unhealthy";
const DRAINING: &str = "draining";
const RESPONSE_CODE_MISMATCH: &str = "Target.ResponseCodeMismatch";

/// How long a step waits for a clock-owed move — the task's start, the registration it owes, the
/// verdict the checks reach — and how often it looks. Generous on purpose: the deadline is only
/// here so a failure is a failure rather than a hang.
const DEADLINE: Duration = Duration::from_secs(120);
const POLL: Duration = Duration::from_millis(50);

/// One row: a target group created with the health-check members it names, in front of its own
/// load balancer and its own one-task service.
struct Row {
    /// What the row is, named in every assertion of it.
    case: &'static str,
    /// The cadence the create asks for, and `None` for a create that asks for none.
    asked_interval: Option<&'static str>,
    /// The two thresholds the create asks for, and `None` for a create that asks for neither.
    asked_thresholds: Option<&'static str>,
    /// What `DescribeTargetGroups` must then report for
    /// `(HealthCheckIntervalSeconds, HealthyThresholdCount, UnhealthyThresholdCount)` —
    /// the values asked for, else the documented defaults quoted in this suite's own docs.
    reported: (&'static str, &'static str, &'static str),
    /// Whether the group's health check is pointed at a path the container holds NOTHING at, so
    /// its checks fail and the row measures the `unhealthy` verdict, the fail-open answer and the
    /// repair; a row that is not starts with the path the container serves and measures the
    /// `healthy` verdict alone.
    checks_a_missing_path: bool,
    /// The names this row's own resources are created under, and the marker its container writes.
    family: &'static str,
    group_name: &'static str,
    load_balancer_name: &'static str,
    service_name: &'static str,
    marker: &'static str,
}

/// The rows, spanning the cadence a request can ask for.
const ROWS: [Row; 3] = [
    Row {
        case: "a target group created asking for no health-check cadence at all",
        asked_interval: None,
        asked_thresholds: None,
        reported: ("30", "5", "2"),
        checks_a_missing_path: true,
        family: "cadencedefaultfamily",
        group_name: "cadence-default-tg",
        load_balancer_name: "cadence-default-lb",
        service_name: "cadencedefault",
        marker: "CADENCE-DEFAULT-BODY",
    },
    Row {
        case: "a target group asking for the shortest cadence the member admits",
        asked_interval: Some("5"),
        asked_thresholds: Some("2"),
        reported: ("5", "2", "2"),
        checks_a_missing_path: true,
        family: "cadenceshortfamily",
        group_name: "cadence-short-tg",
        load_balancer_name: "cadence-short-lb",
        service_name: "cadenceshort",
        marker: "CADENCE-SHORT-BODY",
    },
    Row {
        case: "a target group asking for the longest cadence the member admits",
        asked_interval: Some("300"),
        asked_thresholds: None,
        reported: ("300", "5", "2"),
        checks_a_missing_path: false,
        family: "cadencelongfamily",
        group_name: "cadence-long-tg",
        load_balancer_name: "cadence-long-lb",
        service_name: "cadencelong",
        marker: "CADENCE-LONG-BODY",
    },
];

/// Create this row's `ip` target group, its internet-facing load balancer and the listener that
/// forwards to it — and answer the group's ARN with the address the load balancer published.
async fn front_door(router: &MultiServiceRouter, network: &Network, row: &Row) -> (String, String) {
    let port = CONTAINER_PORT.to_string();
    let mut params: Vec<(&str, &str)> = vec![
        ("Name", row.group_name),
        ("Protocol", "HTTP"),
        ("Port", port.as_str()),
        ("TargetType", "ip"),
        ("VpcId", network.vpc.as_str()),
        ("HealthCheckProtocol", "HTTP"),
        (
            "HealthCheckPath",
            if row.checks_a_missing_path {
                MISSING_PATH
            } else {
                SERVED_PATH
            },
        ),
    ];
    if let Some(interval) = row.asked_interval {
        params.push(("HealthCheckIntervalSeconds", interval));
    }
    if let Some(threshold) = row.asked_thresholds {
        params.push(("HealthyThresholdCount", threshold));
        params.push(("UnhealthyThresholdCount", threshold));
    }
    let created = elbv2(router, "CreateTargetGroup", &params).await;
    let group = opt_element(&created, "TargetGroupArn")
        .unwrap_or_else(|| panic!("case {:?}: no TargetGroupArn in {created}", row.case));
    let balanced = elbv2(
        router,
        "CreateLoadBalancer",
        &[
            ("Name", row.load_balancer_name),
            ("Type", "application"),
            ("Scheme", "internet-facing"),
            ("Subnets.member.1", network.subnets[0].as_str()),
            ("Subnets.member.2", network.subnets[1].as_str()),
            (
                "SecurityGroups.member.1",
                network.load_balancer_group.as_str(),
            ),
        ],
    )
    .await;
    let load_balancer = element(&balanced, "LoadBalancerArn");
    let dns_name = element(&balanced, "DNSName");
    elbv2(
        router,
        "CreateListener",
        &[
            ("LoadBalancerArn", load_balancer.as_str()),
            ("Protocol", "HTTP"),
            ("Port", "80"),
            ("DefaultActions.member.1.Type", "forward"),
            ("DefaultActions.member.1.TargetGroupArn", group.as_str()),
        ],
    )
    .await;
    (group, dns_name)
}

/// One registration as `DescribeTargetHealth` reports it: the address it names, its state, its
/// reason and its description — each read as empty where the answer carries none.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    id: String,
    state: String,
    reason: String,
    description: String,
}

/// Every registration the target group `group` holds, as it is reported.
async fn health(router: &MultiServiceRouter, group: &str) -> Vec<Reported> {
    let body = elbv2(router, "DescribeTargetHealth", &[("TargetGroupArn", group)]).await;
    members_of(&body)
        .into_iter()
        .map(|description| {
            // The health's own block, because the override beside it carries members of the
            // same names ([`target_health_of`]).
            let health = target_health_of(description);
            let text = |body: &str, tag: &str| {
                elements(body, tag)
                    .into_iter()
                    .next()
                    .unwrap_or_default()
            };
            Reported {
                id: text(description, "Id"),
                state: text(health, "State"),
                reason: text(health, "Reason"),
                description: text(health, "Description"),
            }
        })
        .collect()
}

/// Look at the group until its one registration reports `state` (and, where `reason` is given,
/// that reason), and answer that registration.
///
/// Polls the CONDITION — the task's start, the registration that start owes and the verdict the
/// checks reach are all clock-owed — rather than waiting a fixed time.
async fn reaching(
    router: &MultiServiceRouter,
    row: &Row,
    step: &str,
    group: &str,
    state: &str,
    reason: Option<&str>,
) -> Reported {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let reported = health(router, group).await;
        let settled = reported.len() == 1
            && reported[0].state == state
            && reason.is_none_or(|reason| reported[0].reason == reason);
        if settled {
            return reported[0].clone();
        }
        if Instant::now() >= deadline {
            // What the service itself says about the task it placed, so a row that never
            // converges names the reason rather than only the reported health.
            let described = ecs_ok(
                router,
                "DescribeServices",
                json!({ "cluster": CLUSTER, "services": [row.service_name] }),
            )
            .await;
            panic!(
                "case {:?} ({step}): expected one registration reported {state} {reason:?}, got \
                 {reported:?}; the service reports {described}",
                row.case
            );
        }
        ::tokio::time::sleep(POLL).await;
    }
}

/// Look at this row's load balancer until it reports `State.Code` `active`, which is when its
/// listener serves traffic on the name it published.
async fn active(router: &MultiServiceRouter, row: &Row) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let body = elbv2(
            router,
            "DescribeLoadBalancers",
            &[("Names.member.1", row.load_balancer_name)],
        )
        .await;
        let code = elements(&body, "Code").into_iter().next().unwrap_or_default();
        if code == "active" {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {:?}: expected the load balancer to reach active, got {code:?} in {body}",
            row.case
        );
        ::tokio::time::sleep(POLL).await;
    }
}

/// What `DescribeTargetGroups` reports for the three members a row names, in the row's order.
async fn reported_members(router: &MultiServiceRouter, group: &str) -> (String, String, String) {
    let body = elbv2(
        router,
        "DescribeTargetGroups",
        &[("TargetGroupArns.member.1", group)],
    )
    .await;
    (
        element(&body, "HealthCheckIntervalSeconds"),
        element(&body, "HealthyThresholdCount"),
        element(&body, "UnhealthyThresholdCount"),
    )
}

/// RULE: every target group reaches the verdicts its checks find — at the cadence the request
/// asked for, and at this emulator's own transition span where it asked for none — while the
/// control plane keeps reporting the member values AWS carries the group at.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_target_group_reaches_its_verdicts_whatever_cadence_the_request_asked_for() {
    let router = emulator();
    let network = seed_network(&router).await;
    ecs_ok(&router, "CreateCluster", json!({ "clusterName": CLUSTER })).await;

    // Every row's front door and service FIRST, so the spans their tasks' starts owe run
    // together rather than one row at a time.
    let mut stood_up: Vec<(&Row, String, String)> = Vec::new();
    for row in &ROWS {
        let command = serving_command(row.marker);
        let borrowed: Vec<&str> = command.iter().map(String::as_str).collect();
        let revision = register_revision(&router, row.family, CONTAINER_PORT, &borrowed).await;
        let (group, dns_name) = front_door(&router, &network, row).await;
        create_service(
            &router,
            &network,
            row.service_name,
            &revision,
            &group,
            CONTAINER_PORT,
        )
        .await;
        stood_up.push((row, group, dns_name));
    }

    for (row, group, dns_name) in &stood_up {
        active(&router, row).await;
        // What the READ reports is the member set AWS carries the group at, whatever this
        // emulator's clock then spaces the checks by.
        let reported = reported_members(&router, group).await;
        let expected = (
            row.reported.0.to_string(),
            row.reported.1.to_string(),
            row.reported.2.to_string(),
        );
        assert_eq!(
            reported, expected,
            "case {:?}: expected (interval, healthy, unhealthy) {expected:?}, got {reported:?}",
            row.case
        );

        if !row.checks_a_missing_path {
            // The checked path is one the container serves, so the first check of the
            // registration finds it passing and the verdict is `healthy` — the cadence spaces
            // the checks that FOLLOW one, so even the longest of them does not hold off the
            // first finding (and a cadence that would hold a verdict back past the span one
            // stays observable in is spaced to fit inside it).
            let healthy = reaching(&router, row, "the checked path served", group, HEALTHY, None)
                .await;
            assert_eq!(
                (healthy.reason.as_str(), healthy.description.as_str()),
                ("", ""),
                "case {:?}: a healthy registration carries no reason and no description, got \
                 {healthy:?}",
                row.case
            );
            let answered = get(&router, dns_name, SERVED_PATH).await;
            assert_eq!(
                answered.status, SERVED_STATUS,
                "case {:?}: expected the healthy target's own {SERVED_STATUS}, got {} with body \
                 {:?}",
                row.case, answered.status, answered.body
            );
            continue;
        }

        // The checks were ANSWERED, and the answer is not one the group's matcher counts as a
        // success: the registration reaches `unhealthy` with the model's own reason for that,
        // described by the status the checks got back.
        let unhealthy = reaching(
            &router,
            row,
            "the checked path answering outside the matcher",
            group,
            UNHEALTHY,
            Some(RESPONSE_CODE_MISMATCH),
        )
        .await;
        assert_eq!(
            unhealthy.description,
            format!("Health checks failed with these codes: [{NOT_FOUND_STATUS}]"),
            "case {:?}: the reason must be described by the status the checks got back, got \
             {unhealthy:?}",
            row.case
        );

        // FAIL OPEN: the group holds no healthy target, so the request goes to the unhealthy one
        // anyway and the client is answered with the CONTAINER's own response — in the plane and
        // over a real connection to the address the load balancer published.
        let answered = get(&router, dns_name, SERVED_PATH).await;
        assert_eq!(
            (answered.status, answered.body.contains(row.marker)),
            (SERVED_STATUS, true),
            "case {:?}: expected the unhealthy target's own {SERVED_STATUS} and document, got {} \
             with body {:?}",
            row.case,
            answered.status,
            answered.body
        );
        let off_the_socket = dialled(row.case, dns_name, SERVED_PATH).await;
        assert_eq!(
            (off_the_socket.status, off_the_socket.body.contains(row.marker)),
            (SERVED_STATUS, true),
            "case {:?}: expected the same fail-open answer on the published address, got {} with \
             body {:?}",
            row.case,
            off_the_socket.status,
            off_the_socket.body
        );

        // REPAIRED: the check is pointed at the path the container serves, so its checks pass and
        // the registration returns to `healthy` with no reason at all.
        elbv2(
            &router,
            "ModifyTargetGroup",
            &[
                ("TargetGroupArn", group.as_str()),
                ("HealthCheckPath", SERVED_PATH),
            ],
        )
        .await;
        let healthy = reaching(
            &router,
            row,
            "the checked path repaired",
            group,
            HEALTHY,
            None,
        )
        .await;
        assert_eq!(
            (healthy.reason.as_str(), healthy.description.as_str()),
            ("", ""),
            "case {:?}: a healthy registration carries no reason and no description, got \
             {healthy:?}",
            row.case
        );
        let answered = get(&router, dns_name, SERVED_PATH).await;
        assert_eq!(
            answered.status, SERVED_STATUS,
            "case {:?}: expected the healthy target's own {SERVED_STATUS}, got {} with body {:?}",
            row.case, answered.status, answered.body
        );

        // NO TARGET TO SEND IT TO: the one registration is deregistered, which stops it receiving
        // new requests while it drains out of the group — so the load balancer has no target for
        // this request and answers its own status, carrying no document of the target.
        elbv2(
            &router,
            "DeregisterTargets",
            &[
                ("TargetGroupArn", group.as_str()),
                ("Targets.member.1.Id", healthy.id.as_str()),
                ("Targets.member.1.Port", &CONTAINER_PORT.to_string()),
            ],
        )
        .await;
        reaching(
            &router,
            row,
            "the only target deregistered",
            group,
            DRAINING,
            None,
        )
        .await;
        let answered = get(&router, dns_name, SERVED_PATH).await;
        assert_eq!(
            (answered.status, answered.body.contains(row.marker)),
            (NO_TARGET_STATUS, false),
            "case {:?}: expected the load balancer's own {NO_TARGET_STATUS} carrying no document \
             of the target, got {} with body {:?}",
            row.case,
            answered.status,
            answered.body
        );
    }
}
