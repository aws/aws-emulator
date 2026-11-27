// SPDX-License-Identifier: CC0-1.0

//! An Auto Scaling group's target-group lifecycle, and the health verdict Elastic Load
//! Balancing v2 reaches for the instances it registers — through the WHOLE mounted service
//! set, the one `core/router/src/lib.rs` assembles for the binary.
//!
//! TWO RULES, both of which only hold if Auto Scaling, Amazon EC2 and Elastic Load Balancing
//! v2 agree over one engine, which is why this suite lives here:
//!
//! 1. **An instance of a group is registered in the attached target groups it COULD be
//!    registered in, and in no others.** A capacity change launches the instance in the
//!    account's own Amazon EC2, drives it to `InService`, and registers it as a target of
//!    every target group the group is attached to — `CreateAutoScalingGroup$TargetGroupARNs`
//!    is "the Amazon Resource Names (ARN) of the target groups to associate with the Auto
//!    Scaling group. Instances are registered as targets in a target group" — with
//!    `HealthCheckType=ELB` the load balancer's verdict then showing on the group's instance.
//!    A target group of ANOTHER VPC takes none of them: "the specified target does not exist,
//!    is not in the same VPC as the target group, or has an unsupported instance type" is what
//!    Elastic Load Balancing v2 refuses such a registration with, so the attachment stands and
//!    stays in `Adding`. And a target group DELETED out of band can take no registration at
//!    all, so the scale-out that would have to register there launches nothing and records the
//!    failure as a `Failed` scaling activity.
//!
//! 2. **A target is healthy only while a rule admits the load balancer's check.** A health
//!    check is a connection the load balancer opens to the target on the group's effective
//!    health-check port, so it arrives only if an inbound rule of a security group governing
//!    that instance names both that port and that load balancer as a source — a security group
//!    "controls the traffic that is allowed to reach and leave the resources that it is
//!    associated with" (Amazon VPC User Guide, "Security groups"). A load balancer associated
//!    with NO security group — where a Network Load Balancer created without one sits — is
//!    named by no rule naming a source security group, so only a rule covering its own node
//!    addresses admits its checks. A target no check can reach settles on `unhealthy` with the
//!    reason the family of load balancer checking it words a check nothing answered as
//!    (`Target.Timeout` for an Application Load Balancer, `Target.FailedHealthChecks` for a
//!    Network Load Balancer), and a target whose checks arrive is `healthy` with no reason.
//!
//! The rows of the second rule span what it generalises over: the load balancer's family, the
//! kind of inbound rule the instances' own security group carries (none admitting the check, a
//! rule naming the load balancer's own group, a rule naming the address range its nodes sit
//! in), and the health-check port. The recorded configuration — an internal Network Load
//! Balancer with no security group in front of instances whose group admits only itself — is
//! one row.

use std::time::{Duration, Instant};

use bytes::Bytes;
use smithy_server_runtime::MultiServiceRouter;

// The ONE percent-encoder and element readers this test crate keeps for awsQuery suites
// (`all/query_wire.rs`: "Nothing new should write another one — call this module").
use crate::query_wire::{element, elements, form_encode, members_of, opt_element, target_health_of};

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// The API versions an ec2Query and each awsQuery request carries.
const EC2_API_VERSION: &str = "2016-11-15";
const ELB_VERSION: &str = "2015-12-01";
const ASG_VERSION: &str = "2011-01-01";

/// The AMI and instance type every launch template here names.
const IMAGE_ID: &str = "ami-0abcdef1234567890";
const INSTANCE_TYPE: &str = "t3.micro";

/// How long a leg waits for the clock-owed moves it depends on — an instance's launch, its
/// registration, and the health checks of it, which come the target group's own
/// `HealthCheckIntervalSeconds` apart ([`CHECK_INTERVAL_SECONDS`]) so that a verdict is a
/// threshold of them away — and how often it looks. A few multiples of the longest verdict
/// span a row configures, because the deadline is only here so a failure is a failure rather
/// than a hang: the waits below look for their condition, so machine load decides when a leg
/// finishes and never whether it passes.
const DEADLINE: Duration = Duration::from_secs(60);

/// The `HealthCheckIntervalSeconds` and `UnhealthyThresholdCount` every target group of this
/// suite is created with: the shortest spacing and the fewest consecutive checks the two
/// members' own ranges allow (5-300 and 2-10), because a verdict about a registration is that
/// many checks at that spacing away. What this suite asserts is WHICH registrations a scaling
/// group makes and what their checks then decide, not how long the load balancer takes.
const CHECK_INTERVAL_SECONDS: &str = "5";
const CHECK_THRESHOLD: &str = "2";
const POLL: Duration = Duration::from_millis(100);

