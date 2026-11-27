// SPDX-License-Identifier: CC0-1.0

//! The whole diagnostic loop a client runs against an Auto Scaling group behind an internal
//! Network Load Balancer whose instances' security group admits NOTHING from the load
//! balancer: the verdict the target group's own health checks reach, and the verdict they reach
//! once the block the load balancer's nodes sit in is admitted — over the three services that
//! own the links of it (the instance, its guest and its security group are Amazon EC2's, the
//! registration and the verdict are Elastic Load Balancing v2's, and the member the
//! registration stands for is Auto Scaling's).
//!
//! The rule under test: **a registration reaches the verdict its own health checks decide — and
//! reaches it WHATEVER health-check schedule the target group carries, because the run of
//! consecutive checks a verdict takes is spaced to fit inside the span this emulator keeps a
//! verdict observable in.** So a client that polls `DescribeTargetHealth` reads the documented
//! `unhealthy` / `Target.FailedHealthChecks` a Network Load Balancer reports a check nothing
//! answered under, on the target group's own traffic port and against its own effective
//! health-check port; and once `AuthorizeSecurityGroupIngress` admits that port from the
//! subnet's block the SAME registration reads `healthy` with no reason at all, while the group
//! reports its member `InService` / `Healthy`.
//!
//! Both rows carry a schedule whose verdict is MANY checks away — the recorded configuration
//! asks for a twenty-second cadence and ten consecutive failures, which on AWS is minutes of
//! checking — so what the rows span is what the rule generalises over: how long the configured
//! cadence is, how many consecutive checks the thresholds make the verdict's run, and whether
//! the checks go to a health-check port of its own over HTTP or to the port the traffic goes to.
//!
//! What this adds to the suites beside it: `asg_registered_target_health_verdicts.rs` asserts
//! WHICH verdict the checks reach for a group checked at the shortest cadence the member admits,
//! and `asg_unreachable_instance_replacement_until_the_subnet_is_admitted.rs` asserts the
//! replacement an `ELB`-health-checked group makes while its instances answer nothing. Here the
//! same loop is driven over a schedule a client configured for AWS's own pace, which is the
//! configuration a client diagnosing an unreachable target really has.
//!
//! Every clock-owed move (the launch, the registration, the guest's boot, the run of checks a
//! verdict takes) is POLLED for until it holds, never slept for, and the router is assembled on
//! a sized thread exactly as `elbv2_network_resolution.rs` does — whose request plumbing this
//! suite reuses rather than restating.

use std::sync::Arc;
use std::time::{Duration, Instant};

use aws_emulator_router::{build_router, InMemoryEngine};
use smithy_server_runtime::MultiServiceRouter;

use crate::elbv2_network_resolution::{ec2_ok, elbv2, form, send, Answer, BOOT_STACK_SIZE};
use crate::query_wire::{block_of, element, members_of, opt_element};

/// The API version an awsQuery request naming the Auto Scaling model carries.
const ASG_VERSION: &str = "2011-01-01";

/// The AMI and instance type every launch template here names; this suite asserts nothing
/// about either.
const IMAGE_ID: &str = "ami-0123456789abcdef0";
const INSTANCE_TYPE: &str = "t3.micro";

/// How long a leg waits for the clock-owed moves it depends on before it gives up and reports
/// what it last read, and how often it looks.
///
/// Many multiples of the longest of them, because the deadline is only here so a failure is a
/// failure rather than a hang: every wait below looks for its CONDITION, so machine load
/// decides when a leg finishes and never whether it passes. Nothing here asserts on elapsed
/// time.
const DEADLINE: Duration = Duration::from_secs(240);
const POLL: Duration = Duration::from_millis(100);

/// The `TargetHealthStateEnum` values this suite reads.
const INITIAL: &str = "initial";
const HEALTHY: &str = "healthy";
const UNHEALTHY: &str = "unhealthy";

/// The `TargetHealthReasonEnum` value a Network Load Balancer reports a check that did not
/// succeed under: "`Target.FailedHealthChecks` - The load balancer received an error while
/// establishing a connection to the target or the target response was malformed"
/// (`TargetHealth$Reason`).
const FAILED_HEALTH_CHECKS: &str = "Target.FailedHealthChecks";

/// The `LifecycleState` and the `HealthStatus` a group reports for a member in service.
const IN_SERVICE: &str = "InService";
const HEALTHY_INSTANCE: &str = "Healthy";

