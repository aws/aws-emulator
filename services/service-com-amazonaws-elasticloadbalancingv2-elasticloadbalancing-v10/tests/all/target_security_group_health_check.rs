// SPDX-License-Identifier: CC0-1.0

//! Whether a load balancer's HEALTH CHECK of a registered target can ARRIVE is decided by the
//! security groups of the resource the registration NAMES.
//!
//! The rule asserted here: **a health check is an ordinary inbound connection the load
//! balancer opens to the target on the group's effective health-check port, so it is admitted
//! only by an inbound rule of a security group governing that target which covers that port
//! and names the load balancer as its source — the groups of the network interfaces the
//! registered resource holds, whichever way the registration names it (an instance, or an
//! address an interface of that instance carries). A target no such rule admits fails every
//! check and settles on `unhealthy` with the reason a check nothing answered is reported
//! under, never passing through `healthy`; revoking the one rule that admitted it takes a
//! healthy target to that verdict, and authorizing it again brings the target back.**
//!
//! The rows differ along what the rule generalises over: how the registration names the
//! target (its instance id, its address), and what the governing group's inbound rules say
//! (the load balancer's own group on the checked port, that group on another port, the
//! checked port from an unrelated group, an address range holding the load balancer's own
//! addresses, every protocol, and no rule at all).
//!
//! Everything is driven over the wire through the two services' own APIs — EC2 is mounted
//! beside this one on the suite's own engine, so the VPC, the subnets, the security groups,
//! their rules and the instances are the records EC2's own readers answer with, and the
//! verdict is read back through `DescribeTargetHealth`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use smithy_server_runtime::MultiServiceRouter;

use crate::listener_rules::create_listener;
use crate::provisioning_loop::{action, call, encode, first_text, health_block, members, p, REGION};

/// How long a row waits for the verdict the group's own health-check cadence reaches, and how
/// often it looks. Generous on purpose — many times the cadence below — because the deadline
/// is only here so a failure is a failure rather than a hang.
const DEADLINE: Duration = Duration::from_secs(120);
const POLL: Duration = Duration::from_millis(50);

/// The health-check cadence every target group below is created with: a verdict takes a
/// threshold of consecutive checks, each one interval after the last, so a suite that waits
/// for one asks for the shortest interval `HealthCheckIntervalSeconds` admits ("the range is
/// 5-300").
const INTERVAL_SECONDS: &str = "5";

/// The API version an ec2Query request carries.
const EC2_API_VERSION: &str = "2016-11-15";

/// An AMI id a launch names; `RunInstances` only has to accept it, and this suite asserts
/// nothing about the image.
const AMI: &str = "ami-0123456789abcdef0";

/// The network every row runs in: one VPC, and two subnets of it in different zones (which is
/// what an Application Load Balancer is placed across). The zones belong to the Region every
/// request of this crate's suites is signed for.
const VPC_CIDR: &str = "10.90.0.0/16";
const SUBNETS: [(&str, &str); 2] = [
    ("10.90.1.0/24", "us-west-2a"),
    ("10.90.2.0/24", "us-west-2b"),
];

/// The port the target groups below receive traffic on, which is also the port their health
/// check connects to: no row names a `HealthCheckPort` of its own.
const TRAFFIC_PORT: i32 = 80;

/// A port no target group of this suite forwards to, for a rule that admits the load balancer
/// somewhere other than the checked port.
const OTHER_PORT: i32 = 8080;

/// The `TargetHealth$State` / `TargetHealth$Reason` a target whose checks nothing answers
/// settles on, and the state a reachable one reaches.
const UNHEALTHY: (&str, &str) = ("unhealthy", "Target.Timeout");
const HEALTHY: (&str, &str) = ("healthy", "");