/// How long a leg keeps re-reading a state that must NOT change (a registration that must
/// never appear), and how often: the absence is asserted over a run of reads rather than one.
const HELD_READS: usize = 20;

/// The whole emulator over one engine, exactly as the binary assembles it.
fn router() -> MultiServiceRouter {
    let engine = aws_emulator_router::new_engine();
    let mut router = aws_emulator_router::build_router(engine);
    // Signature verification is the front door's concern, not this suite's.
    router.set_request_authentication(false);
    router
}

/// One form body: the action, the version and the parameters.
fn form(action: &str, version: &str, params: &[(&str, &str)]) -> String {
    let mut body = format!("Action={action}&Version={version}");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    body
}

/// Send one form-encoded request to the service `signing_name` addresses, and answer its
/// status and body.
async fn send(router: &MultiServiceRouter, signing_name: &str, body: String) -> (u16, String) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{signing_name}.{REGION}.amazonaws.com/"))
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).to_string(),
    )
}

/// One ec2Query action that must succeed.
async fn ec2(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> String {
    let (status, body) = send(router, "ec2", form(action, EC2_API_VERSION, params)).await;
    assert_eq!(
        status, 200,
        "setup: EC2 {action} must succeed, got {status} with body {body}"
    );
    body
}

/// One Elastic Load Balancing v2 action that must succeed.
async fn elbv2(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> String {
    let (status, body) = send(
        router,
        "elasticloadbalancingv2",
        form(action, ELB_VERSION, params),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: ELBv2 {action} must succeed, got {status} with body {body}"
    );
    body
}

/// One Auto Scaling action that must succeed.
async fn asg(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> String {
    let (status, body) = send(router, "autoscaling", form(action, ASG_VERSION, params)).await;
    assert_eq!(
        status, 200,
        "setup: Auto Scaling {action} must succeed, got {status} with body {body}"
    );
    body
}

/// The network one leg runs in, created through Amazon EC2 itself so every id asserted
/// against is one EC2 minted.
struct Network {
    vpc: String,
    /// The subnets, in the order created, each with the address range it was created with.
    subnets: Vec<(String, String)>,
}

/// Create a VPC of `vpc_cidr` with one subnet per entry of `subnets` (`(block, zone)`).
async fn seed_network(
    router: &MultiServiceRouter,
    vpc_cidr: &str,
    subnets: &[(&str, &str)],
) -> Network {
    let vpc = element(
        &ec2(router, "CreateVpc", &[("CidrBlock", vpc_cidr)]).await,
        "vpcId",
    );
    let mut created = Vec::new();
    for (cidr, zone) in subnets {
        let id = element(
            &ec2(
                router,
                "CreateSubnet",
                &[
                    ("VpcId", vpc.as_str()),
                    ("CidrBlock", cidr),
                    ("AvailabilityZone", zone),
                ],
            )
            .await,
            "subnetId",
        );
        created.push((id, (*cidr).to_string()));
    }
    Network {
        vpc,
        subnets: created,
    }
}

/// Create one security group in `vpc` and answer its id.
async fn security_group(
    router: &MultiServiceRouter,
    vpc: &str,
    name: &str,
    description: &str,
) -> String {
    element(
        &ec2(
            router,
            "CreateSecurityGroup",
            &[
                ("GroupName", name),
                ("GroupDescription", description),
                ("VpcId", vpc),
            ],
        )
        .await,
        "groupId",
    )
}

/// Create one launch template naming `security_groups` and answer nothing: the group names it
/// by name.
async fn launch_template(router: &MultiServiceRouter, name: &str, security_groups: &[&str]) {
    let mut params: Vec<(String, String)> = vec![
        ("LaunchTemplateName".to_string(), name.to_string()),
        (
            "LaunchTemplateData.ImageId".to_string(),
            IMAGE_ID.to_string(),
        ),
        (
            "LaunchTemplateData.InstanceType".to_string(),
            INSTANCE_TYPE.to_string(),
        ),
    ];
    for (index, group) in security_groups.iter().enumerate() {
        params.push((
            // `RequestLaunchTemplateData$SecurityGroupIds` travels as its own `xmlName`,
            // `SecurityGroupId`, which is what an ec2Query client sends.
            format!("LaunchTemplateData.SecurityGroupId.{}", index + 1),
            (*group).to_string(),
        ));
    }
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    ec2(router, "CreateLaunchTemplate", &borrowed).await;
}

/// One registration as `DescribeTargetHealth` reports it: the target, its state, and the
/// reason code the state carries (empty for a state reported without one).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    id: String,
    state: String,
    reason: String,
}

/// Every registration the target group `arn` holds right now, as a client reads it.
async fn target_health(router: &MultiServiceRouter, arn: &str) -> Vec<Reported> {
    let body = elbv2(router, "DescribeTargetHealth", &[("TargetGroupArn", arn)]).await;
    members_of(&body)
        .into_iter()
        .map(|description| {
            // One registration per member, each read inside its OWN member: the state and
            // the reason come out of that member's health block, because the administrative
            // override beside it carries members of the same names
            // ([`target_health_of`]).
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
            }
        })
        .collect()
}

/// The `(LifecycleState, HealthStatus)` the group `name` reports for each instance it holds.
async fn group_instances(router: &MultiServiceRouter, name: &str) -> Vec<(String, String)> {
    let body = asg(
        router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", name)],
    )
    .await;
    elements(&body, "LifecycleState")
        .into_iter()
        .zip(elements(&body, "HealthStatus"))
        .collect()
}

/// The state the group `name` reports for the attachment of `arn`, through the reader whose
/// subject the attachments are.
async fn attachment_state(router: &MultiServiceRouter, name: &str, arn: &str) -> String {
    let body = asg(
        router,
        "DescribeLoadBalancerTargetGroups",
        &[("AutoScalingGroupName", name)],
    )
    .await;
    elements(&body, "LoadBalancerTargetGroupARN")
        .into_iter()
        .zip(elements(&body, "State"))
        .find(|(reported, _)| reported == arn)
        .map(|(_, state)| state)
        .unwrap_or_default()
}

/// RULE 1: a group's instance reaches `InService` and is registered — and reported healthy —
/// in the attached target group of its own VPC, is NOT registered in an attached target group
/// of another VPC, and a scale-out whose attached target group has been deleted out of band
/// launches nothing and records the failure.
#[tokio::test]
async fn an_asg_registers_its_instance_only_where_it_could_be_registered() {
    let router = router();
    let network = seed_network(
        &router,
        "10.70.0.0/16",
        &[("10.70.1.0/24", "us-east-1a"), ("10.70.2.0/24", "us-east-1b")],
    )
    .await;
    // A VPC the group places nothing in, holding the target group that can take none of its
    // instances.
    let elsewhere = seed_network(&router, "10.80.0.0/16", &[("10.80.1.0/24", "us-east-1a")]).await;

    let instances_group = security_group(&router, &network.vpc, "asglc-instances", "the instances").await;
    let balancer_group = security_group(&router, &network.vpc, "asglc-alb", "the load balancer").await;
    // The instances admit the load balancer's own group on the traffic and health-check port,
    // which is what lets its checks arrive.
    ec2(
        &router,
        "AuthorizeSecurityGroupIngress",
        &[
            ("GroupId", instances_group.as_str()),
            ("IpPermissions.1.IpProtocol", "tcp"),
            ("IpPermissions.1.FromPort", "80"),
            ("IpPermissions.1.ToPort", "80"),
            ("IpPermissions.1.Groups.1.GroupId", balancer_group.as_str()),
        ],
    )
    .await;
    launch_template(&router, "asglc-template", &[instances_group.as_str()]).await;

    let home = element(
        &elbv2(
            &router,
            "CreateTargetGroup",
            &[
                ("Name", "asglc-home"),
                ("Protocol", "HTTP"),
                ("Port", "80"),
                ("VpcId", network.vpc.as_str()),
                ("TargetType", "instance"),
                ("HealthCheckIntervalSeconds", CHECK_INTERVAL_SECONDS),
                ("UnhealthyThresholdCount", CHECK_THRESHOLD),
                ("HealthyThresholdCount", CHECK_THRESHOLD),
            ],
        )
        .await,
        "TargetGroupArn",
    );
    let away = element(
        &elbv2(
            &router,
            "CreateTargetGroup",
            &[
                ("Name", "asglc-away"),
                ("Protocol", "HTTP"),
                ("Port", "80"),
                ("VpcId", elsewhere.vpc.as_str()),
                ("TargetType", "instance"),
                ("HealthCheckIntervalSeconds", CHECK_INTERVAL_SECONDS),
                ("UnhealthyThresholdCount", CHECK_THRESHOLD),
                ("HealthyThresholdCount", CHECK_THRESHOLD),
            ],
        )
        .await,
        "TargetGroupArn",
    );
    let balancer = element(
        &elbv2(
            &router,
            "CreateLoadBalancer",
            &[
                ("Name", "asglc-alb"),
                ("Scheme", "internal"),
                ("Subnets.member.1", network.subnets[0].0.as_str()),
                ("Subnets.member.2", network.subnets[1].0.as_str()),
                ("SecurityGroups.member.1", balancer_group.as_str()),
            ],
        )
        .await,
        "LoadBalancerArn",
    );
    let listener = element(
        &elbv2(
            &router,
            "CreateListener",
            &[
                ("LoadBalancerArn", balancer.as_str()),
                ("Protocol", "HTTP"),
                ("Port", "80"),
                ("DefaultActions.member.1.Type", "forward"),
                ("DefaultActions.member.1.TargetGroupArn", home.as_str()),
            ],
        )
        .await,
        "ListenerArn",
    );

    let group = "asglc-group";
    asg(
        &router,
        "CreateAutoScalingGroup",
        &[
            ("AutoScalingGroupName", group),
            ("MinSize", "1"),
            ("MaxSize", "1"),
            ("DesiredCapacity", "1"),
            ("LaunchTemplate.LaunchTemplateName", "asglc-template"),
            ("VPCZoneIdentifier", network.subnets[0].0.as_str()),
            ("TargetGroupARNs.member.1", home.as_str()),
            // "ELB - Use Elastic Load Balancing health checks": the load balancer's verdict
            // is this group's instance health.
            ("HealthCheckType", "ELB"),
        ],
    )
    .await;
    // The target group of the other VPC is attached too, so the only reason it takes no
    // registration is the VPC it belongs to.
    asg(
        &router,
        "AttachLoadBalancerTargetGroups",
        &[
            ("AutoScalingGroupName", group),
            ("TargetGroupARNs.member.1", away.as_str()),
        ],
    )
    .await;

    // The launch, the registration and the first checks are all clock-owed: the leg waits for
    // the CONDITION — one `InService`, `Healthy` instance reported `healthy` in the home
    // target group — rather than for a span.
    let deadline = Instant::now() + DEADLINE;
    let (held, reported) = loop {
        let held = group_instances(&router, group).await;
        let reported = target_health(&router, &home).await;
        let converged = held.len() == 1
            && held[0] == ("InService".to_string(), "Healthy".to_string())
            && reported.len() == 1
            && reported[0].state == "healthy";
        if converged || Instant::now() >= deadline {
            break (held, reported);
        }
        tokio::time::sleep(POLL).await;
    };
    assert_eq!(
        held,
        vec![("InService".to_string(), "Healthy".to_string())],
        "the group must bring its one instance into service with the load balancer's verdict, \
         got {held:?} with the target group reporting {reported:?}"
    );
    assert_eq!(
        reported.len(),
        1,
        "the home target group must hold exactly the group's instance, got {reported:?}"
    );
    assert_eq!(
        (reported[0].state.as_str(), reported[0].reason.as_str()),
        ("healthy", ""),
        "a target whose checks arrive is healthy with no reason, got {reported:?}"
    );
    let instance = reported[0].id.clone();
    assert!(
        instance.starts_with("i-"),
        "the registration must name the group's own Amazon EC2 instance, got {reported:?}"
    );
    // The attachment the registration belongs to is reported `InService`: "at least one Auto
    // Scaling instance passed an ELB health check".
    let state = attachment_state(&router, group, &home).await;
    assert_eq!(
        state, "InService",
        "the home attachment must be reported InService once its registration is healthy, got \
         {state:?}"
    );

    // The target group of the other VPC holds NOTHING, and goes on holding nothing: the
    // absence is asserted over a run of reads, so a registration that arrives late still
    // fails the leg.
    for read in 0..HELD_READS {
        let cross_vpc = target_health(&router, &away).await;
        assert!(
            cross_vpc.is_empty(),
            "read {read}: an attached target group of another VPC must take no registration of \
             the group's instance, got {cross_vpc:?}"
        );
        tokio::time::sleep(POLL).await;
    }
    let state = attachment_state(&router, group, &away).await;
    assert_eq!(
        state, "Adding",
        "the attachment that can take no registration stays in Adding, got {state:?}"
    );

    // The home target group is deleted out of band, which leaves the group attached to a
    // target group that can take no registration at all: the scale-out it is then asked for
    // launches nothing and records the failure.
    elbv2(&router, "DeleteListener", &[("ListenerArn", listener.as_str())]).await;
    elbv2(&router, "DeleteTargetGroup", &[("TargetGroupArn", home.as_str())]).await;
    asg(
        &router,
        "UpdateAutoScalingGroup",
        &[("AutoScalingGroupName", group), ("MaxSize", "2")],
    )
    .await;
    asg(
        &router,
        "SetDesiredCapacity",
        &[("AutoScalingGroupName", group), ("DesiredCapacity", "2")],
    )
    .await;
    // The refusal is recorded as the scale-out is attempted, which is clock-owed too: wait
    // for the activity that names the target group it could not update.
    let deadline = Instant::now() + DEADLINE;
    let (statuses, messages) = loop {
        let body = asg(
            &router,
            "DescribeScalingActivities",
            &[("AutoScalingGroupName", group)],
        )
        .await;
        let statuses = elements(&body, "StatusCode");
        let messages = elements(&body, "StatusMessage");
        if statuses.iter().any(|status| status == "Failed") || Instant::now() >= deadline {
            break (statuses, messages);
        }
        tokio::time::sleep(POLL).await;
    };
    assert!(
        statuses.iter().any(|status| status == "Failed"),
        "the scale-out into a deleted target group must record a Failed activity, got \
         {statuses:?} {messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains(&home) && message.contains("Launching EC2 instance failed.")),
        "the failed activity must name the target group it could not update, got {messages:?}"
    );
    let after = group_instances(&router, group).await;
    assert_eq!(
        after,
        vec![("InService".to_string(), "Healthy".to_string())],
        "the blocked scale-out must launch nothing, leaving the group's one instance, got \
         {after:?}"
    );
    let still = elements(
        &asg(
            &router,
            "DescribeAutoScalingGroups",
            &[("AutoScalingGroupNames.member.1", group)],
        )
        .await,
        "InstanceId",
    );
    assert_eq!(
        still,
        vec![instance],
        "the group must still hold exactly the instance it had, got {still:?}"
    );
}

/// What a row's instances admit on the checked port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Admit {
    /// Nothing from any address range: the group's only inbound rule names itself, which is
    /// the configuration a cluster security group carries.
    Nothing,
    /// The address range the load balancer's own nodes sit in.
    SubnetRange,
    /// The security group the load balancer itself is associated with.
    BalancerGroup,
}

