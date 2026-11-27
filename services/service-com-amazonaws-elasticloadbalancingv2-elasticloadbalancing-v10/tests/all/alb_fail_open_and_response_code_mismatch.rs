// SPDX-License-Identifier: CC0-1.0

//! What an Application Load Balancer's health check MAKES OF THE ANSWER it gets, and what the
//! load balancer does with a target group whose targets are all unhealthy or all gone.
//!
//! Two rules are asserted here, over the same deployment:
//!
//! 1. **A health check of an HTTP/HTTPS target group is a request for `HealthCheckPath`, and it
//!    passes only when the status the target answers it with is one the group's `Matcher`
//!    counts as a success.** `HealthCheckPath` is "[HTTP/HTTPS health checks] The destination
//!    for health checks on the targets" and `Matcher` "[HTTP/HTTPS health checks] The HTTP or
//!    gRPC codes to use when checking for a successful response from a target"
//!    (`smithy_query(query="doc",shape="CreateTargetGroupInput",member="Matcher")`), so a
//!    target answering that path outside those codes fails every check and reaches the
//!    `unhealthy` its `UnhealthyThresholdCount` takes — reported with the model's own reason
//!    for it, "`Target.ResponseCodeMismatch` - The health checks did not return an expected
//!    HTTP code" (`TargetHealth$Reason`), described by the status those checks really got back.
//!    A target answering INSIDE them is `healthy` with no reason at all ("if the target state
//!    is `healthy`, a reason code is not provided").
//! 2. **A `forward` sends the request to a target the group holds: to one the load balancer may
//!    send requests to where it holds such a target, and across its UNHEALTHY ones where it
//!    holds none — so only a group with no target to send the request to earns the load
//!    balancer's own `503`.** That is the fail-open the target-group health-check documentation
//!    states, and the distinction a client reads a 503 as "nothing registered" by.
//!
//! The rows of rule 1 differ along what it generalises over: the status the target answers the
//! checked path with (the document it holds, another document of the same root, a path it holds
//! nothing at) and the codes the matcher names (the default one code, several, a range covering
//! the answered status, and a code set the target's own `200` is outside of).
//!
//! Everything is driven through the two services' own APIs — EC2 is mounted beside this one on
//! the suite's own engine, so the network, the security group and the instance are the records
//! EC2's own readers answer with, and the instance's web server is the one its own user data
//! installed and started. The requests to the load balancer carry no AWS credential, exactly as
//! a client of the published `DNSName` sends them.

use std::time::{Duration, Instant};

use base64::Engine as _;
use smithy_server_runtime::MultiServiceRouter;

use crate::alb_data_plane::send;
use crate::provisioning_loop::{action, call, first_text, health_block, members, p};
use crate::target_security_group_health_check::{ec2_ok, emulator_with_ec2, reported};

/// How long a step waits for a clock-owed move — the instance's to `running`, the load
/// balancer's to `active`, a registration's to the verdict its checks reach — and how often it
/// looks. Generous on purpose: the deadline is only here so a failure is a failure rather than
/// a hang.
const DEADLINE: Duration = Duration::from_secs(120);
const POLL: Duration = Duration::from_millis(25);

/// The health-check cadence every target group below is created with: a verdict takes a
/// threshold of consecutive checks, each one interval after the last, so a suite that waits for
/// one asks for the shortest interval `HealthCheckIntervalSeconds` admits ("the range is
/// 5-300") and the fewest checks a threshold may ask for ("the range is 2-10").
const INTERVAL_SECONDS: &str = "5";
const THRESHOLD: &str = "2";

/// An AMI id the launch names; nothing here asserts anything about the image.
const AMI: &str = "ami-0123456789abcdef0";

/// The network this suite runs in: one VPC and two subnets of it in different zones, which is
/// what an Application Load Balancer is placed across.
const VPC_CIDR: &str = "10.70.0.0/16";
const SUBNETS: [(&str, &str); 2] = [("10.70.1.0/24", "us-west-2a"), ("10.70.2.0/24", "us-west-2b")];

/// The address the serving instance is launched at, and the port its own server listens on —
/// which is the port the target groups receive traffic on and the port their checks connect to.
const SERVING_ADDRESS: &str = "10.70.1.10";
const TRAFFIC_PORT: i32 = 80;

/// The marker the instance's own user-data program writes into the document it serves, so a
/// body anything else produced fails an assertion.
const MARKER: &str = "fail-open-served-body";