/// A router carrying this service and EC2 over one shared engine: the account's network is
/// EC2's, so the rule under test needs both mounted.
///
/// `pub(crate)` because the other suite of this binary whose rule is about an EC2-owned fact
/// (`all/internet_facing_placement.rs`) needs the same two services on one engine: one
/// builder for the crate, so no suite can drift on how they are mounted.
pub(crate) fn emulator_with_ec2() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// Send one ec2Query request: the action, the API version and `params`, addressed to EC2's own
/// signing name.
pub(crate) async fn ec2(
    router: &MultiServiceRouter,
    name: &str,
    params: &[(String, String)],
) -> (u16, String) {
    let mut body = format!("Action={name}&Version={EC2_API_VERSION}");
    for (key, value) in params {
        body.push('&');
        body.push_str(&encode(key));
        body.push('=');
        body.push_str(&encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://ec2.{REGION}.amazonaws.com/"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/ec2/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).to_string(),
    )
}

/// One ec2Query request that must be accepted, as its response body.
pub(crate) async fn ec2_ok(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    params: &[(String, String)],
) -> String {
    let (status, body) = ec2(router, name, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from EC2 {name}, got {status} body {body}"
    );
    body
}

/// The text of the element `tag` of an EC2 answer, which must be there.
pub(crate) fn reported(case: &str, body: &str, tag: &str) -> String {
    first_text(body, tag)
        .unwrap_or_else(|| panic!("case {case:?}: no {tag} in {body}"))
}

/// The account's network: the VPC, its subnets, and the security group the load balancer is
/// associated with.
struct Network {
    vpc: String,
    subnets: Vec<String>,
    load_balancer_group: String,
}

/// Create that network through EC2's own API.
///
/// The VPC is given the internet gateway AWS requires of an internet-facing load balancer —
/// which is what the load balancer below is, naming no scheme — attached through EC2's own
/// `AttachInternetGateway`; no route is added to any route table, which the rule about a
/// public placement does not read (`all/internet_facing_placement.rs`).
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
    let load_balancer_group = security_group(router, &vpc, "sgcheck-alb").await;
    Network {
        vpc,
        subnets,
        load_balancer_group,
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
            // `CreateSecurityGroupRequest$Description` travels under its own `xmlName`,
            // `GroupDescription`, which is the name an ec2Query body carries it as.
            p("GroupDescription", "a group of this suite"),
        ],
    )
    .await;
    reported("setup", &created, "groupId")
}

/// Launch one instance in `subnet`, in the security group `group`, at `address`.
async fn run_instance(
    router: &MultiServiceRouter,
    case: &str,
    subnet: &str,
    group: &str,
    address: &str,
) -> String {
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
        ],
    )
    .await;
    let instance = reported(case, &launched, "instanceId");
    assert!(
        instance.starts_with("i-"),
        "case {case:?}: expected an instance id, got {instance:?}"
    );
    instance
}

/// Wait until EC2 reports `instance` as `running`.
///
/// A launch answers with the instance `pending`, and the move to `running` is the one the
/// clock owes it: a client waits for that state before treating the instance as a target it
/// can be health-checked at, and so does this suite — a target whose workload is not running
/// yet answers nothing for a reason that is not the firewall under test.
async fn running(router: &MultiServiceRouter, case: &str, instance: &str) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let reported = ec2_ok(
            router,
            case,
            "DescribeInstances",
            &[p("InstanceId.1", instance)],
        )
        .await;
        let is_running = reported.contains("<name>running</name>");
        if is_running || Instant::now() >= deadline {
            assert!(
                is_running,
                "case {case:?}: expected EC2 to report {instance} running, got {reported}"
            );
            return;
        }
        ::tokio::time::sleep(POLL).await;
    }
}

