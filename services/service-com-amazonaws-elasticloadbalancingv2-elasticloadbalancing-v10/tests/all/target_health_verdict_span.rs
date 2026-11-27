// SPDX-License-Identifier: CC0-1.0

//! WHETHER a registration reaches the verdict its own health checks decide, whatever
//! health-check schedule the target group was created with.
//!
//! The rule asserted here: **a registration's reported health is the finding of the checks the
//! load balancer makes of it — `unhealthy`, worded as the family performing them words a check
//! nothing answered, once the group's `UnhealthyThresholdCount` consecutive checks found
//! nothing, and `healthy` with no reason at all once a check of it passed — and that verdict
//! is REACHED, for every cadence and threshold `HealthCheckIntervalSeconds` and
//! `HealthCheckThresholdCount` admit, because the run of consecutive checks a verdict takes is
//! spaced to fit inside the span this emulator keeps a verdict observable in. Nothing a READ
//! reports about the schedule changes: the three members answer the values the create stored.**
//!
//! `TargetHealthStateEnum` is `initial|healthy|unhealthy|unhealthy.draining|unused|draining|
//! unavailable` and `TargetHealth$Reason` gives a Network Load Balancer's wording of a check
//! that did not succeed — "`Target.FailedHealthChecks` - The load balancer received an error
//! while establishing a connection to the target or the target response was malformed" — while
//! "if the target state is healthy, a reason code is not provided"
//! (`smithy_query(query="shape",shape="TargetHealthStateEnum,TargetHealthReasonEnum")`,
//! `smithy_query(query="doc",shape="TargetHealth",member="Reason")`).
//!
//! What gates the checks here is the account's own firewall, which is the condition a client
//! diagnoses with this read: an internal Network Load Balancer is associated with no security
//! group of its own, so a group of instances carrying only a self-reference admits none of its
//! checks, and the one rule that can admit them is one covering the block the load balancer's
//! nodes sit in. The verdict is DERIVED from those rules rather than latched, so admitting that
//! block on the checked port takes the same registration from `unhealthy` to `healthy`
//! ([`crate::target_security_group_health_check`] tables which rules admit a check; this suite
//! is about the SCHEDULE the verdict is reached on).
//!
//! The rows differ along what the rule generalises over: the cadence the create asks for (the
//! shortest the member admits, a long one, the longest), how many consecutive checks the
//! thresholds make the verdict's run (the fewest either admits, the most), and which port the
//! checks go to and over which protocol (the group's own traffic port over its own protocol, and
//! a health-check port of its own requested over HTTP at a path the target's guest serves). The
//! recorded configuration — a Network Load Balancer's TCP group with an HTTP health check on a
//! port of its own, a twenty-second cadence and ten failing checks to a verdict — is one row. How
//! many checks the thresholds take, and the spacing each cadence and threshold pair yields, are
//! tabled in this crate's own `state::target_health` tests, which is where the arithmetic lives.
//!
//! Everything runs over the two services' own APIs on one engine, as
//! [`crate::target_security_group_health_check`] mounts them: the VPC, the subnet, the security
//! groups, their rules and the instances are EC2's records, and every wait polls its CONDITION.

use std::time::{Duration, Instant};

use smithy_server_runtime::MultiServiceRouter;

use crate::provisioning_loop::{action, call, first_text, members, p};
use crate::target_security_group_health_check::{ec2_ok, emulator_with_ec2, reported};

/// How long a leg waits for the verdict a group's own checks reach, and how often it looks.
///
/// Many times the longest run a row below can need, because the deadline is only here so a
/// failure is a failure rather than a hang: every wait looks for its CONDITION, so machine load
/// decides when a leg finishes and never whether it passes. Nothing here asserts on elapsed
/// time.
const DEADLINE: Duration = Duration::from_secs(120);
const POLL: Duration = Duration::from_millis(50);

/// An AMI id a launch names; nothing here asserts anything about the image.
const AMI: &str = "ami-0123456789abcdef0";

/// The network every row runs in: one VPC and the one subnet the load balancer's nodes and the
/// instances both sit in, so the block a rule admits is the block the checks come from.
const VPC_CIDR: &str = "172.28.0.0/16";
const SUBNET_CIDR: &str = "172.28.0.0/23";
const ZONE: &str = "us-west-2a";

