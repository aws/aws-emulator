// SPDX-License-Identifier: CC0-1.0

//! Which PORTS a health check of an `instance` target may be judged against, over the wire.
//!
//! The rule asserted here: **a health check of a registered target is failed for the port it
//! connects to only against the ports something behind that registration is POSITIVELY held to
//! be listening on; where the emulator holds no statement about them, the check is not failed
//! for its port. For a registration naming an instance of this account those ports are the
//! servers that instance's guest runs — known for a guest whose own program this emulator
//! performed in full, including the guest given no program at all, which runs nothing. A guest
//! whose program asked for something this emulator performs nothing of may have been asking it
//! to SERVE, so what that machine listens on is not known: the port its registration names and
//! the port its group's `HealthCheckPort` names are both the caller's own word about the same
//! workload, and a reading that holds no port of it contradicts neither. A guest whose ports
//! ARE known is checked against exactly them, so a check pointed at one nothing listens on
//! fails and the registration settles on `unhealthy` with the reason a check nothing answered
//! is reported under, never passing through `healthy`.**
//!
//! `HealthCheckPort` is "the port the load balancer uses when performing health checks on
//! targets" and `TargetDescription$Port` "the port on which the target is listening"
//! (`smithy_query(query="operation",operation="CreateTargetGroup")`,
//! `smithy_query(query="shape",shape="TargetDescription")`); `TargetHealthStateEnum` is
//! `initial|healthy|unhealthy|unhealthy.draining|unused|draining|unavailable`, and
//! `TargetHealth$Reason`'s own documentation gives the `initial` reasons
//! (`Elb.RegistrationInProgress`, `Elb.InitialHealthChecking`) and states that for `healthy`
//! "a reason code is not provided".
//!
//! The rows differ along what the rule generalises over: whether the group names a
//! health-check port of its own at all, whether that port is the one the registration names,
//! whether the instance's guest was given a program of its own, whether this emulator performs
//! every command of that program (a distribution's package and the interpreter's own module) or
//! not (the interpreter handed a program the guest wrote, and a program this emulator has none
//! of), and whether the checked port is one of the ports that guest is held to be listening on.
//!
//! Everything is driven over the wire through the two services' own APIs: EC2 is mounted beside
//! this one on the suite's own engine, so the VPC, the subnets, the security group, its rule and
//! the instances are the records EC2's own readers answer with, each instance's servers are the
//! ones its own user data installed and started, and the verdict is read back through
//! `DescribeTargetHealth`.

use std::time::{Duration, Instant};

use base64::Engine as _;
use smithy_server_runtime::MultiServiceRouter;

use crate::listener_rules::create_listener;
use crate::provisioning_loop::{action, call, first_text, health_block, members, p};
use crate::target_security_group_health_check::{ec2_ok, emulator_with_ec2, reported};

/// How long a row waits for the verdict the group's own health-check cadence reaches, and how
/// often it looks. Generous on purpose — many times the cadence below — because the deadline is
/// only here so a failure is a failure rather than a hang.
const DEADLINE: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_millis(50);

/// The health-check cadence every target group below is created with: a verdict takes a
/// threshold of consecutive checks, each one interval after the last, so a suite that waits for
/// one asks for the shortest interval `HealthCheckIntervalSeconds` admits ("the range is
/// 5-300") and the lowest threshold its counterparts admit ("the range is 2-10").
const INTERVAL_SECONDS: &str = "5";
const THRESHOLD: &str = "2";

/// The path every group's check requests, and the document each serving guest answers it with.
const HEALTH_PATH: &str = "/health";

/// An AMI id a launch names; nothing here asserts anything about the image.
const AMI: &str = "ami-0123456789abcdef0";

/// The network every row runs in: one VPC, and two subnets of it in different zones, which is
/// what an Application Load Balancer is placed across.
const VPC_CIDR: &str = "10.70.0.0/16";
const SUBNETS: [(&str, &str); 2] = [
    ("10.70.1.0/24", "us-west-2a"),
    ("10.70.2.0/24", "us-west-2b"),
];