/// Where one inbound rule grants access FROM, as `AuthorizeSecurityGroupIngress` names a
/// source: "a CIDR range, a security group, or a prefix list".
#[derive(Clone, Copy)]
enum Source {
    /// The load balancer's own security group — the source a rule for load-balancer traffic
    /// names.
    LoadBalancer,
    /// A security group of the account no load balancer is associated with.
    Unrelated,
    /// An address range.
    Cidr(&'static str),
}

/// One inbound rule of the group governing a target: its protocol, its port range (absent for
/// a rule that names none), and the one source it grants.
#[derive(Clone, Copy)]
struct InboundRule {
    protocol: &'static str,
    ports: Option<(i32, i32)>,
    source: Source,
}

/// The form fields that AUTHORIZE or REVOKE `rule` on `group` — one request shape for both,
/// since "to revoke a rule, the values that you specify must match exactly".
fn rule_fields(
    group: &str,
    rule: &InboundRule,
    network: &Network,
    unrelated: &str,
) -> Vec<(String, String)> {
    let mut fields = vec![
        p("GroupId", group),
        p("IpPermissions.1.IpProtocol", rule.protocol),
    ];
    if let Some((first, last)) = rule.ports {
        fields.push(p("IpPermissions.1.FromPort", &first.to_string()));
        fields.push(p("IpPermissions.1.ToPort", &last.to_string()));
    }
    match rule.source {
        Source::LoadBalancer => fields.push(p(
            "IpPermissions.1.Groups.1.GroupId",
            &network.load_balancer_group,
        )),
        Source::Unrelated => fields.push(p("IpPermissions.1.Groups.1.GroupId", unrelated)),
        Source::Cidr(cidr) => fields.push(p("IpPermissions.1.IpRanges.1.CidrIp", cidr)),
    }
    fields
}

/// Create the load balancer every row's listeners belong to: an Application Load Balancer
/// across the network's subnets, associated with the network's own security group.
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

/// Create a target group of `target_type` in the network's VPC and answer its ARN.
async fn target_group(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    target_type: &str,
    vpc: &str,
) -> String {
    let mut form = action("CreateTargetGroup");
    form.push(p("Name", name));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", &TRAFFIC_PORT.to_string()));
    form.push(p("TargetType", target_type));
    form.push(p("VpcId", vpc));
    form.push(p("HealthCheckIntervalSeconds", INTERVAL_SECONDS));
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

/// Register `id` with the target group `arn` names.
async fn register(router: &MultiServiceRouter, case: &str, arn: &str, id: &str) {
    let mut form = action("RegisterTargets");
    form.push(p("TargetGroupArn", arn));
    form.push(p("Targets.member.1.Id", id));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from RegisterTargets, got {} body {}",
        reply.status, reply.body
    );
}

/// `(state, reason)` for every registration the target group `arn` reports.
async fn health(router: &MultiServiceRouter, case: &str, arn: &str) -> Vec<(String, String)> {
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
            )
        })
        .collect()
}