/// The path an HTTP-checked row's health check requests, and the port the guest serves it on.
const HEALTH_CHECK_PATH: &str = "/health-check";
const HEALTH_CHECK_PORT: &str = "4000";

/// The body of one control-plane answer that must have been served.
fn served(action: &str, answer: Answer) -> String {
    assert_eq!(
        answer.status, 200,
        "setup: {action} must succeed, got {answer:?}"
    );
    answer.text
}

/// Drive one Elastic Load Balancing v2 action that must succeed, and answer its body.
fn elbv2_ok(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    served(action, elbv2(router, runtime, action, params))
}

/// Drive one Auto Scaling action that must succeed, and answer its body.
fn asg_ok(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    served(
        action,
        send(
            router,
            runtime,
            "autoscaling",
            form(action, ASG_VERSION, params),
        ),
    )
}

/// The whole emulator over one engine, exactly as the binary assembles it, with signature
/// verification left to the front door.
fn router() -> MultiServiceRouter {
    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
    let mut router = build_router(engine);
    router.set_request_authentication(false);
    router
}

/// A current-thread runtime, which carries each request below and drives the emulator's own
/// clock-owed work while it does.
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a current-thread runtime")
}

/// Spend one polling interval inside the runtime that drives the emulator's scheduled work.
fn waited(runtime: &tokio::runtime::Runtime) {
    runtime.block_on(async { tokio::time::sleep(POLL).await });
}

/// One VPC and the one subnet the load balancer's nodes and the group's instances both sit in.
struct Network {
    vpc: String,
    subnet: String,
    block: String,
}

/// Create that network through Amazon EC2 itself, so every id below is one EC2 minted.
fn seed_network(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    vpc_block: &str,
    subnet_block: &str,
) -> Network {
    let vpc = element(
        &ec2_ok(router, runtime, "CreateVpc", &[("CidrBlock", vpc_block)]),
        "vpcId",
    );
    let subnet = element(
        &ec2_ok(
            router,
            runtime,
            "CreateSubnet",
            &[
                ("VpcId", vpc.as_str()),
                ("CidrBlock", subnet_block),
                ("AvailabilityZone", "us-west-2a"),
            ],
        ),
        "subnetId",
    );
    Network {
        vpc,
        subnet,
        block: subnet_block.to_string(),
    }
}

/// Create one security group in `vpc` carrying the SELF-REFERENCE a cluster security group
/// carries and nothing else, and answer its id: no rule of it names a load balancer outside the
/// group, and none names an address range at all.
fn cluster_security_group(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    vpc: &str,
    name: &str,
) -> String {
    let group = element(
        &ec2_ok(
            router,
            runtime,
            "CreateSecurityGroup",
            &[
                ("GroupName", name),
                ("GroupDescription", "the groups instances"),
                ("VpcId", vpc),
            ],
        ),
        "groupId",
    );
    ec2_ok(
        router,
        runtime,
        "AuthorizeSecurityGroupIngress",
        &[
            ("GroupId", group.as_str()),
            ("IpPermissions.1.IpProtocol", "-1"),
            ("IpPermissions.1.Groups.1.GroupId", group.as_str()),
        ],
    );
    group
}

/// The guest program every row's instances run: a server listening on the port the checks go
/// to, holding the document an HTTP-checked row's health check requests at its own path.
fn guest_program() -> String {
    format!(
        "#!/bin/bash\n\
         mkdir -p /srv/health\n\
         echo ok > /srv/health{HEALTH_CHECK_PATH}\n\
         nohup python3 -m http.server {HEALTH_CHECK_PORT} --directory /srv/health &\n"
    )
}

/// Create one launch template that launches into `group`, running [`guest_program`] — the
/// program the guest performs at boot, which the API carries base64-encoded.
fn launch_template(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    name: &str,
    group: &str,
) {
    let encoded = aws_smithy_types::base64::encode(guest_program());
    ec2_ok(
        router,
        runtime,
        "CreateLaunchTemplate",
        &[
            ("LaunchTemplateName", name),
            ("LaunchTemplateData.ImageId", IMAGE_ID),
            ("LaunchTemplateData.InstanceType", INSTANCE_TYPE),
            // `RequestLaunchTemplateData$SecurityGroupIds` travels as its own `xmlName`,
            // `SecurityGroupId`, which is what an ec2Query client sends.
            ("LaunchTemplateData.SecurityGroupId.1", group),
            ("LaunchTemplateData.UserData", encoded.as_str()),
        ],
    );
}

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