/// The `TargetHealthStateEnum` values this suite reads.
const INITIAL: &str = "initial";
const HEALTHY: &str = "healthy";
const UNHEALTHY: &str = "unhealthy";

/// The `TargetHealthReasonEnum` value a Network Load Balancer reports a check that did not
/// succeed under.
const FAILED_HEALTH_CHECKS: &str = "Target.FailedHealthChecks";

/// The path the health check of an HTTP-checked row requests, and the document the guest of
/// that row's instance holds for it.
const HEALTH_CHECK_PATH: &str = "/health-check";

/// The port that row's guest serves that document on, which is the `HealthCheckPort` its target
/// group names.
const HEALTH_CHECK_PORT: &str = "4000";

/// One registration as `DescribeTargetHealth` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    id: String,
    port: Option<String>,
    state: String,
    reason: Option<String>,
    description: Option<String>,
    health_check_port: Option<String>,
}

/// One row: the schedule its target group is created with, where its checks go, and what its
/// guest leaves listening there.
struct Row {
    case: &'static str,
    /// What every resource name of this row is suffixed with.
    slug: &'static str,
    /// `CreateTargetGroup$Port`, the port the registration names the target as listening on.
    traffic_port: &'static str,
    /// `HealthCheckPort` where the row names one, and `None` for a row checked on the port its
    /// traffic goes to.
    health_check_port: Option<&'static str>,
    /// Whether the check speaks HTTP semantics — a protocol and a path of its own — rather than
    /// opening a connection to the port.
    checks_over_http: bool,
    /// `HealthCheckIntervalSeconds`, `HealthyThresholdCount` and `UnhealthyThresholdCount` as
    /// the create asks for them, and `None` for a member it leaves at its documented default.
    interval: &'static str,
    thresholds: Option<(&'static str, &'static str)>,
    /// `HealthCheckTimeoutSeconds` where the row names one — a check may not be left waiting
    /// for an answer past the next check's own instant, so a cadence shorter than the group's
    /// documented default timeout names its own.
    timeout: Option<&'static str>,
    /// The port the checks therefore connect to, which is what the remediation admits and what
    /// `HealthCheckPort` must read back as.
    checked_port: &'static str,
}

/// The rows: the recorded configuration, and the two ends of what the two members admit.
const ROWS: [Row; 3] = [
    Row {
        case: "the recorded configuration: a TCP group checked over HTTP on a port of its own, \
               at a twenty-second cadence and the most failing checks the threshold admits",
        slug: "recorded",
        traffic_port: "44300",
        health_check_port: Some(HEALTH_CHECK_PORT),
        checks_over_http: true,
        interval: "20",
        thresholds: Some(("2", "10")),
        timeout: Some("10"),
        checked_port: HEALTH_CHECK_PORT,
    },
    Row {
        case: "the shortest cadence the member admits and the fewest checks a threshold asks \
               for, checked on the port the traffic goes to",
        slug: "shortest",
        traffic_port: HEALTH_CHECK_PORT,
        health_check_port: None,
        checks_over_http: false,
        interval: "5",
        thresholds: Some(("2", "2")),
        timeout: Some("2"),
        checked_port: HEALTH_CHECK_PORT,
    },
    Row {
        case: "the longest cadence the member admits, at the fewest consecutive checks either \
               threshold asks for",
        slug: "longest",
        traffic_port: HEALTH_CHECK_PORT,
        health_check_port: None,
        checks_over_http: false,
        interval: "300",
        thresholds: Some(("2", "2")),
        timeout: None,
        checked_port: HEALTH_CHECK_PORT,
    },
];

/// The guest program every row's instance runs: a server listening on the port the checks go
/// to, holding the document an HTTP-checked row requests at its own path.
///
/// One program for every row, so what a row varies is the SCHEDULE and the port the checks are
/// pointed at rather than what the target answers.
fn guest_program() -> String {
    format!(
        "#!/bin/bash\n\
         mkdir -p /srv/health\n\
         echo ok > /srv/health{HEALTH_CHECK_PATH}\n\
         python3 -m http.server {HEALTH_CHECK_PORT} --directory /srv/health\n"
    )
}