/// The paths the instance's own server answers, and the one its document root holds nothing at.
const ROOT_PATH: &str = "/";
const SECOND_DOCUMENT_PATH: &str = "/health.txt";
const MISSING_PATH: &str = "/not-deployed";

/// The statuses a static server answers those paths with: the document it holds, and the status
/// for a path its root holds nothing at.
const SERVED_STATUS: u16 = 200;
const NOT_FOUND_STATUS: u16 = 404;

/// The status the load balancer itself answers a `forward` to a target group it has no target to
/// send the request to.
const NO_TARGET_STATUS: u16 = 503;

/// The `TargetHealth$State` values this suite reads back.
const HEALTHY: &str = "healthy";
const UNHEALTHY: &str = "unhealthy";

/// The `TargetHealthReasonEnum` value an answer the matcher rejects is reported under.
const RESPONSE_CODE_MISMATCH: &str = "Target.ResponseCodeMismatch";

/// The user data of the serving instance: it installs the Apache package of the Amazon Linux
/// family, starts its unit and writes the two documents this suite asks for into the directory
/// that package serves.
fn serving_user_data() -> String {
    format!(
        "#!/bin/bash\n\
         yum install -y httpd\n\
         systemctl enable httpd\n\
         systemctl start httpd\n\
         echo {MARKER} > /var/www/html/index.html\n\
         echo {MARKER} > /var/www/html/health.txt\n"
    )
}

/// The account's network, as the two services read it.
struct Network {
    vpc: String,
    subnets: Vec<String>,
    /// The one group the load balancers are associated with and the instance is governed by,
    /// whose inbound rule admits the traffic port from every address — so no row here is
    /// answered by the firewall.
    security_group: String,
}

/// Create that network through EC2's own API: the VPC with the internet gateway an
/// internet-facing load balancer needs, the two subnets, and the security group with the one
/// inbound rule that admits the load balancer to the target on the traffic port.
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
    let created = ec2_ok(
        router,
        "setup",
        "CreateSecurityGroup",
        &[
            p("VpcId", &vpc),
            p("GroupName", "failopen-group"),
            p("GroupDescription", "a group of this suite"),
        ],
    )
    .await;
    let security_group = reported("setup", &created, "groupId");
    ec2_ok(
        router,
        "setup",
        "AuthorizeSecurityGroupIngress",
        &[
            p("GroupId", &security_group),
            p("IpPermissions.1.IpProtocol", "tcp"),
            p("IpPermissions.1.FromPort", &TRAFFIC_PORT.to_string()),
            p("IpPermissions.1.ToPort", &TRAFFIC_PORT.to_string()),
            p("IpPermissions.1.IpRanges.1.CidrIp", "0.0.0.0/0"),
        ],
    )
    .await;
    Network {
        vpc,
        subnets,
        security_group,
    }
}

/// Launch the serving instance of `network` and answer its id.
async fn run_serving_instance(router: &MultiServiceRouter, network: &Network) -> String {
    let launched = ec2_ok(
        router,
        "setup",
        "RunInstances",
        &[
            p("ImageId", AMI),
            p("MinCount", "1"),
            p("MaxCount", "1"),
            p("SubnetId", &network.subnets[0]),
            p("SecurityGroupId.1", &network.security_group),
            p("PrivateIpAddress", SERVING_ADDRESS),
            p(
                "UserData",
                &base64::engine::general_purpose::STANDARD.encode(serving_user_data()),
            ),
        ],
    )
    .await;
    reported("setup", &launched, "instanceId")
}

/// Look at the instance until EC2 reports it `running` — the move the launch owes, which is also
/// when its guest comes up and runs its user data.
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

/// Create an `ip` target group checking `path` against `matcher` — the member left out where the
/// row names none — and answer its ARN.
async fn target_group(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    network: &Network,
    path: &str,
    matcher: Option<&str>,
) -> String {
    let mut form = action("CreateTargetGroup");
    form.push(p("Name", name));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", &TRAFFIC_PORT.to_string()));
    form.push(p("TargetType", "ip"));
    form.push(p("VpcId", &network.vpc));
    form.push(p("HealthCheckProtocol", "HTTP"));
    form.push(p("HealthCheckPath", path));
    form.push(p("HealthCheckIntervalSeconds", INTERVAL_SECONDS));
    form.push(p("HealthyThresholdCount", THRESHOLD));
    form.push(p("UnhealthyThresholdCount", THRESHOLD));
    if let Some(matcher) = matcher {
        form.push(p("Matcher.HttpCode", matcher));
    }
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from CreateTargetGroup, got {} body {}",
        reply.status, reply.body
    );
    reply
        .text("TargetGroupArn")
        .unwrap_or_else(|| panic!("case {case:?}: no TargetGroupArn in {}", reply.body))
}