/// The health of `arn`'s registrations once every one of them reports `wanted`, and every
/// state reported on the way there — so a row can assert what it must never have reported.
async fn health_reaching(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
    wanted: &str,
) -> (Vec<(String, String)>, Vec<String>) {
    let deadline = Instant::now() + DEADLINE;
    let mut seen: Vec<String> = Vec::new();
    loop {
        let found = health(router, case, arn).await;
        for (state, _) in &found {
            if seen.last().map(String::as_str) != Some(state.as_str()) {
                seen.push(state.clone());
            }
        }
        let settled = !found.is_empty() && found.iter().all(|(state, _)| state == wanted);
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

/// One row: how the registration names the target, the inbound rule the group governing it
/// carries, and the verdict that implies.
struct Row {
    case: &'static str,
    /// The target group's name, which is also the name of the security group of its target.
    name: &'static str,
    /// `instance` to register the target by its instance id, `ip` by its address.
    target_type: &'static str,
    /// The one inbound rule the target's security group carries, or `None` for a group with
    /// no inbound rule at all.
    rule: Option<InboundRule>,
    /// The `(state, reason)` every registration of the group must reach.
    reaches: (&'static str, &'static str),
}

/// An address range that holds no address of this network, so no address a load balancer of it
/// holds is inside it.
const FOREIGN_RANGE: &str = "192.168.0.0/16";

/// The rows, in creation order. Each gets a security group, an instance, a target group and a
/// listener of its own.
const ROWS: [Row; 9] = [
    Row {
        case: "an instance whose group admits the load balancer on the checked port",
        name: "sgcheck-admitted",
        target_type: "instance",
        rule: Some(InboundRule {
            protocol: "tcp",
            ports: Some((TRAFFIC_PORT, TRAFFIC_PORT)),
            source: Source::LoadBalancer,
        }),
        reaches: HEALTHY,
    },
    Row {
        case: "the recorded configuration: an instance whose group carries no inbound rule",
        name: "sgcheck-norule",
        target_type: "instance",
        rule: None,
        reaches: UNHEALTHY,
    },
    Row {
        case: "an instance whose group admits the load balancer on another port",
        name: "sgcheck-otherport",
        target_type: "instance",
        rule: Some(InboundRule {
            protocol: "tcp",
            ports: Some((OTHER_PORT, OTHER_PORT)),
            source: Source::LoadBalancer,
        }),
        reaches: UNHEALTHY,
    },
    Row {
        case: "an instance whose group admits the checked port from an unrelated group",
        name: "sgcheck-othergroup",
        target_type: "instance",
        rule: Some(InboundRule {
            protocol: "tcp",
            ports: Some((TRAFFIC_PORT, TRAFFIC_PORT)),
            source: Source::Unrelated,
        }),
        reaches: UNHEALTHY,
    },
    Row {
        case: "an instance whose group admits every protocol from the load balancer",
        name: "sgcheck-allprotocols",
        target_type: "instance",
        rule: Some(InboundRule {
            protocol: "-1",
            ports: None,
            source: Source::LoadBalancer,
        }),
        reaches: HEALTHY,
    },
    Row {
        case: "an instance whose group admits the checked port from the VPC's own range",
        name: "sgcheck-cidr",
        target_type: "instance",
        rule: Some(InboundRule {
            protocol: "tcp",
            ports: Some((TRAFFIC_PORT, TRAFFIC_PORT)),
            source: Source::Cidr(VPC_CIDR),
        }),
        reaches: HEALTHY,
    },
    Row {
        case: "an instance whose group admits the checked port from a range holding no \
               address of the load balancer",
        name: "sgcheck-foreigncidr",
        target_type: "instance",
        rule: Some(InboundRule {
            protocol: "tcp",
            ports: Some((TRAFFIC_PORT, TRAFFIC_PORT)),
            source: Source::Cidr(FOREIGN_RANGE),
        }),
        reaches: UNHEALTHY,
    },
    Row {
        case: "the same workload registered by its address, admitted on the checked port",
        name: "sgcheck-addr-admitted",
        target_type: "ip",
        rule: Some(InboundRule {
            protocol: "tcp",
            ports: Some((TRAFFIC_PORT, TRAFFIC_PORT)),
            source: Source::LoadBalancer,
        }),
        reaches: HEALTHY,
    },
    Row {
        case: "the same workload registered by its address, with no inbound rule at all",
        name: "sgcheck-addr-norule",
        target_type: "ip",
        rule: None,
        reaches: UNHEALTHY,
    },
];

#[tokio::test]
async fn a_check_is_admitted_only_by_a_rule_naming_the_load_balancer_and_the_checked_port() {
    let router = emulator_with_ec2();
    let network = seed_network(&router).await;
    let unrelated = security_group(&router, &network.vpc, "sgcheck-unrelated").await;
    let load_balancer = create_alb(&router, "sgcheck-alb", &network).await;

    // One security group, one instance and one inbound rule per row: the workload each row's
    // registration names, and the rules governing traffic to it.
    let mut workloads: Vec<(String, String)> = Vec::new();
    for (index, row) in ROWS.iter().enumerate() {
        let guarding = security_group(&router, &network.vpc, row.name).await;
        let address = format!(
            "10.90.1.{}",
            20 + u16::try_from(index).expect("the row count fits a u16")
        );
        let instance = run_instance(
            &router,
            row.case,
            &network.subnets[0],
            &guarding,
            &address,
        )
        .await;
        if let Some(rule) = row.rule {
            ec2_ok(
                &router,
                row.case,
                "AuthorizeSecurityGroupIngress",
                &rule_fields(&guarding, &rule, &network, &unrelated),
            )
            .await;
        }
        workloads.push((instance, address));
    }

    // One target group per row, each with a listener forwarding to it — which is what makes
    // the group one the load balancer checks — and the row's own workload registered with it.
    let mut groups: Vec<String> = Vec::new();
    for (index, (row, (instance, address))) in ROWS.iter().zip(&workloads).enumerate() {
        running(&router, row.case, instance).await;
        let group = target_group(
            &router,
            row.case,
            row.name,
            row.target_type,
            &network.vpc,
        )
        .await;
        create_listener(
            &router,
            &load_balancer,
            &group,
            TRAFFIC_PORT + i32::try_from(index).expect("the row count fits an i32"),
        )
        .await;
        let target = if row.target_type == "instance" {
            instance
        } else {
            address
        };
        register(&router, row.case, &group, target).await;
        groups.push(group);
    }

    for (group, row) in groups.iter().zip(&ROWS) {
        let (found, seen) = health_reaching(&router, row.case, group, row.reaches.0).await;
        assert_eq!(
            found.len(),
            1,
            "case {:?}: expected one description per registration, got {found:?}",
            row.case
        );
        for (state, reason) in &found {
            assert_eq!(
                (state.as_str(), reason.as_str()),
                row.reaches,
                "case {:?}: expected {}/{}, got {state}/{reason}",
                row.case,
                row.reaches.0,
                row.reaches.1
            );
        }
        if row.reaches.0 != HEALTHY.0 {
            // A target no rule admits is answered by nothing from the first check on, so it
            // is never reported healthy on its way to the verdict.
            assert!(
                !seen.iter().any(|state| state == HEALTHY.0),
                "case {:?}: a target whose checks cannot arrive must never be reported \
                 healthy, got the states {seen:?}",
                row.case
            );
        }
    }
}

#[tokio::test]
async fn revoking_the_rule_that_admits_the_check_unseats_the_verdict_and_authorizing_restores_it() {
    let router = emulator_with_ec2();
    let network = seed_network(&router).await;
    let unrelated = security_group(&router, &network.vpc, "sgrevoke-unrelated").await;
    let guarding = security_group(&router, &network.vpc, "sgrevoke-target").await;
    let rule = InboundRule {
        protocol: "tcp",
        ports: Some((TRAFFIC_PORT, TRAFFIC_PORT)),
        source: Source::LoadBalancer,
    };
    let fields = rule_fields(&guarding, &rule, &network, &unrelated);

    let case = "an instance target whose admitting rule is revoked and authorized again";
    let instance = run_instance(
        &router,
        case,
        &network.subnets[0],
        &guarding,
        "10.90.1.60",
    )
    .await;
    ec2_ok(&router, case, "AuthorizeSecurityGroupIngress", &fields).await;
    running(&router, case, &instance).await;
    let load_balancer = create_alb(&router, "sgrevoke-alb", &network).await;
    let group = target_group(&router, case, "sgrevoke-tg", "instance", &network.vpc).await;
    create_listener(&router, &load_balancer, &group, TRAFFIC_PORT).await;
    register(&router, case, &group, &instance).await;

    // The target is reachable while the rule admitting the load balancer stands.
    health_reaching(&router, case, &group, HEALTHY.0).await;

    // Revoking that rule leaves the target's groups admitting no check of it, so the checks
    // stop being answered and the verdict moves once the group's threshold is reached.
    ec2_ok(&router, case, "RevokeSecurityGroupIngress", &fields).await;
    let (found, _) = health_reaching(&router, case, &group, UNHEALTHY.0).await;
    for (state, reason) in &found {
        assert_eq!(
            (state.as_str(), reason.as_str()),
            UNHEALTHY,
            "case {case:?}: after the revoke, expected {}/{}, got {state}/{reason}",
            UNHEALTHY.0,
            UNHEALTHY.1
        );
    }

    // Authorizing it again admits the checks, and the target comes back.
    ec2_ok(&router, case, "AuthorizeSecurityGroupIngress", &fields).await;
    let (found, _) = health_reaching(&router, case, &group, HEALTHY.0).await;
    for (state, reason) in &found {
        assert_eq!(
            (state.as_str(), reason.as_str()),
            HEALTHY,
            "case {case:?}: after the re-authorization, expected {}/{}, got {state}/{reason}",
            HEALTHY.0,
            HEALTHY.1
        );
    }
}