/// Create the VPC and the one subnet of it every row runs in, and answer the subnet's id.
async fn seed_network(router: &MultiServiceRouter) -> (String, String) {
    let created = ec2_ok(router, "setup", "CreateVpc", &[p("CidrBlock", VPC_CIDR)]).await;
    let vpc = reported("setup", &created, "vpcId");
    let created = ec2_ok(
        router,
        "setup",
        "CreateSubnet",
        &[
            p("VpcId", &vpc),
            p("CidrBlock", SUBNET_CIDR),
            p("AvailabilityZone", ZONE),
        ],
    )
    .await;
    let subnet = reported("setup", &created, "subnetId");
    (vpc, subnet)
}

/// Create one security group of `vpc` carrying the SELF-REFERENCE a cluster security group
/// carries and nothing else: no rule of it names a load balancer outside the group, and none
/// names an address range at all.
async fn cluster_security_group(router: &MultiServiceRouter, vpc: &str, name: &str) -> String {
    let created = ec2_ok(
        router,
        "setup",
        "CreateSecurityGroup",
        &[
            p("VpcId", vpc),
            p("GroupName", name),
            p("GroupDescription", "the instances of this suite"),
        ],
    )
    .await;
    let group = reported("setup", &created, "groupId");
    ec2_ok(
        router,
        "setup",
        "AuthorizeSecurityGroupIngress",
        &[
            p("GroupId", &group),
            p("IpPermissions.1.IpProtocol", "-1"),
            p("IpPermissions.1.Groups.1.GroupId", &group),
        ],
    )
    .await;
    group
}

/// Launch one instance of `group` in `subnet`, running [`guest_program`], and answer its id.
async fn run_instance(
    router: &MultiServiceRouter,
    case: &str,
    subnet: &str,
    group: &str,
    address: &str,
) -> String {
    let encoded = ::aws_smithy_types::base64::encode(guest_program());
    let launched = ec2_ok(
        router,
        case,
        "RunInstances",
        &[
            p("ImageId", AMI),
            p("MinCount", "1"),
            p("MaxCount", "1"),
            p("SubnetId", subnet),
            p("SecurityGroupId.1", group),
            p("PrivateIpAddress", address),
            p("UserData", &encoded),
        ],
    )
    .await;
    reported(case, &launched, "instanceId")
}

/// Look at the instance until EC2 reports it running, which is the move its launch owes it: an
/// instance whose guest is not up yet answers nothing for a reason that is not the schedule
/// under test.
async fn running(router: &MultiServiceRouter, case: &str, instance: &str) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let described = ec2_ok(
            router,
            case,
            "DescribeInstances",
            &[p("InstanceId.1", instance)],
        )
        .await;
        let reached = described.contains("<name>running</name>");
        if reached || Instant::now() >= deadline {
            assert!(
                reached,
                "case {case:?}: expected EC2 to report {instance} running, got {described}"
            );
            return;
        }
        ::tokio::time::sleep(POLL).await;
    }
}

/// Create this row's target group and answer its ARN.
async fn target_group(router: &MultiServiceRouter, row: &Row, name: &str, vpc: &str) -> String {
    let mut form = action("CreateTargetGroup");
    form.push(p("Name", name));
    form.push(p("Protocol", "TCP"));
    form.push(p("Port", row.traffic_port));
    form.push(p("TargetType", "instance"));
    form.push(p("VpcId", vpc));
    form.push(p("HealthCheckIntervalSeconds", row.interval));
    if let Some(timeout) = row.timeout {
        form.push(p("HealthCheckTimeoutSeconds", timeout));
    }
    if let Some(port) = row.health_check_port {
        form.push(p("HealthCheckPort", port));
    }
    if row.checks_over_http {
        form.push(p("HealthCheckProtocol", "HTTP"));
        form.push(p("HealthCheckPath", HEALTH_CHECK_PATH));
    }
    if let Some((healthy, unhealthy)) = row.thresholds {
        form.push(p("HealthyThresholdCount", healthy));
        form.push(p("UnhealthyThresholdCount", unhealthy));
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

/// Create this row's INTERNAL Network Load Balancer — associated with no security group of its
/// own, so the only rule that can name it covers the block its nodes sit in — and the listener
/// that forwards to `group`, which is what starts the checks.
async fn forwarding_balancer(
    router: &MultiServiceRouter,
    row: &Row,
    name: &str,
    subnet: &str,
    group: &str,
) {
    let mut form = action("CreateLoadBalancer");
    form.push(p("Name", name));
    form.push(p("Type", "network"));
    form.push(p("Scheme", "internal"));
    form.push(p("Subnets.member.1", subnet));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {:?}: expected 200 from CreateLoadBalancer, got {} body {}",
        row.case, reply.status, reply.body
    );
    let balancer = reply
        .text("LoadBalancerArn")
        .unwrap_or_else(|| panic!("case {:?}: no LoadBalancerArn in {}", row.case, reply.body));
    let mut form = action("CreateListener");
    form.push(p("LoadBalancerArn", &balancer));
    form.push(p("Protocol", "TCP"));
    form.push(p("Port", row.traffic_port));
    form.push(p("DefaultActions.member.1.Type", "forward"));
    form.push(p("DefaultActions.member.1.TargetGroupArn", group));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {:?}: expected 200 from CreateListener, got {} body {}",
        row.case, reply.status, reply.body
    );
}