/// Register the serving address with the target group `arn` names.
async fn register(router: &MultiServiceRouter, case: &str, arn: &str) {
    let mut form = action("RegisterTargets");
    form.push(p("TargetGroupArn", arn));
    form.push(p("Targets.member.1.Id", SERVING_ADDRESS));
    form.push(p("Targets.member.1.Port", &TRAFFIC_PORT.to_string()));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from RegisterTargets, got {} body {}",
        reply.status, reply.body
    );
}

/// Create an Application Load Balancer across the network's subnets whose HTTP listener on
/// `port` forwards to `tg_arn`, and answer its ARN and the name it published.
async fn load_balancer(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    network: &Network,
    port: i32,
    tg_arn: &str,
) -> (String, String) {
    let mut form = action("CreateLoadBalancer");
    form.push(p("Name", name));
    for (index, subnet) in network.subnets.iter().enumerate() {
        form.push(p(&format!("Subnets.member.{}", index + 1), subnet));
    }
    form.push(p("SecurityGroups.member.1", &network.security_group));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from CreateLoadBalancer, got {} body {}",
        reply.status, reply.body
    );
    let arn = reply
        .text("LoadBalancerArn")
        .unwrap_or_else(|| panic!("case {case:?}: no LoadBalancerArn in {}", reply.body));
    let dns_name = reply
        .text("DNSName")
        .unwrap_or_else(|| panic!("case {case:?}: no DNSName in {}", reply.body));
    let mut form = action("CreateListener");
    form.push(p("LoadBalancerArn", &arn));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", &port.to_string()));
    form.push(p("DefaultActions.member.1.Type", "forward"));
    form.push(p("DefaultActions.member.1.TargetGroupArn", tg_arn));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from CreateListener, got {} body {}",
        reply.status, reply.body
    );
    (arn, dns_name)
}

/// Look at the load balancer until it reports `State.Code` `active`, which is when its listeners
/// serve traffic on the name it published.
async fn active(router: &MultiServiceRouter, case: &str, arn: &str) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let mut form = action("DescribeLoadBalancers");
        form.push(p("LoadBalancerArns.member.1", arn));
        let reply = call(router, &form).await;
        let code = first_text(&reply.body, "Code").unwrap_or_default();
        if code == "active" || Instant::now() >= deadline {
            assert_eq!(
                code, "active",
                "case {case:?}: expected the load balancer to reach active, got {code:?} in {}",
                reply.body
            );
            return;
        }
        ::tokio::time::sleep(POLL).await;
    }
}

/// What `DescribeTargetHealth` reports for one registration: the state, the reason and the
/// description, each read as empty where the answer carries none.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    state: String,
    reason: String,
    description: String,
}

/// Every registration the target group `arn` holds, as it is reported.
async fn described(router: &MultiServiceRouter, arn: &str) -> Vec<Reported> {
    let mut form = action("DescribeTargetHealth");
    form.push(p("TargetGroupArn", arn));
    let reply = call(router, &form).await;
    members(&reply.body, "TargetHealthDescriptions")
        .iter()
        .map(|description| {
            // The health's own element, because the override beside it carries members of
            // the same names ([`health_block`]).
            let health = health_block(description);
            Reported {
                state: first_text(&health, "State").unwrap_or_default(),
                reason: first_text(&health, "Reason").unwrap_or_default(),
                description: first_text(&health, "Description").unwrap_or_default(),
            }
        })
        .collect()
}

/// Look at the target group until its one registration is reported `wanted`, and assert that it
/// is — the whole triple, so a state reached with another reason fails the case.
async fn health_reaching(router: &MultiServiceRouter, case: &str, arn: &str, wanted: &Reported) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let reported = described(router, arn).await;
        let settled = reported.len() == 1 && &reported[0] == wanted;
        if settled || Instant::now() >= deadline {
            assert_eq!(
                reported,
                vec![wanted.clone()],
                "case {case:?}: expected the registration reported {wanted:?}, got {reported:?}"
            );
            return;
        }
        ::tokio::time::sleep(POLL).await;
    }
}