/// One row of the health-verdict table.
struct HealthRow {
    case: &'static str,
    /// `CreateLoadBalancer$Type`, and so which family checks the target.
    lb_type: &'static str,
    /// The target group's own protocol, which is the family's: HTTP for an Application Load
    /// Balancer, TCP for a Network Load Balancer.
    protocol: &'static str,
    /// The port the target group's traffic and health checks go to.
    port: &'static str,
    admit: Admit,
    /// The `TargetHealth$State` the registration must reach, and the `TargetHealth$Reason`
    /// that state carries (empty for a state reported without a reason code: "if the target
    /// state is healthy, a reason code is not provided").
    state: &'static str,
    reason: &'static str,
}

const HEALTH_ROWS: [HealthRow; 4] = [
    HealthRow {
        case: "the recorded configuration: an internal network load balancer with no security \
               group, in front of instances that admit nothing from any range",
        lb_type: "network",
        protocol: "TCP",
        port: "4000",
        admit: Admit::Nothing,
        state: "unhealthy",
        reason: "Target.FailedHealthChecks",
    },
    HealthRow {
        case: "the same network load balancer once its nodes' own subnet range is admitted",
        lb_type: "network",
        protocol: "TCP",
        port: "4000",
        admit: Admit::SubnetRange,
        state: "healthy",
        reason: "",
    },
    HealthRow {
        case: "an application load balancer whose own security group is admitted",
        lb_type: "application",
        protocol: "HTTP",
        port: "80",
        admit: Admit::BalancerGroup,
        state: "healthy",
        reason: "",
    },
    HealthRow {
        case: "an application load balancer whose checks no rule admits",
        lb_type: "application",
        protocol: "HTTP",
        port: "80",
        admit: Admit::Nothing,
        state: "unhealthy",
        reason: "Target.Timeout",
    },
];