/// Register `instance` with the target group `arn` names, on the group's own port.
async fn register(router: &MultiServiceRouter, case: &str, arn: &str, instance: &str) {
    let mut form = action("RegisterTargets");
    form.push(p("TargetGroupArn", arn));
    form.push(p("Targets.member.1.Id", instance));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from RegisterTargets, got {} body {}",
        reply.status, reply.body
    );
}

/// Every registration the target group `arn` holds, as a client reads it.
async fn health(router: &MultiServiceRouter, case: &str, arn: &str) -> Vec<Reported> {
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
        .map(|description| Reported {
            id: first_text(description, "Id").unwrap_or_default(),
            port: first_text(description, "Port"),
            state: first_text(description, "State").unwrap_or_default(),
            reason: first_text(description, "Reason"),
            description: first_text(description, "Description"),
            health_check_port: first_text(description, "HealthCheckPort"),
        })
        .collect()
}

/// Look at `arn` until one of its registrations reports `wanted`, and answer that registration
/// beside every state reported on the way there — so a leg can assert what was never reported.
async fn health_reaching(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
    wanted: &str,
) -> (Option<Reported>, Vec<String>) {
    let deadline = Instant::now() + DEADLINE;
    let mut seen: Vec<String> = Vec::new();
    loop {
        let found = health(router, case, arn).await;
        for report in &found {
            if seen.last() != Some(&report.state) {
                seen.push(report.state.clone());
            }
        }
        let settled = found.iter().find(|report| report.state == wanted).cloned();
        if settled.is_some() || Instant::now() >= deadline {
            assert!(
                settled.is_some(),
                "case {case:?}: expected a registration to reach {wanted}, got {found:?} \
                 (states seen: {seen:?})"
            );
            return (settled, seen);
        }
        ::tokio::time::sleep(POLL).await;
    }
}

/// What `DescribeTargetGroups` reports for the schedule members of `arn`.
async fn reported_schedule(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
) -> (Option<String>, Option<String>, Option<String>) {
    let mut form = action("DescribeTargetGroups");
    form.push(p("TargetGroupArns.member.1", arn));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from DescribeTargetGroups, got {} body {}",
        reply.status, reply.body
    );
    (
        reply.text("HealthCheckIntervalSeconds"),
        reply.text("HealthyThresholdCount"),
        reply.text("UnhealthyThresholdCount"),
    )
}