/// The port every target group receives traffic on, which is the port every registration below
/// names — deliberately NOT a port any guest of this suite listens on, so what decides each row
/// is the health-check port alone.
const TRAFFIC_PORT: i32 = 8081;

/// The user data of a guest whose own program runs a SERVER OF ITS OWN: the interpreter is
/// handed a program the guest wrote rather than a module of its own, which is a program this
/// emulator performs nothing of — the shape of every user data that ships an application.
const OWN_PROGRAM_SERVER: &str = "#!/bin/bash\n\
     mkdir -p /srv/app\n\
     echo ok > /srv/app/health\n\
     echo \"import http.server\" > /srv/app/server.py\n\
     python3 /srv/app/server.py 8080\n";

/// The user data of a guest whose program asks for a server program this emulator has none of
/// at all.
const UNKNOWN_PROGRAM_SERVER: &str = "#!/bin/bash\n\
     mkdir -p /srv/app\n\
     echo ok > /srv/app/health\n\
     socat TCP-LISTEN:8080,fork,reuseaddr EXEC:/srv/app/answer\n";

/// The user data of a guest that installs a distribution's web server, starts its unit and
/// writes the document the health check asks for into the directory that package serves — the
/// shape the EC2 user guide's own web-server tutorial has, every command of which this emulator
/// performs.
const PACKAGE_SERVER: &str = "#!/bin/bash\n\
     yum update -y\n\
     yum install -y httpd\n\
     systemctl start httpd\n\
     echo ok > /var/www/html/health\n";

/// The port that package's own configuration listens on
/// (`service_common::static_web_servers::WEB_SERVER_PACKAGES`).
const PACKAGE_PORT: &str = "80";

/// The user data of a guest that serves a directory of its own with the interpreter's own HTTP
/// server module, on a port of its own — an invocation this emulator performs.
const MODULE_SERVER: &str = "#!/bin/bash\n\
     mkdir -p /srv/site\n\
     echo ok > /srv/site/health\n\
     python3 -m http.server 8080 --directory /srv/site\n";

/// The port those two invocations name.
const MODULE_PORT: &str = "8080";

/// A port no guest of this suite listens on.
const UNSERVED_PORT: &str = "9090";

/// The `(State, Reason)` pair a reachable registration reaches — "if the target state is
/// healthy, a reason code is not provided" — and the one a registration nothing answers for
/// settles on.
const HEALTHY: (&str, &str) = ("healthy", "");
const UNHEALTHY: (&str, &str) = ("unhealthy", "Target.Timeout");

/// The `TargetHealthReasonEnum` values `TargetHealth$Reason` documents for `initial`: "if the
/// target state is initial, the reason code can be Elb.RegistrationInProgress or
/// Elb.InitialHealthChecking".
const INITIAL_REASONS: [&str; 2] = ["Elb.RegistrationInProgress", "Elb.InitialHealthChecking"];

/// One row: the instance behind the registration, the health-check port its group names, and
/// the verdict that implies.
struct Row {
    case: &'static str,
    /// The target group's name, which also names the row's instance in nothing but its address.
    name: &'static str,
    /// The user data of the row's instance, or `None` for an instance given no program of its
    /// own — a guest this emulator knows runs nothing at all.
    user_data: Option<&'static str>,
    /// The `HealthCheckPort` the group is created with, or `None` for a group that leaves the
    /// check on the port the registration names.
    health_check_port: Option<&'static str>,
    /// The `(State, Reason)` the registration must reach.
    reaches: (&'static str, &'static str),
}