/// RULE 2: the health verdict of an Auto Scaling group's registration follows the inbound
/// rules of the instance's own security groups, read against the load balancer performing the
/// check — and an unhealthy verdict carries the reason that family of load balancer words a
/// check nothing answered as.
#[tokio::test]
async fn a_target_is_healthy_only_while_a_rule_admits_the_load_balancers_check() {
    for row in &HEALTH_ROWS {
        let router = router();
        // Each row runs in its own network, so one row's rules cannot admit another's checks.
        let network = seed_network(
            &router,
            "172.25.0.0/16",
            &[
                ("172.25.0.0/23", "us-east-1a"),
                ("172.25.2.0/23", "us-east-1b"),
            ],
        )
        .await;
        let cluster = security_group(&router, &network.vpc, "nlbhc-cluster", "the instances").await;
        let balancer_group =
            security_group(&router, &network.vpc, "nlbhc-balancer", "the load balancer").await;
        // Every row's instances admit THEMSELVES, the self-reference a cluster security group
        // carries, which admits no check from a load balancer outside the group.
        ec2(
            &router,
            "AuthorizeSecurityGroupIngress",
            &[
                ("GroupId", cluster.as_str()),
                ("IpPermissions.1.IpProtocol", "-1"),
                ("IpPermissions.1.Groups.1.GroupId", cluster.as_str()),
            ],
        )
        .await;
        match row.admit {
            Admit::Nothing => {}
            Admit::SubnetRange => {
                ec2(
                    &router,
                    "AuthorizeSecurityGroupIngress",
                    &[
                        ("GroupId", cluster.as_str()),
                        ("IpPermissions.1.IpProtocol", "tcp"),
                        ("IpPermissions.1.FromPort", row.port),
                        ("IpPermissions.1.ToPort", row.port),
                        ("IpPermissions.1.IpRanges.1.CidrIp", network.subnets[0].1.as_str()),
                    ],
                )
                .await;
            }
            Admit::BalancerGroup => {
                ec2(
                    &router,
                    "AuthorizeSecurityGroupIngress",
                    &[
                        ("GroupId", cluster.as_str()),
                        ("IpPermissions.1.IpProtocol", "tcp"),
                        ("IpPermissions.1.FromPort", row.port),
                        ("IpPermissions.1.ToPort", row.port),
                        ("IpPermissions.1.Groups.1.GroupId", balancer_group.as_str()),
                    ],
                )
                .await;
            }
        }
        launch_template(&router, "nlbhc-template", &[cluster.as_str()]).await;

        let target_group = element(
            &elbv2(
                &router,
                "CreateTargetGroup",
                &[
                    ("Name", "nlbhc-tg"),
                    ("Protocol", row.protocol),
                    ("Port", row.port),
                    ("VpcId", network.vpc.as_str()),
                    ("TargetType", "instance"),
                    // The schedule the recorded target group was configured with, which is
                    // what decides how many consecutive failed checks a verdict takes.
                    ("HealthCheckIntervalSeconds", "10"),
                    ("HealthCheckTimeoutSeconds", "5"),
                    ("UnhealthyThresholdCount", "2"),
                ],
            )
            .await,
            "TargetGroupArn",
        );
        let mut balancer_params: Vec<(&str, &str)> = vec![
            ("Name", "nlbhc-lb"),
            ("Type", row.lb_type),
            ("Scheme", "internal"),
            ("Subnets.member.1", network.subnets[0].0.as_str()),
        ];
        if row.lb_type == "application" {
            // An Application Load Balancer is placed in at least two zones and always
            // carries security groups; a Network Load Balancer created without one is the
            // row this suite was filed for.
            balancer_params.push(("Subnets.member.2", network.subnets[1].0.as_str()));
            balancer_params.push(("SecurityGroups.member.1", balancer_group.as_str()));
        }
        let balancer = element(
            &elbv2(&router, "CreateLoadBalancer", &balancer_params).await,
            "LoadBalancerArn",
        );
        elbv2(
            &router,
            "CreateListener",
            &[
                ("LoadBalancerArn", balancer.as_str()),
                ("Protocol", row.protocol),
                ("Port", row.port),
                ("DefaultActions.member.1.Type", "forward"),
                ("DefaultActions.member.1.TargetGroupArn", target_group.as_str()),
            ],
        )
        .await;
        // The load balancer of a Network Load Balancer row carries no security group at all,
        // which is what the row's rule is about: read it back from the service that holds it.
        let carried = opt_element(
            &elbv2(
                &router,
                "DescribeLoadBalancers",
                &[("LoadBalancerArns.member.1", balancer.as_str())],
            )
            .await,
            "SecurityGroups",
        )
        .unwrap_or_default();
        assert_eq!(
            carried.contains(&balancer_group),
            row.lb_type == "application",
            "case {:?}: expected the load balancer to carry its own security group only as an \
             application load balancer, got {carried:?}",
            row.case
        );

        asg(
            &router,
            "CreateAutoScalingGroup",
            &[
                ("AutoScalingGroupName", "nlbhc-group"),
                ("MinSize", "1"),
                ("MaxSize", "1"),
                ("DesiredCapacity", "1"),
                ("LaunchTemplate.LaunchTemplateName", "nlbhc-template"),
                ("VPCZoneIdentifier", network.subnets[0].0.as_str()),
                ("TargetGroupARNs.member.1", target_group.as_str()),
                // The group's own health checks are Amazon EC2's, so the verdict asserted
                // below is the load balancer's alone.
                ("HealthCheckType", "EC2"),
            ],
        )
        .await;

        // The verdict is a number of CHECKS away, all of them clock-owed: the row waits for
        // the state it expects rather than for a span, and reports what it last read.
        let deadline = Instant::now() + DEADLINE;
        let reported = loop {
            let reported = target_health(&router, &target_group).await;
            let settled = reported.len() == 1 && reported[0].state == row.state;
            if settled || Instant::now() >= deadline {
                break reported;
            }
            tokio::time::sleep(POLL).await;
        };
        assert_eq!(
            reported.len(),
            1,
            "case {:?}: the target group must hold exactly the group's instance, got \
             {reported:?}",
            row.case
        );
        assert_eq!(
            (reported[0].state.as_str(), reported[0].reason.as_str()),
            (row.state, row.reason),
            "case {:?}: expected the target to settle on {}/{:?}, got {reported:?}",
            row.case,
            row.state,
            row.reason
        );
    }
}