/// Every registration the target group `arn` holds right now, as a client reads it.
fn target_health(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    arn: &str,
) -> Vec<Reported> {
    let body = elbv2_ok(
        router,
        runtime,
        "DescribeTargetHealth",
        &[("TargetGroupArn", arn)],
    );
    let descriptions = block_of(&body, "TargetHealthDescriptions").unwrap_or_default();
    members_of(descriptions)
        .into_iter()
        .map(|member| {
            let target = block_of(member, "Target").unwrap_or_default();
            let health = block_of(member, "TargetHealth").unwrap_or_default();
            Reported {
                id: opt_element(target, "Id").unwrap_or_default(),
                port: opt_element(target, "Port"),
                state: opt_element(health, "State").unwrap_or_default(),
                reason: opt_element(health, "Reason"),
                description: opt_element(health, "Description"),
                health_check_port: opt_element(member, "HealthCheckPort"),
            }
        })
        .collect()
}

/// One instance of an Auto Scaling group, as `DescribeAutoScalingGroups` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Held {
    instance_id: String,
    lifecycle_state: String,
    health_status: String,
}

/// The instances the group `name` reports right now.
fn group_instances(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    name: &str,
) -> Vec<Held> {
    let body = asg_ok(
        router,
        runtime,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", name)],
    );
    let instances = block_of(&body, "Instances").unwrap_or_default();
    members_of(instances)
        .into_iter()
        .map(|member| Held {
            instance_id: opt_element(member, "InstanceId").unwrap_or_default(),
            lifecycle_state: opt_element(member, "LifecycleState").unwrap_or_default(),
            health_status: opt_element(member, "HealthStatus").unwrap_or_default(),
        })
        .collect()
}

/// Read both services until `settled` holds for the pair, or [`DEADLINE`] passes; answer the
/// last reading either way, so a failure reports what was really read — beside every target
/// state seen on the way, so a leg can assert what was never reported.
fn read_until(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    target_group: &str,
    group_name: &str,
    settled: impl Fn(&[Reported], &[Held]) -> bool,
) -> (Vec<Reported>, Vec<Held>, Vec<String>) {
    let deadline = Instant::now() + DEADLINE;
    let mut seen: Vec<String> = Vec::new();
    loop {
        let reported = target_health(router, runtime, target_group);
        let held = group_instances(router, runtime, group_name);
        for report in &reported {
            if seen.last() != Some(&report.state) {
                seen.push(report.state.clone());
            }
        }
        if settled(&reported, &held) || Instant::now() >= deadline {
            return (reported, held, seen);
        }
        waited(runtime);
    }
}

/// One row: the health-check schedule its target group carries, and where its checks go.
struct Row {
    case: &'static str,
    /// What every resource name of this row is suffixed with.
    slug: &'static str,
    /// `CreateTargetGroup$Port`, the port the registration names the target as listening on.
    traffic_port: &'static str,
    /// `HealthCheckPort` where the row names one, and `None` for a row checked on the port its
    /// traffic goes to.
    health_check_port: Option<&'static str>,
    /// Whether the check speaks HTTP semantics — a protocol and a path of its own.
    checks_over_http: bool,
    /// `HealthCheckIntervalSeconds` and `HealthCheckTimeoutSeconds` the create asks for.
    interval: &'static str,
    timeout: &'static str,
    /// `HealthyThresholdCount` and `UnhealthyThresholdCount` the create asks for.
    thresholds: (&'static str, &'static str),
    /// The port the checks therefore connect to, which is what the remediation admits.
    checked_port: &'static str,
}

/// The rows: the recorded configuration, and a group whose checks go to the port its traffic
/// does at the shortest cadence the member admits.
const ROWS: [Row; 2] = [
    Row {
        case: "the recorded configuration: a TCP group checked over HTTP on a port of its own, \
               at a twenty-second cadence and the most failing checks the threshold admits",
        slug: "recorded",
        traffic_port: "44300",
        health_check_port: Some(HEALTH_CHECK_PORT),
        checks_over_http: true,
        interval: "20",
        timeout: "10",
        thresholds: ("2", "10"),
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
        timeout: "2",
        thresholds: ("2", "2"),
        checked_port: HEALTH_CHECK_PORT,
    },
];