/// One deployment: the router, the account's network and the serving instance's id, with the
/// instance already running its own server.
struct Deployment {
    router: MultiServiceRouter,
    network: Network,
}

/// Everything the rows are answered out of, written through the two services' own APIs.
async fn deployment() -> Deployment {
    let router = emulator_with_ec2();
    let network = seed_network(&router).await;
    let instance = run_serving_instance(&router, &network).await;
    running(&router, "the serving instance", &instance).await;
    Deployment { router, network }
}

/// One row of rule 1: the path the group's health check requests, the codes its matcher names,
/// and the health that check then reaches.
struct Row {
    case: &'static str,
    group_name: &'static str,
    listener_port: i32,
    path: &'static str,
    matcher: Option<&'static str>,
    expected: Reported,
}

/// The health a target answering INSIDE the matcher is reported with: the state alone, because
/// "if the target state is `healthy`, a reason code is not provided".
fn healthy() -> Reported {
    Reported {
        state: HEALTHY.to_string(),
        reason: String::new(),
        description: String::new(),
    }
}

/// The health a target answering `status` OUTSIDE the matcher is reported with: the model's own
/// reason for an answer the codes reject, described by the status the checks got back.
fn mismatched(status: u16) -> Reported {
    Reported {
        state: UNHEALTHY.to_string(),
        reason: RESPONSE_CODE_MISMATCH.to_string(),
        description: format!("Health checks failed with these codes: [{status}]"),
    }
}

/// RULE 1: a health check requests `HealthCheckPath` and passes only when the status the target
/// answers it with is one the group's `Matcher` names.
#[tokio::test]
async fn a_health_check_passes_only_on_a_status_its_matcher_names() {
    let deployed = deployment().await;
    let rows: Vec<Row> = vec![
        Row {
            case: "the document the target's root holds, against the default one code",
            group_name: "failopen-root",
            listener_port: 8090,
            path: ROOT_PATH,
            matcher: None,
            expected: healthy(),
        },
        Row {
            case: "another document of the same root, against that code written out",
            group_name: "failopen-second",
            listener_port: 8091,
            path: SECOND_DOCUMENT_PATH,
            matcher: Some("200"),
            expected: healthy(),
        },
        Row {
            case: "a path the target's root holds nothing at, against the default one code",
            group_name: "failopen-missing",
            listener_port: 8092,
            path: MISSING_PATH,
            matcher: None,
            expected: mismatched(NOT_FOUND_STATUS),
        },
        Row {
            case: "the same path against a range that covers the status it answers",
            group_name: "failopen-range",
            listener_port: 8093,
            path: MISSING_PATH,
            matcher: Some("200-499"),
            expected: healthy(),
        },
        Row {
            case: "the same path against the one code it answers",
            group_name: "failopen-exact",
            listener_port: 8094,
            path: MISSING_PATH,
            matcher: Some("404"),
            expected: healthy(),
        },
        Row {
            case: "the served document against codes its own status is outside of",
            group_name: "failopen-outside",
            listener_port: 8095,
            path: ROOT_PATH,
            matcher: Some("201,202"),
            expected: mismatched(SERVED_STATUS),
        },
        Row {
            case: "the served document against several codes, one of them its own",
            group_name: "failopen-several",
            listener_port: 8096,
            path: ROOT_PATH,
            matcher: Some("200,202"),
            expected: healthy(),
        },
    ];

    // Every group is created, registered and routed before anything waits, so the checks of all
    // of them run on their own cadence at the same time.
    let mut arns: Vec<String> = Vec::new();
    for row in &rows {
        let arn = target_group(
            &deployed.router,
            row.case,
            row.group_name,
            &deployed.network,
            row.path,
            row.matcher,
        )
        .await;
        register(&deployed.router, row.case, &arn).await;
        load_balancer(
            &deployed.router,
            row.case,
            &format!("{}-lb", row.group_name),
            &deployed.network,
            row.listener_port,
            &arn,
        )
        .await;
        arns.push(arn);
    }
    for (row, arn) in rows.iter().zip(&arns) {
        health_reaching(&deployed.router, row.case, arn, &row.expected).await;
    }
}