/// The rows, in creation order.
const ROWS: [Row; 8] = [
    Row {
        case: "the recorded shape: a guest running a server program of its own, checked on a \
               health-check port of its own",
        name: "hcport-ownprog",
        user_data: Some(OWN_PROGRAM_SERVER),
        health_check_port: Some(MODULE_PORT),
        reaches: HEALTHY,
    },
    Row {
        case: "a guest whose program asks for a server this emulator has none of",
        name: "hcport-unknown",
        user_data: Some(UNKNOWN_PROGRAM_SERVER),
        health_check_port: Some(MODULE_PORT),
        reaches: HEALTHY,
    },
    Row {
        case: "a guest given no program of its own, checked on a health-check port of its own",
        name: "hcport-noserver",
        user_data: None,
        health_check_port: Some(MODULE_PORT),
        reaches: UNHEALTHY,
    },
    Row {
        case: "the same instance checked on the port its registration names",
        name: "hcport-traffic",
        user_data: None,
        health_check_port: None,
        reaches: HEALTHY,
    },
    Row {
        case: "an instance whose guest serves the health-check port its package listens on",
        name: "hcport-package",
        user_data: Some(PACKAGE_SERVER),
        health_check_port: Some(PACKAGE_PORT),
        reaches: HEALTHY,
    },
    Row {
        case: "an instance whose guest serves a health-check port of its own through the \
               interpreter's module",
        name: "hcport-module",
        user_data: Some(MODULE_SERVER),
        health_check_port: Some(MODULE_PORT),
        reaches: HEALTHY,
    },
    Row {
        case: "an instance whose guest runs a server, checked on a port it does not listen on",
        name: "hcport-unserved",
        user_data: Some(PACKAGE_SERVER),
        health_check_port: Some(UNSERVED_PORT),
        reaches: UNHEALTHY,
    },
    Row {
        case: "an instance whose guest runs a server on a port of its own, checked on the \
               package port nothing listens on",
        name: "hcport-othersrv",
        user_data: Some(MODULE_SERVER),
        health_check_port: Some(PACKAGE_PORT),
        reaches: UNHEALTHY,
    },
];

/// The account's network: the VPC, its subnets, the group the load balancer is associated with
/// and the group governing every instance.
struct Network {
    vpc: String,
    subnets: Vec<String>,
    load_balancer_group: String,
    target_group: String,
}

/// Create that network through EC2's own API, with the one inbound rule that admits the load
/// balancer to every instance on every protocol and port — so no row's verdict is decided by
/// the firewall (`all/target_security_group_health_check.rs` is the suite about that).
async fn seed_network(router: &MultiServiceRouter) -> Network {
    let created = ec2_ok(router, "setup", "CreateVpc", &[p("CidrBlock", VPC_CIDR)]).await;
    let vpc = reported("setup", &created, "vpcId");
    let gateway = ec2_ok(router, "setup", "CreateInternetGateway", &[]).await;
    let gateway = reported("setup", &gateway, "internetGatewayId");
    ec2_ok(
        router,
        "setup",
        "AttachInternetGateway",
        &[p("InternetGatewayId", &gateway), p("VpcId", &vpc)],
    )
    .await;
    let mut subnets: Vec<String> = Vec::new();
    for (cidr, zone) in SUBNETS {
        let created = ec2_ok(
            router,
            "setup",
            "CreateSubnet",
            &[
                p("VpcId", &vpc),
                p("CidrBlock", cidr),
                p("AvailabilityZone", zone),
            ],
        )
        .await;
        subnets.push(reported("setup", &created, "subnetId"));
    }
    let load_balancer_group = security_group(router, &vpc, "hcport-alb").await;
    let target_group = security_group(router, &vpc, "hcport-targets").await;
    ec2_ok(
        router,
        "setup",
        "AuthorizeSecurityGroupIngress",
        &[
            p("GroupId", &target_group),
            // "-1" names every protocol, which a rule that admits a source outright carries.
            p("IpPermissions.1.IpProtocol", "-1"),
            p("IpPermissions.1.Groups.1.GroupId", &load_balancer_group),
        ],
    )
    .await;
    Network {
        vpc,
        subnets,
        load_balancer_group,
        target_group,
    }
}

/// Create one security group of `vpc` and answer its `GroupId`.
async fn security_group(router: &MultiServiceRouter, vpc: &str, name: &str) -> String {
    let created = ec2_ok(
        router,
        "setup",
        "CreateSecurityGroup",
        &[
            p("VpcId", vpc),
            p("GroupName", name),
            p("GroupDescription", "a group of this suite"),
        ],
    )
    .await;
    reported("setup", &created, "groupId")
}