/// Everything one row needs standing up, so every row's checks run on the one clock instead of
/// one row waiting out the next row's setup.
struct Deployed<'a> {
    row: &'a Row,
    guarding: String,
    target_group: String,
    instance: String,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_configured_schedule_reaches_the_verdict_its_own_checks_decide() {
    let router = emulator_with_ec2();
    let (vpc, subnet) = seed_network(&router).await;

    let mut deployed: Vec<Deployed<'_>> = Vec::new();
    for (index, row) in ROWS.iter().enumerate() {
        let guarding =
            cluster_security_group(&router, &vpc, &format!("hcspan-{}-sg", row.slug)).await;
        let address = format!(
            "172.28.0.{}",
            20 + u16::try_from(index).expect("the row count fits a u16")
        );
        let instance = run_instance(&router, row.case, &subnet, &guarding, &address).await;
        let target_group =
            target_group(&router, row, &format!("hcspan-{}-tg", row.slug), &vpc).await;
        forwarding_balancer(
            &router,
            row,
            &format!("hcspan-{}-nlb", row.slug),
            &subnet,
            &target_group,
        )
        .await;
        deployed.push(Deployed {
            row,
            guarding,
            target_group,
            instance,
        });
    }

    // The instance is registered once its guest can answer at all, so what the checks then find
    // is the firewall and nothing else.
    for entry in &deployed {
        running(&router, entry.row.case, &entry.instance).await;
        register(
            &router,
            entry.row.case,
            &entry.target_group,
            &entry.instance,
        )
        .await;
    }

    // LEG 1: no rule of the instances' own group names the load balancer, so its checks arrive
    // nowhere and every row reaches the verdict its threshold of consecutive failures decides —
    // whatever cadence that row asked for.
    for entry in &deployed {
        let row = entry.row;
        let (verdict, seen) =
            health_reaching(&router, row.case, &entry.target_group, UNHEALTHY).await;
        let verdict = verdict.expect("the verdict asserted by the wait");
        assert_eq!(
            (verdict.reason.as_deref(), verdict.state.as_str()),
            (Some(FAILED_HEALTH_CHECKS), UNHEALTHY),
            "case {:?}: a Network Load Balancer's check that nothing answered is reported \
             {UNHEALTHY}/{FAILED_HEALTH_CHECKS}, got {verdict:?}",
            row.case
        );
        assert!(
            verdict
                .description
                .as_deref()
                .is_some_and(|text| !text.is_empty()),
            "case {:?}: an unhealthy registration carries the detail its state is described \
             by, got {verdict:?}",
            row.case
        );
        // The checks connect to the group's EFFECTIVE health-check port, and the registration
        // is held on the port the traffic goes to.
        assert_eq!(
            (
                verdict.health_check_port.as_deref(),
                verdict.port.as_deref()
            ),
            (Some(row.checked_port), Some(row.traffic_port)),
            "case {:?}: expected the checks on port {} and the registration on port {}, got \
             {verdict:?}",
            row.case,
            row.checked_port,
            row.traffic_port
        );
        // A target no rule admits is answered by nothing from its first check on, so the only
        // state it can have passed through on the way to the verdict is `initial`.
        assert!(
            seen.iter().all(|state| state == INITIAL || state == UNHEALTHY),
            "case {:?}: a target whose checks cannot arrive must never be reported healthy, \
             got the states {seen:?}",
            row.case
        );
    }

    // LEG 2: the remediation — each row's own effective health-check port, admitted from the
    // block the load balancer's nodes sit in, which is the only way a rule can name a Network
    // Load Balancer associated with no security group. The verdict is derived rather than
    // latched, so the same registration returns to `healthy`.
    for entry in &deployed {
        ec2_ok(
            &router,
            entry.row.case,
            "AuthorizeSecurityGroupIngress",
            &[
                p("GroupId", &entry.guarding),
                p("IpPermissions.1.IpProtocol", "tcp"),
                p("IpPermissions.1.FromPort", entry.row.checked_port),
                p("IpPermissions.1.ToPort", entry.row.checked_port),
                p("IpPermissions.1.IpRanges.1.CidrIp", SUBNET_CIDR),
            ],
        )
        .await;
    }
    for entry in &deployed {
        let row = entry.row;
        let (settled, _) =
            health_reaching(&router, row.case, &entry.target_group, HEALTHY).await;
        let settled = settled.expect("the verdict asserted by the wait");
        // "If the target state is healthy, a reason code is not provided."
        assert_eq!(
            (settled.reason.as_deref(), settled.description.as_deref()),
            (None, None),
            "case {:?}: a healthy registration carries no reason and no description, got \
             {settled:?}",
            row.case
        );
        assert_eq!(
            settled.id, entry.instance,
            "case {:?}: expected the healthy registration to be the instance registered, got \
             {settled:?}",
            row.case
        );
        // What the read reports about the SCHEDULE is the member set the create stored,
        // whatever this emulator then spaces the checks by.
        let reported = reported_schedule(&router, row.case, &entry.target_group).await;
        let (healthy, unhealthy) = row.thresholds.unwrap_or(("5", "2"));
        let expected = (
            Some(row.interval.to_string()),
            Some(healthy.to_string()),
            Some(unhealthy.to_string()),
        );
        assert_eq!(
            reported, expected,
            "case {:?}: expected (interval, healthy, unhealthy) {expected:?}, got {reported:?}",
            row.case
        );
    }
}