/// RULE 2: the load balancer fails open to the unhealthy target its group holds, answers its own
/// 503 once the group holds no target at all, and serves the target again once its checks pass.
#[tokio::test]
async fn an_alb_fails_open_to_an_unhealthy_target_and_answers_503_for_an_empty_group() {
    let deployed = deployment().await;
    let case = "the only target unhealthy, then deregistered, then checked on a served path";
    // The group checks a path the target holds nothing at, so its only target settles unhealthy
    // with the mismatch its answer is.
    let arn = target_group(
        &deployed.router,
        case,
        "failopen-plane",
        &deployed.network,
        MISSING_PATH,
        None,
    )
    .await;
    register(&deployed.router, case, &arn).await;
    let (lb_arn, dns_name) = load_balancer(
        &deployed.router,
        case,
        "failopen-plane-lb",
        &deployed.network,
        TRAFFIC_PORT,
        &arn,
    )
    .await;
    active(&deployed.router, case, &lb_arn).await;
    health_reaching(
        &deployed.router,
        case,
        &arn,
        &mismatched(NOT_FOUND_STATUS),
    )
    .await;

    // FAIL OPEN: the group holds no healthy target, so the request goes to the unhealthy one
    // anyway and the client is answered with the TARGET's own response.
    let answered = send(&deployed.router, "GET", &dns_name, ROOT_PATH, &[]).await;
    assert_eq!(
        answered.status, SERVED_STATUS,
        "case {case:?}: expected the unhealthy target's own {SERVED_STATUS}, got {} body {:?}",
        answered.status, answered.body
    );
    assert!(
        answered.body.contains(MARKER),
        "case {case:?}: expected the target's own document, got {:?}",
        answered.body
    );

    // EMPTY GROUP: the drain is configured to take no time, so the deregistration takes the one
    // registration out at once and the group has no target to send a request to.
    let mut form = action("ModifyTargetGroupAttributes");
    form.push(p("TargetGroupArn", &arn));
    form.push(p(
        "Attributes.member.1.Key",
        "deregistration_delay.timeout_seconds",
    ));
    form.push(p("Attributes.member.1.Value", "0"));
    let reply = call(&deployed.router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from ModifyTargetGroupAttributes, got {} body {}",
        reply.status, reply.body
    );
    let mut form = action("DeregisterTargets");
    form.push(p("TargetGroupArn", &arn));
    form.push(p("Targets.member.1.Id", SERVING_ADDRESS));
    form.push(p("Targets.member.1.Port", &TRAFFIC_PORT.to_string()));
    let reply = call(&deployed.router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from DeregisterTargets, got {} body {}",
        reply.status, reply.body
    );
    let emptied = described(&deployed.router, &arn).await;
    assert!(
        emptied.is_empty(),
        "case {case:?}: expected the group to hold no registration, got {emptied:?}"
    );
    let answered = send(&deployed.router, "GET", &dns_name, ROOT_PATH, &[]).await;
    assert_eq!(
        answered.status, NO_TARGET_STATUS,
        "case {case:?}: expected the load balancer's own {NO_TARGET_STATUS} for a group with no \
         target, got {} body {:?}",
        answered.status, answered.body
    );
    assert!(
        !answered.body.contains(MARKER),
        "case {case:?}: the 503 is the load balancer's own, so it carries no document of the \
         target, got {:?}",
        answered.body
    );

    // RESTORED: the target is registered again and the check is pointed at a path it serves, so
    // its checks pass, it is reported healthy, and the requests it answers stay the target's.
    register(&deployed.router, case, &arn).await;
    let mut form = action("ModifyTargetGroup");
    form.push(p("TargetGroupArn", &arn));
    form.push(p("HealthCheckPath", ROOT_PATH));
    let reply = call(&deployed.router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from ModifyTargetGroup, got {} body {}",
        reply.status, reply.body
    );
    health_reaching(&deployed.router, case, &arn, &healthy()).await;
    let answered = send(&deployed.router, "GET", &dns_name, SECOND_DOCUMENT_PATH, &[]).await;
    assert_eq!(
        answered.status, SERVED_STATUS,
        "case {case:?}: expected the healthy target's own {SERVED_STATUS}, got {} body {:?}",
        answered.status, answered.body
    );
    assert!(
        answered.body.contains(MARKER),
        "case {case:?}: expected the target's own document, got {:?}",
        answered.body
    );
}