/// Launch one instance of `network` at `address`, with `user_data` as its user data, and answer
/// its id.
async fn run_instance(
    router: &MultiServiceRouter,
    case: &str,
    network: &Network,
    address: &str,
    user_data: Option<&str>,
) -> String {
    let mut params = vec![
        p("ImageId", AMI),
        p("MinCount", "1"),
        p("MaxCount", "1"),
        p("SubnetId", &network.subnets[0]),
        p("SecurityGroupId.1", &network.target_group),
        p("PrivateIpAddress", address),
    ];
    if let Some(user_data) = user_data {
        params.push(p(
            "UserData",
            &base64::engine::general_purpose::STANDARD.encode(user_data),
        ));
    }
    let launched = ec2_ok(router, case, "RunInstances", &params).await;
    reported(case, &launched, "instanceId")
}

/// Look at the instance until EC2 reports it `running` — the move the launch owes, which is
/// also when its guest comes up and runs its user data.
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
        let is_running = described.contains("<name>running</name>");
        if is_running || Instant::now() >= deadline {
            assert!(
                is_running,
                "case {case:?}: expected EC2 to report {instance} running, got {described}"
            );
            return;
        }
        ::tokio::time::sleep(POLL).await;
    }
}

/// Create the load balancer every row's listener belongs to.
async fn create_alb(router: &MultiServiceRouter, name: &str, network: &Network) -> String {
    let mut form = action("CreateLoadBalancer");
    form.push(p("Name", name));
    for (index, subnet) in network.subnets.iter().enumerate() {
        form.push(p(&format!("Subnets.member.{}", index + 1), subnet));
    }
    form.push(p("SecurityGroups.member.1", &network.load_balancer_group));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "setup {name}: expected 200 from CreateLoadBalancer, got {} body {}",
        reply.status, reply.body
    );
    reply
        .text("LoadBalancerArn")
        .unwrap_or_else(|| panic!("setup {name}: no LoadBalancerArn in {}", reply.body))
}

/// Create the `instance` target group of one row — an HTTP group checked at this suite's own
/// cadence on [`HEALTH_PATH`] — and answer its ARN.
async fn target_group(router: &MultiServiceRouter, row: &Row, vpc: &str) -> String {
    let mut form = action("CreateTargetGroup");
    form.push(p("Name", row.name));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", &TRAFFIC_PORT.to_string()));
    form.push(p("TargetType", "instance"));
    form.push(p("VpcId", vpc));
    form.push(p("HealthCheckProtocol", "HTTP"));
    form.push(p("HealthCheckPath", HEALTH_PATH));
    form.push(p("HealthCheckIntervalSeconds", INTERVAL_SECONDS));
    form.push(p("HealthyThresholdCount", THRESHOLD));
    form.push(p("UnhealthyThresholdCount", THRESHOLD));
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

/// Register `instance` on the traffic port with the target group `arn` names.
async fn register(router: &MultiServiceRouter, case: &str, arn: &str, instance: &str) {
    let mut form = action("RegisterTargets");
    form.push(p("TargetGroupArn", arn));
    form.push(p("Targets.member.1.Id", instance));
    form.push(p("Targets.member.1.Port", &TRAFFIC_PORT.to_string()));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from RegisterTargets, got {} body {}",
        reply.status, reply.body
    );
}

/// `(State, Reason, Description)` for every registration the target group `arn` reports.
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

/// The health of `arn`'s registrations once every one of them reports `wanted`, and every state
/// reported on the way there — so a row can assert what it must never have reported.
async fn health_reaching(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
    wanted: &str,
) -> (Vec<(String, String, String)>, Vec<String>) {
    let deadline = Instant::now() + DEADLINE;
    let mut seen: Vec<String> = Vec::new();
    loop {
        let found = health(router, case, arn).await;
        for (state, _, _) in &found {
            if seen.last().map(String::as_str) != Some(state.as_str()) {
                seen.push(state.clone());
            }
        }
        let settled = !found.is_empty() && found.iter().all(|(state, _, _)| state == wanted);
        if settled || Instant::now() >= deadline {
            assert!(
                settled,
                "case {case:?}: expected every registration to reach {wanted}, got {found:?} \
                 (states seen: {seen:?})"
            );
            return (found, seen);
        }
        ::tokio::time::sleep(POLL).await;
    }
}