/// One row of the instance-health table: a group over a target group whose checks either
/// arrive or do not, checking its instances the way the row says.
struct InstanceHealthRow {
    case: &'static str,
    /// The target group and the group that registers into it, and the listener port that
    /// makes the target group one the load balancer forwards to.
    target_group_name: &'static str,
    group_name: &'static str,
    listener_port: &'static str,
    /// `HealthCheckType` of the Auto Scaling group: whether the load balancer's verdict is
    /// this group's instance health at all.
    health_check_type: &'static str,
    /// Whether the instances' own security group admits the load balancer's check, which is
    /// what decides the verdict Elastic Load Balancing reaches.
    admits_the_check: bool,
    /// The `TargetHealth$State` the registration settles on.
    target_state: &'static str,
    /// The `Instance$HealthStatus` the group must then report for that instance.
    instance_health: &'static str,
}

/// The rows span what the rule generalises over: the verdict the load balancer reaches
/// (`healthy`, `unhealthy`) and whether the group takes that verdict as its instances' health
/// at all (`ELB` against `EC2`, the type that "cannot be disabled" and reads no load
/// balancer).
const INSTANCE_HEALTH_ROWS: [InstanceHealthRow; 3] = [
    InstanceHealthRow {
        case: "an ELB-health-checked group whose instance answers the load balancer's checks",
        target_group_name: "asgih-open",
        group_name: "asgih-open-group",
        listener_port: "80",
        health_check_type: "ELB",
        admits_the_check: true,
        target_state: "healthy",
        instance_health: "Healthy",
    },
    InstanceHealthRow {
        case: "an ELB-health-checked group whose instance answers none of them",
        target_group_name: "asgih-closed",
        group_name: "asgih-closed-group",
        listener_port: "8080",
        health_check_type: "ELB",
        admits_the_check: false,
        target_state: "unhealthy",
        instance_health: "Unhealthy",
    },
    InstanceHealthRow {
        case: "an EC2-health-checked group over the same unanswered checks",
        target_group_name: "asgih-ec2",
        group_name: "asgih-ec2-group",
        listener_port: "8081",
        health_check_type: "EC2",
        admits_the_check: false,
        target_state: "unhealthy",
        instance_health: "Healthy",
    },
];