/// Everything one row needs standing up, so every row's launch and checks run on the one clock
/// instead of one row waiting out the next row's setup.
struct Deployed<'a> {
    row: &'a Row,
    guarding: String,
    target_group: String,
    group_name: String,
}

#[test]
fn a_group_behind_an_ingressless_security_group_reaches_both_verdicts_at_its_own_cadence() {
    std::thread::Builder::new()
        .name("asg-nlb-verdicts".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let router = router();
            let runtime = runtime();
            // One subnet, which is where the internal Network Load Balancers below put their
            // own nodes and where the groups' instances are launched.
            let network = seed_network(&router, &runtime, "172.29.0.0/16", "172.29.0.0/23");

            let mut deployed: Vec<Deployed<'_>> = Vec::new();
            for row in &ROWS {
                let guarding = cluster_security_group(
                    &router,
                    &runtime,
                    &network.vpc,
                    &format!("asgnlb-{}-sg", row.slug),
                );
                let template = format!("asgnlb-{}-lt", row.slug);
                launch_template(&router, &runtime, &template, &guarding);
                let group_label = format!("asgnlb-{}-tg", row.slug);
                let mut group_params: Vec<(&str, &str)> = vec![
                    ("Name", group_label.as_str()),
                    ("Protocol", "TCP"),
                    ("Port", row.traffic_port),
                    ("VpcId", network.vpc.as_str()),
                    ("TargetType", "instance"),
                    ("HealthCheckIntervalSeconds", row.interval),
                    ("HealthCheckTimeoutSeconds", row.timeout),
                    ("HealthyThresholdCount", row.thresholds.0),
                    ("UnhealthyThresholdCount", row.thresholds.1),
                ];
                if let Some(port) = row.health_check_port {
                    group_params.push(("HealthCheckPort", port));
                }
                if row.checks_over_http {
                    group_params.push(("HealthCheckProtocol", "HTTP"));
                    group_params.push(("HealthCheckPath", HEALTH_CHECK_PATH));
                }
                let target_group = element(
                    &elbv2_ok(&router, &runtime, "CreateTargetGroup", &group_params),
                    "TargetGroupArn",
                );
                // An INTERNAL Network Load Balancer, associated with no security group of its
                // own, so the only rule that can name it is one covering the block its nodes
                // sit in.
                let balancer_name = format!("asgnlb-{}-nlb", row.slug);
                let balancer = element(
                    &elbv2_ok(
                        &router,
                        &runtime,
                        "CreateLoadBalancer",
                        &[
                            ("Name", balancer_name.as_str()),
                            ("Type", "network"),
                            ("Scheme", "internal"),
                            ("Subnets.member.1", network.subnet.as_str()),
                        ],
                    ),
                    "LoadBalancerArn",
                );
                // A target group nothing forwards to is checked by no load balancer at all, so
                // the listener is what starts the checks this row is about.
                elbv2_ok(
                    &router,
                    &runtime,
                    "CreateListener",
                    &[
                        ("LoadBalancerArn", balancer.as_str()),
                        ("Protocol", "TCP"),
                        ("Port", row.traffic_port),
                        ("DefaultActions.member.1.Type", "forward"),
                        ("DefaultActions.member.1.TargetGroupArn", target_group.as_str()),
                    ],
                );
                let group_name = format!("asgnlb-{}-asg", row.slug);
                asg_ok(
                    &router,
                    &runtime,
                    "CreateAutoScalingGroup",
                    &[
                        ("AutoScalingGroupName", group_name.as_str()),
                        ("MinSize", "1"),
                        ("MaxSize", "1"),
                        ("DesiredCapacity", "1"),
                        ("LaunchTemplate.LaunchTemplateName", template.as_str()),
                        ("VPCZoneIdentifier", network.subnet.as_str()),
                        ("TargetGroupARNs.member.1", target_group.as_str()),
                        // Amazon EC2's own checking, so the group replaces nothing: what this
                        // suite reads is the load balancer's verdict about the member the
                        // group keeps.
                        ("HealthCheckType", "EC2"),
                    ],
                );
                deployed.push(Deployed {
                    row,
                    guarding,
                    target_group,
                    group_name,
                });
            }

            // LEG 1: no rule of the instances' own group names the load balancer, so its checks
            // arrive nowhere and the registration reaches the verdict its own threshold of
            // consecutive failures decides — at the cadence its own group was created with.
            for entry in &deployed {
                let row = entry.row;
                let (reported, held, seen) = read_until(
                    &router,
                    &runtime,
                    &entry.target_group,
                    &entry.group_name,
                    |reported, _| reported.iter().any(|report| report.state == UNHEALTHY),
                );
                let verdict = reported.iter().find(|report| report.state == UNHEALTHY);
                assert!(
                    verdict.is_some(),
                    "case {:?}: expected the registration to reach {UNHEALTHY} while no rule \
                     admits the checks of port {}, got {reported:?} with the group holding \
                     {held:?} (states seen: {seen:?})",
                    row.case,
                    row.checked_port
                );
                let verdict = verdict.expect("the verdict asserted above");
                assert_eq!(
                    verdict.reason.as_deref(),
                    Some(FAILED_HEALTH_CHECKS),
                    "case {:?}: a Network Load Balancer's check that nothing answered is \
                     reported under {FAILED_HEALTH_CHECKS}, got {verdict:?}",
                    row.case
                );
                // The registration is held on the port the traffic goes to, and the checks
                // connect to the group's own effective health-check port.
                assert_eq!(
                    (
                        verdict.port.as_deref(),
                        verdict.health_check_port.as_deref()
                    ),
                    (Some(row.traffic_port), Some(row.checked_port)),
                    "case {:?}: expected the registration on port {} and the checks on port \
                     {}, got {verdict:?}",
                    row.case,
                    row.traffic_port,
                    row.checked_port
                );
                // A target no rule admits answers nothing from its first check on, so the only
                // state it can have passed through on the way to the verdict is `initial`.
                assert!(
                    seen.iter()
                        .all(|state| state == INITIAL || state == UNHEALTHY),
                    "case {:?}: a target whose checks cannot arrive must never be reported \
                     {HEALTHY}, got the states {seen:?}",
                    row.case
                );
            }

            // LEG 2: the remediation — each row's own effective health-check port, admitted
            // from the block the load balancer's nodes sit in, which is the only way a rule can
            // name a Network Load Balancer associated with no security group.
            for entry in &deployed {
                ec2_ok(
                    &router,
                    &runtime,
                    "AuthorizeSecurityGroupIngress",
                    &[
                        ("GroupId", entry.guarding.as_str()),
                        ("IpPermissions.1.IpProtocol", "tcp"),
                        ("IpPermissions.1.FromPort", entry.row.checked_port),
                        ("IpPermissions.1.ToPort", entry.row.checked_port),
                        ("IpPermissions.1.IpRanges.1.CidrIp", network.block.as_str()),
                    ],
                );
            }
            for entry in &deployed {
                let row = entry.row;
                let (reported, held, seen) = read_until(
                    &router,
                    &runtime,
                    &entry.target_group,
                    &entry.group_name,
                    |reported, held| {
                        reported.iter().any(|report| report.state == HEALTHY)
                            && held.iter().any(|instance| {
                                instance.health_status == HEALTHY_INSTANCE
                                    && instance.lifecycle_state == IN_SERVICE
                            })
                    },
                );
                let settled = reported.iter().find(|report| report.state == HEALTHY);
                assert!(
                    settled.is_some(),
                    "case {:?}: expected the registration to reach {HEALTHY} once port {} is \
                     admitted from {}, got {reported:?} with the group holding {held:?} \
                     (states seen: {seen:?})",
                    row.case,
                    row.checked_port,
                    network.block
                );
                let settled = settled.expect("the verdict asserted above");
                // "If the target state is healthy, a reason code is not provided."
                assert_eq!(
                    (settled.reason.as_deref(), settled.description.as_deref()),
                    (None, None),
                    "case {:?}: a healthy registration carries no reason and no description, \
                     got {settled:?}",
                    row.case
                );
                // …and the healthy registration is the member the group holds, in service.
                assert!(
                    held.iter().any(|instance| {
                        instance.instance_id == settled.id
                            && instance.health_status == HEALTHY_INSTANCE
                            && instance.lifecycle_state == IN_SERVICE
                    }),
                    "case {:?}: expected the healthy registration {settled:?} to be the \
                     group's own member, {HEALTHY_INSTANCE} and {IN_SERVICE}, got {held:?}",
                    row.case
                );
            }
        })
        .expect("the suite's own thread")
        .join()
        .expect("the suite's own thread finishes");
}