/// RULE: a check is failed for the port it connects to only against ports something behind the
/// registration is positively held to be listening on; an instance whose guest this emulator
/// holds no server for has none held, so neither the port its registration names nor the port
/// its group's `HealthCheckPort` names is contradicted.
#[tokio::test]
async fn an_instance_target_is_failed_for_its_health_check_port_only_against_held_ports() {
    let router = emulator_with_ec2();
    let network = seed_network(&router).await;
    let load_balancer = create_alb(&router, "hcport-alb", &network).await;

    // One instance per row, all launched before anything waits, so the span each owes its move
    // to `running` runs at the same time.
    let mut instances: Vec<String> = Vec::new();
    for (index, row) in ROWS.iter().enumerate() {
        let address = format!(
            "10.70.1.{}",
            20 + u16::try_from(index).expect("the row count fits a u16")
        );
        instances.push(run_instance(&router, row.case, &network, &address, row.user_data).await);
    }
    for (row, instance) in ROWS.iter().zip(&instances) {
        running(&router, row.case, instance).await;
    }

    // One target group per row, each with a listener forwarding to it — which is what makes the
    // group one the load balancer checks — and the row's own instance registered with it. Every
    // registration is made before anything is polled, so the rows' cadences run together.
    let mut groups: Vec<String> = Vec::new();
    for (index, (row, instance)) in ROWS.iter().zip(&instances).enumerate() {
        let group = target_group(&router, row, &network.vpc).await;
        create_listener(
            &router,
            &load_balancer,
            &group,
            80 + i32::try_from(index).expect("the row count fits an i32"),
        )
        .await;
        register(&router, row.case, &group, instance).await;
        // A registration whose checks have decided nothing yet is `initial`, under one of the
        // two reason codes the model documents for that state — read here, before any verdict
        // could have been reached, because the earliest one is a whole configured interval away.
        let reported = health(&router, row.case, &group).await;
        for (state, reason, _) in &reported {
            assert_eq!(
                state, "initial",
                "case {:?}: expected a just-registered target to be initial, got {reported:?}",
                row.case
            );
            assert!(
                INITIAL_REASONS.contains(&reason.as_str()),
                "case {:?}: expected one of {INITIAL_REASONS:?} for an initial target, got \
                 {reason:?}",
                row.case
            );
        }
        groups.push(group);
    }

    for (row, group) in ROWS.iter().zip(&groups) {
        let (found, seen) = health_reaching(&router, row.case, group, row.reaches.0).await;
        assert_eq!(
            found.len(),
            1,
            "case {:?}: expected one description per registration, got {found:?}",
            row.case
        );
        for (state, reason, description) in &found {
            assert_eq!(
                (state.as_str(), reason.as_str()),
                row.reaches,
                "case {:?}: expected {}/{}, got {state}/{reason}",
                row.case,
                row.reaches.0,
                row.reaches.1
            );
            if row.reaches.0 == HEALTHY.0 {
                // "If the target state is healthy, a reason code is not provided", and no
                // description either.
                assert!(
                    reason.is_empty() && description.is_empty(),
                    "case {:?}: a healthy target carries no reason and no description, got \
                     {reason:?} / {description:?}",
                    row.case
                );
            } else {
                assert!(
                    !description.is_empty(),
                    "case {:?}: a non-healthy verdict carries a description, got {found:?}",
                    row.case
                );
            }
        }
        if row.reaches.0 == HEALTHY.0 {
            // A target whose checks are answered is never reported unhealthy on the way to its
            // verdict: it leaves `initial` for `healthy` and nothing else.
            assert!(
                seen.iter().all(|state| state == "initial" || state == HEALTHY.0),
                "case {:?}: expected only initial and healthy on the way to the verdict, got \
                 {seen:?}",
                row.case
            );
        } else {
            // And one nothing answers for is never reported healthy on the way to its verdict.
            assert!(
                !seen.iter().any(|state| state == HEALTHY.0),
                "case {:?}: a target whose checked port nothing listens on must never be \
                 reported healthy, got the states {seen:?}",
                row.case
            );
        }
    }
}