/// RULE: a group whose `HealthCheckType` names `ELB` takes the load balancer's verdict as the
/// health of its instances — an instance whose target Elastic Load Balancing reports
/// `unhealthy` is reported `Unhealthy` and one whose target it reports `healthy` is reported
/// `Healthy` — while a group that checks its instances any other way reports the same
/// unanswered target as `Healthy`. The status is DERIVED, so it follows the verdict back: a
/// rule that lets the checks arrive again returns the instance to `Healthy`.
#[tokio::test]
async fn an_elb_health_checked_groups_instance_health_follows_the_load_balancers_verdict() {
    let router = router();
    // One network for every row: what separates the rows is the inbound rule of the security
    // group each row's instances carry, so each flavour of group is created once and the rows
    // that need it share it.
    let network = seed_network(
        &router,
        "10.76.0.0/16",
        &[("10.76.1.0/24", "us-east-1a"), ("10.76.2.0/24", "us-east-1b")],
    )
    .await;
    let balancer_group =
        security_group(&router, &network.vpc, "asgih-balancer", "the load balancer").await;
    let open = security_group(&router, &network.vpc, "asgih-open", "instances that answer").await;
    let closed =
        security_group(&router, &network.vpc, "asgih-closed", "instances that do not").await;
    // The open group admits the load balancer's own group on every port a row checks; the
    // closed one admits only ITSELF, the self-reference a cluster security group carries,
    // which admits no check from a load balancer outside the group.
    for port in INSTANCE_HEALTH_ROWS.map(|row| row.listener_port) {
        ec2(
            &router,
            "AuthorizeSecurityGroupIngress",
            &[
                ("GroupId", open.as_str()),
                ("IpPermissions.1.IpProtocol", "tcp"),
                ("IpPermissions.1.FromPort", port),
                ("IpPermissions.1.ToPort", port),
                ("IpPermissions.1.Groups.1.GroupId", balancer_group.as_str()),
            ],
        )
        .await;
    }
    ec2(
        &router,
        "AuthorizeSecurityGroupIngress",
        &[
            ("GroupId", closed.as_str()),
            ("IpPermissions.1.IpProtocol", "-1"),
            ("IpPermissions.1.Groups.1.GroupId", closed.as_str()),
        ],
    )
    .await;
    launch_template(&router, "asgih-open-template", &[open.as_str()]).await;
    launch_template(&router, "asgih-closed-template", &[closed.as_str()]).await;
    let balancer = element(
        &elbv2(
            &router,
            "CreateLoadBalancer",
            &[
                ("Name", "asgih-alb"),
                ("Scheme", "internal"),
                ("Subnets.member.1", network.subnets[0].0.as_str()),
                ("Subnets.member.2", network.subnets[1].0.as_str()),
                ("SecurityGroups.member.1", balancer_group.as_str()),
            ],
        )
        .await,
        "LoadBalancerArn",
    );

    // Every row's target group, listener and group are created FIRST, so the checks of all of
    // them run on the one clock rather than one row's waiting for the next row's setup.
    let mut target_groups: Vec<String> = Vec::new();
    for row in &INSTANCE_HEALTH_ROWS {
        let target_group = element(
            &elbv2(
                &router,
                "CreateTargetGroup",
                &[
                    ("Name", row.target_group_name),
                    ("Protocol", "HTTP"),
                    ("Port", row.listener_port),
                    ("VpcId", network.vpc.as_str()),
                    ("TargetType", "instance"),
                    ("HealthCheckIntervalSeconds", CHECK_INTERVAL_SECONDS),
                    ("UnhealthyThresholdCount", CHECK_THRESHOLD),
                    ("HealthyThresholdCount", CHECK_THRESHOLD),
                ],
            )
            .await,
            "TargetGroupArn",
        );
        elbv2(
            &router,
            "CreateListener",
            &[
                ("LoadBalancerArn", balancer.as_str()),
                ("Protocol", "HTTP"),
                ("Port", row.listener_port),
                ("DefaultActions.member.1.Type", "forward"),
                ("DefaultActions.member.1.TargetGroupArn", target_group.as_str()),
            ],
        )
        .await;
        let template = if row.admits_the_check {
            "asgih-open-template"
        } else {
            "asgih-closed-template"
        };
        asg(
            &router,
            "CreateAutoScalingGroup",
            &[
                ("AutoScalingGroupName", row.group_name),
                ("MinSize", "1"),
                ("MaxSize", "1"),
                ("DesiredCapacity", "1"),
                ("LaunchTemplate.LaunchTemplateName", template),
                ("VPCZoneIdentifier", network.subnets[0].0.as_str()),
                ("TargetGroupARNs.member.1", target_group.as_str()),
                ("HealthCheckType", row.health_check_type),
            ],
        )
        .await;
        target_groups.push(target_group);
    }

    for (row, target_group) in INSTANCE_HEALTH_ROWS.iter().zip(&target_groups) {
        // The launch, the registration and the checks are all clock-owed: the row waits for
        // the CONDITION — the verdict it expects, with the instance health that follows it —
        // rather than for a span.
        let deadline = Instant::now() + DEADLINE;
        let (reported, held) = loop {
            let reported = target_health(&router, target_group).await;
            let held = group_instances(&router, row.group_name).await;
            let settled = reported.len() == 1
                && reported[0].state == row.target_state
                && held.len() == 1
                && held[0].1 == row.instance_health;
            if settled || Instant::now() >= deadline {
                break (reported, held);
            }
            tokio::time::sleep(POLL).await;
        };
        assert_eq!(
            reported.len(),
            1,
            "case {:?}: the target group must hold exactly the group's instance, got \
             {reported:?}",
            row.case
        );
        assert_eq!(
            reported[0].state, row.target_state,
            "case {:?}: expected the target to settle on {}, got {reported:?}",
            row.case, row.target_state
        );
        assert_eq!(
            held,
            vec![("InService".to_string(), row.instance_health.to_string())],
            "case {:?}: expected the group to report its instance {} beside a {} target, got \
             {held:?}",
            row.case,
            row.instance_health,
            row.target_state
        );
    }

    // The status is DERIVED from the verdict rather than latched: once a rule lets the
    // load balancer's checks arrive, the target the group was reporting `Unhealthy` for
    // returns to `Healthy` with no call to Auto Scaling in between.
    let recovering = &INSTANCE_HEALTH_ROWS[1];
    ec2(
        &router,
        "AuthorizeSecurityGroupIngress",
        &[
            ("GroupId", closed.as_str()),
            ("IpPermissions.1.IpProtocol", "tcp"),
            ("IpPermissions.1.FromPort", recovering.listener_port),
            ("IpPermissions.1.ToPort", recovering.listener_port),
            ("IpPermissions.1.Groups.1.GroupId", balancer_group.as_str()),
        ],
    )
    .await;
    let deadline = Instant::now() + DEADLINE;
    let held = loop {
        let held = group_instances(&router, recovering.group_name).await;
        if held.len() == 1 && held[0].1 == "Healthy" || Instant::now() >= deadline {
            break held;
        }
        tokio::time::sleep(POLL).await;
    };
    assert_eq!(
        held,
        vec![("InService".to_string(), "Healthy".to_string())],
        "case {:?}: an instance whose checks arrive again must be reported Healthy, got \
         {held:?}",
        recovering.case
    );
}
