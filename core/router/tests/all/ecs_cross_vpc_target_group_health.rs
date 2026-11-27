// SPDX-License-Identifier: CC0-1.0

//! An ECS task is reported HEALTHY by Elastic Load Balancing only in a target group whose
//! own network can carry the load balancer's health check to the task's address.
//!
//! RULE: a health check is a connection a load balancer node opens to the target's address
//! from the subnets the load balancer is placed in, so it only arrives if the VPC the target
//! group belongs to can route to that address — "the VPC in which the targets are located"
//! (`CreateTargetGroup$VpcId`), whose local route carries traffic to an address inside it and
//! whose accepted VPC peerings carry it to an address the peer holds. An address held in a VPC
//! the group's own neither holds nor is peered with answers no check of it: the target is
//! never reported `healthy` and settles on `unhealthy` with the reason the Application Load
//! Balancer words for a check nothing answered, while the same task registered in a group of a
//! VPC that CAN route to it converges to `healthy` with no reason code ("if the target state
//! is `healthy`, a reason code is not provided", `TargetHealth$Reason`).
//!
//! Three crates have to agree for that to hold, which is why this suite lives here: ECS
//! registers its `awsvpc` task at the private address of the interface it placed
//! (`LoadBalancer$targetGroupArn`), EC2 owns that interface and the VPC holding it, and
//! Elastic Load Balancing v2 reads where the address lives against the VPC of the group whose
//! check it is. Everything is driven through the real operations on one engine, so what is
//! asserted is what each service persisted.
//!
//! The rows differ along where the target group's VPC sits relative to the address: the task's
//! own VPC, a VPC an accepted peering joins to it, an isolated VPC of another address range,
//! and an isolated VPC whose address range COVERS the task's address — which is what pins the
//! rule to the network actually holding the address rather than to arithmetic on a CIDR block.
//! The recorded configuration — an internal load balancer and an `ip` target group in a
//! second, isolated VPC, fronting a service placed in the first — is the third row.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The ONE percent-encoder and element readers this test crate keeps for awsQuery suites
// (`all/query_wire.rs`: "Nothing new should write another one — call this module").
use crate::query_wire::{
    element, elements, form_encode, members_of, opt_element, target_health_of,
};

/// The region every request below is signed for.
const REGION: &str = "us-east-1";

/// The API versions an ec2Query and an awsQuery request carry.
const EC2_API_VERSION: &str = "2016-11-15";
const ELB_VERSION: &str = "2015-12-01";

/// The ECS cluster, task-definition family and container every row runs.
const CLUSTER: &str = "crossvpc";
const FAMILY: &str = "crossvpcfamily";
const CONTAINER: &str = "app";

/// The port the container listens on, which is also every row's traffic and health-check
/// port: this suite's rule is about the network, so the port is held fixed.
const CONTAINER_PORT: i64 = 80;

/// How long a row waits for the clock-owed moves it depends on — a load balancer's move to
/// `active`, a task's start, and the health checks the target group's own cadence runs
/// ([`HEALTH_CHECK_INTERVAL_SECONDS`]) — and how often it looks. Generous on purpose: the
/// deadline is only here so a failure is a failure rather than a hang.
const DEADLINE: Duration = Duration::from_secs(120);
const POLL: Duration = Duration::from_millis(100);

/// The `HealthCheckIntervalSeconds` every target group below is created with: a verdict takes
/// `UnhealthyThresholdCount` consecutive failing checks, each one interval after the last, so
/// a suite that waits for one asks for the shortest interval the member admits ("the range is
/// 5-300").
const HEALTH_CHECK_INTERVAL_SECONDS: &str = "5";

/// The `UnhealthyThresholdCount` and `HealthyThresholdCount` every target group below carries,
/// as the recorded configuration sets them.
const THRESHOLD_COUNT: &str = "2";

/// Where one row's target group — and the load balancer forwarding to it — is placed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Placement {
    /// The VPC the service's tasks are placed in.
    Own,
    /// A second, isolated VPC of another address range.
    Isolated,
    /// A third, isolated VPC whose address range covers the task's own address.
    Overlapping,
    /// A fourth VPC, of its own address range, which an ACCEPTED peering joins to the
    /// tasks' own.
    Peered,
}

/// One row: where the target group lives, and the verdict that implies for the task the
/// service registers into it.
struct Row {
    case: &'static str,
    /// Which network the group and its load balancer are in.
    placement: Placement,
    /// The target group's name, and the ECS service that binds it.
    group_name: &'static str,
    service_name: &'static str,
    /// The `TargetHealth$State` the registration must reach.
    state: &'static str,
    /// The `TargetHealth$Reason` values AWS gives for that state; empty for a state reported
    /// without a reason code.
    reasons: &'static [&'static str],
}

const ROWS: [Row; 4] = [
    Row {
        case: "a target group in the task's own VPC",
        placement: Placement::Own,
        group_name: "own-vpc",
        service_name: "ownvpc",
        state: "healthy",
        reasons: &[],
    },
    Row {
        case: "a target group in a VPC an accepted peering joins to the task's own",
        placement: Placement::Peered,
        group_name: "peered-vpc",
        service_name: "peeredvpc",
        state: "healthy",
        reasons: &[],
    },
    Row {
        case: "the recorded configuration: a target group in a second, isolated VPC",
        placement: Placement::Isolated,
        group_name: "other-vpc",
        service_name: "othervpc",
        state: "unhealthy",
        reasons: &["Target.Timeout", "Target.FailedHealthChecks"],
    },
    Row {
        case: "a target group in an isolated VPC whose address range covers the task's address",
        placement: Placement::Overlapping,
        group_name: "overlapping-vpc",
        service_name: "overlappingvpc",
        state: "unhealthy",
        reasons: &["Target.Timeout", "Target.FailedHealthChecks"],
    },
];

/// The address range and subnets of each network this suite stands up.
///
/// The overlapping network carries the SAME blocks as the task's own, which is what makes its
/// row a question about the VPC holding the address rather than about the block holding it.
const TASK_VPC_CIDR: &str = "10.90.0.0/16";
const ISOLATED_VPC_CIDR: &str = "10.91.0.0/16";
const PEERED_VPC_CIDR: &str = "10.92.0.0/16";
const SUBNET_ZONES: [&str; 2] = ["us-east-1a", "us-east-1b"];

/// The `VpcPeeringConnectionStateReason$Code` values the peering below passes through: the
/// state a request waits for its answer in, and the one it carries traffic in.
const PEERING_PENDING_ACCEPTANCE: &str = "pending-acceptance";
const PEERING_ACTIVE: &str = "active";

/// The subnet blocks of a VPC carrying `cidr`'s second octet.
fn subnet_cidrs(vpc_cidr: &str) -> [String; 2] {
    let prefix = vpc_cidr
        .split_once(".0.0/16")
        .map(|(prefix, _)| prefix.to_string())
        .unwrap_or_else(|| panic!("the suite's VPC blocks are /16s, got {vpc_cidr}"));
    [format!("{prefix}.1.0/24"), format!("{prefix}.2.0/24")]
}

/// One emulator carrying EC2, Elastic Load Balancing v2 and ECS over ONE shared engine,
/// exactly as the production router builds them.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    (router, engine)
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
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).to_string(),
    )
}

/// Drive one ec2Query action that must succeed, and answer its body.
async fn ec2(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> String {
    let (status, body) = send(router, "ec2", form(action, EC2_API_VERSION, params)).await;
    assert_eq!(
        status, 200,
        "setup: EC2 {action} must succeed, got {status} with body {body}"
    );
    body
}

/// Drive one awsQuery action against the ELBv2 model that must succeed, and answer its body.
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

/// One awsJson1_1 call to ECS that must succeed, answered with its parsed body.
async fn ecs_ok(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AmazonEC2ContainerServiceV20141113.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDECSTEST/20260101/{REGION}\
                 /ecs/aws4_request, SignedHeaders=host;x-amz-target, Signature=00"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed: Value = if payload.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&payload)
            .unwrap_or_else(|_| json!({ "__unparsed": String::from_utf8_lossy(&payload) }))
    };
    assert_eq!(
        status, 200,
        "setup: ECS {operation} answered {status} with {parsed}"
    );
    parsed
}

/// One network this suite stands up, as EC2 minted it.
struct Network {
    vpc: String,
    subnets: Vec<String>,
    /// The security group the load balancer placed in this VPC is associated with.
    load_balancer_group: String,
}

/// Create one network through EC2 itself, so every id asserted against is one EC2 minted.
///
/// Every VPC is given the internet gateway AWS requires of an internet-facing load balancer —
/// the scheme `CreateLoadBalancer` documents as its default — attached through EC2's own
/// `AttachInternetGateway`.
async fn seed_network(router: &MultiServiceRouter, name: &str, cidr: &str) -> Network {
    let vpc = element(
        &ec2(router, "CreateVpc", &[("CidrBlock", cidr)]).await,
        "vpcId",
    );
    let gateway = element(
        &ec2(router, "CreateInternetGateway", &[]).await,
        "internetGatewayId",
    );
    ec2(
        router,
        "AttachInternetGateway",
        &[
            ("InternetGatewayId", gateway.as_str()),
            ("VpcId", vpc.as_str()),
        ],
    )
    .await;
    let mut subnets = Vec::new();
    for (block, zone) in subnet_cidrs(cidr).iter().zip(SUBNET_ZONES) {
        subnets.push(element(
            &ec2(
                router,
                "CreateSubnet",
                &[
                    ("VpcId", vpc.as_str()),
                    ("CidrBlock", block.as_str()),
                    ("AvailabilityZone", zone),
                ],
            )
            .await,
            "subnetId",
        ));
    }
    // The route off the VPC, written through EC2's OWN operations, the way a client that
    // stands a network up writes it: a table of the VPC carrying the default route to that
    // gateway, associated with each subnet. ECS reads this path before it starts a task — a
    // placement with no route off the VPC cannot reach a public registry — so the records
    // asserted against are the ones EC2's own writes left behind.
    let route_table = element(
        &ec2(router, "CreateRouteTable", &[("VpcId", vpc.as_str())]).await,
        "routeTableId",
    );
    ec2(
        router,
        "CreateRoute",
        &[
            ("RouteTableId", route_table.as_str()),
            ("DestinationCidrBlock", "0.0.0.0/0"),
            ("GatewayId", gateway.as_str()),
        ],
    )
    .await;
    for subnet in &subnets {
        ec2(
            router,
            "AssociateRouteTable",
            &[
                ("RouteTableId", route_table.as_str()),
                ("SubnetId", subnet.as_str()),
            ],
        )
        .await;
    }
    let load_balancer_group = element(
        &ec2(
            router,
            "CreateSecurityGroup",
            &[
                ("GroupName", &format!("{name}-alb")),
                ("GroupDescription", "the load balancers own group"),
                ("VpcId", vpc.as_str()),
            ],
        )
        .await,
        "groupId",
    );
    Network {
        vpc,
        subnets,
        load_balancer_group,
    }
}

/// The security group every task below runs in: one inbound rule admitting the checked port
/// from the WHOLE address family, so no row's verdict can turn on a security group and the
/// network is the only thing left to decide it.
async fn task_security_group(router: &MultiServiceRouter, network: &Network) -> String {
    let group = element(
        &ec2(
            router,
            "CreateSecurityGroup",
            &[
                ("GroupName", "crossvpc-task"),
                ("GroupDescription", "the tasks own group"),
                ("VpcId", network.vpc.as_str()),
            ],
        )
        .await,
        "groupId",
    );
    ec2(
        router,
        "AuthorizeSecurityGroupIngress",
        &[
            ("GroupId", group.as_str()),
            ("IpPermissions.1.IpProtocol", "tcp"),
            ("IpPermissions.1.FromPort", "80"),
            ("IpPermissions.1.ToPort", "80"),
            ("IpPermissions.1.IpRanges.1.CidrIp", "0.0.0.0/0"),
        ],
    )
    .await;
    group
}

/// Create the cluster and the revision every row's service runs: an `awsvpc`,
/// Fargate-compatible revision whose one container listens on [`CONTAINER_PORT`].
async fn seed_ecs(router: &MultiServiceRouter) {
    ecs_ok(router, "CreateCluster", json!({ "clusterName": CLUSTER })).await;
    ecs_ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": FAMILY,
            "networkMode": "awsvpc",
            "cpu": "256",
            "memory": "512",
            "containerDefinitions": [{
                "name": CONTAINER,
                "image": "public.ecr.aws/nginx/nginx:latest",
                "memory": 128,
                "portMappings": [{ "containerPort": CONTAINER_PORT }],
            }],
        }),
    )
    .await;
}

/// Create the load balancer of one network, in that network's own subnets and security group,
/// and answer its ARN. Every one but the task's own is INTERNAL, as the recorded
/// configuration's second load balancer is.
async fn load_balancer(
    router: &MultiServiceRouter,
    name: &str,
    network: &Network,
    internal: bool,
) -> String {
    let mut params: Vec<(&str, &str)> = vec![
        ("Name", name),
        ("Subnets.member.1", network.subnets[0].as_str()),
        ("Subnets.member.2", network.subnets[1].as_str()),
        (
            "SecurityGroups.member.1",
            network.load_balancer_group.as_str(),
        ),
    ];
    if internal {
        params.push(("Scheme", "internal"));
    }
    element(
        &elbv2(router, "CreateLoadBalancer", &params).await,
        "LoadBalancerArn",
    )
}

/// Create the `ip` target group one row describes, in `network`'s VPC, health-checked on the
/// port the container listens on, and answer its ARN.
async fn target_group(router: &MultiServiceRouter, network: &Network, row: &Row) -> String {
    let created = elbv2(
        router,
        "CreateTargetGroup",
        &[
            ("Name", row.group_name),
            ("Protocol", "HTTP"),
            ("Port", "80"),
            ("TargetType", "ip"),
            ("VpcId", network.vpc.as_str()),
            ("HealthCheckProtocol", "HTTP"),
            ("HealthCheckPath", "/"),
            ("HealthCheckIntervalSeconds", HEALTH_CHECK_INTERVAL_SECONDS),
            ("HealthyThresholdCount", THRESHOLD_COUNT),
            ("UnhealthyThresholdCount", THRESHOLD_COUNT),
        ],
    )
    .await;
    opt_element(&created, "TargetGroupArn")
        .unwrap_or_else(|| panic!("case {:?}: no TargetGroupArn in {created}", row.case))
}

/// Create the ECS service one row describes: one task in the TASK network's first subnet, in
/// the shared task security group, bound to `group`.
async fn create_service(
    router: &MultiServiceRouter,
    tasks: &Network,
    row: &Row,
    group: &str,
    task_group: &str,
) {
    let created = ecs_ok(
        router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": row.service_name,
            "taskDefinition": FAMILY,
            "desiredCount": 1,
            "launchType": "FARGATE",
            "networkConfiguration": {
                "awsvpcConfiguration": {
                    "subnets": [tasks.subnets[0]],
                    "securityGroups": [task_group],
                    // The task's own public address, which is what makes the internet
                    // gateway its subnet routes through a path to the image's registry.
                    "assignPublicIp": "ENABLED",
                }
            },
            "loadBalancers": [{
                "targetGroupArn": group,
                "containerName": CONTAINER,
                "containerPort": CONTAINER_PORT,
            }],
        }),
    )
    .await;
    assert_eq!(
        created["service"]["loadBalancers"][0]["targetGroupArn"].as_str(),
        Some(group),
        "case {:?}: the stored service must carry the group the request named, got {created}",
        row.case
    );
}

/// The private IPv4 address of the one RUNNING task of `service_name`, once its scheduler has
/// placed it and the clock has started it.
async fn running_task_address(
    router: &MultiServiceRouter,
    case: &str,
    service_name: &str,
) -> String {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let described = ecs_ok(
            router,
            "DescribeServices",
            json!({ "cluster": CLUSTER, "services": [service_name] }),
        )
        .await;
        if described["services"][0]["runningCount"]
            .as_i64()
            .unwrap_or_default()
            == 1
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the service to run its one task, got {described}"
        );
        ::tokio::time::sleep(POLL).await;
    }
    let listed = ecs_ok(
        router,
        "ListTasks",
        json!({ "cluster": CLUSTER, "serviceName": service_name }),
    )
    .await;
    let arn = listed["taskArns"][0]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no task ARN in {listed}"))
        .to_string();
    let described = ecs_ok(
        router,
        "DescribeTasks",
        json!({ "cluster": CLUSTER, "tasks": [arn] }),
    )
    .await;
    let task = &described["tasks"][0];
    task["attachments"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|attachment| attachment["details"].as_array())
        .flatten()
        .find(|detail| detail["name"].as_str() == Some("privateIPv4Address"))
        .and_then(|detail| detail["value"].as_str())
        .unwrap_or_else(|| {
            panic!("case {case:?}: the task carries its interface's address, got {task}")
        })
        .to_string()
}

/// `(target id, state, reason, description)` for every registration the group reports.
async fn health(router: &MultiServiceRouter, arn: &str) -> Vec<(String, String, String, String)> {
    let body = elbv2(router, "DescribeTargetHealth", &[("TargetGroupArn", arn)]).await;
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
            (
                text(description, "Id"),
                text(health, "State"),
                text(health, "Reason"),
                text(health, "Description"),
            )
        })
        .collect()
}

/// The health of `arn`'s registrations once every one of them reports `wanted`, and every
/// state reported on the way there.
async fn health_reaching(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
    wanted: &str,
) -> (Vec<(String, String, String, String)>, Vec<String>) {
    let deadline = Instant::now() + DEADLINE;
    let mut seen: Vec<String> = Vec::new();
    loop {
        let reported = health(router, arn).await;
        for (_, state, ..) in &reported {
            if seen.last().map(String::as_str) != Some(state.as_str()) {
                seen.push(state.clone());
            }
        }
        let settled = !reported.is_empty() && reported.iter().all(|(_, state, ..)| state == wanted);
        if settled || Instant::now() >= deadline {
            assert!(
                settled,
                "case {case:?}: expected every registration to reach {wanted}, got {reported:?}"
            );
            return (reported, seen);
        }
        ::tokio::time::sleep(POLL).await;
    }
}

/// The `LoadBalancerState$Code` of `arn` once the clock has brought it into service — the
/// gate a client that deploys a load balancer waits on before it reads anything behind it.
async fn load_balancer_state(router: &MultiServiceRouter, case: &str, arn: &str) -> String {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let body = elbv2(
            router,
            "DescribeLoadBalancers",
            &[("LoadBalancerArns.member.1", arn)],
        )
        .await;
        let code = opt_element(&body, "Code").unwrap_or_default();
        if code == "active" || Instant::now() >= deadline {
            assert_eq!(
                code, "active",
                "case {case:?}: expected the load balancer to come into service, got {body}"
            );
            return code;
        }
        ::tokio::time::sleep(POLL).await;
    }
}

/// Peer `one` with `other` through EC2's own operations and leave the connection ACTIVE: the
/// create takes the request, the clock brings it to the state it waits for its answer in, and
/// the accept is what joins the two networks ("a VPC peering connection is a networking
/// connection between two VPCs that enables you to route traffic between them").
async fn peer_networks(router: &MultiServiceRouter, one: &Network, other: &Network) {
    let created = ec2(
        router,
        "CreateVpcPeeringConnection",
        &[
            ("VpcId", one.vpc.as_str()),
            ("PeerVpcId", other.vpc.as_str()),
        ],
    )
    .await;
    let connection = element(&created, "vpcPeeringConnectionId");
    // The request reaches the state it can be answered in on the clock's own span, so it is
    // polled for rather than waited out.
    peering_reaching(router, &connection, PEERING_PENDING_ACCEPTANCE).await;
    ec2(
        router,
        "AcceptVpcPeeringConnection",
        &[("VpcPeeringConnectionId", connection.as_str())],
    )
    .await;
    peering_reaching(router, &connection, PEERING_ACTIVE).await;
}

/// Wait for `connection` to report the `VpcPeeringConnectionStateReason$Code` `wanted`.
async fn peering_reaching(router: &MultiServiceRouter, connection: &str, wanted: &str) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let body = ec2(
            router,
            "DescribeVpcPeeringConnections",
            &[("VpcPeeringConnectionId.1", connection)],
        )
        .await;
        let code = opt_element(&body, "code").unwrap_or_default();
        if code == wanted || Instant::now() >= deadline {
            assert_eq!(
                code, wanted,
                "expected the peering {connection} to reach {wanted}, got {body}"
            );
            return;
        }
        ::tokio::time::sleep(POLL).await;
    }
}

#[tokio::test]
async fn an_ecs_task_is_healthy_only_where_its_target_groups_network_reaches_it() {
    let (router, _engine) = emulator();
    let tasks = seed_network(&router, "crossvpc-task", TASK_VPC_CIDR).await;
    let isolated = seed_network(&router, "crossvpc-isolated", ISOLATED_VPC_CIDR).await;
    // A third network carrying the SAME address range as the task's own: its row is what
    // makes the rule a question about which VPC HOLDS the address.
    let overlapping = seed_network(&router, "crossvpc-overlapping", TASK_VPC_CIDR).await;
    // And a fourth the tasks' own network is JOINED to, which is what a VPC peering connection
    // does: an address one of them holds is one the other can route to.
    let peered = seed_network(&router, "crossvpc-peered", PEERED_VPC_CIDR).await;
    peer_networks(&router, &tasks, &peered).await;
    let task_group = task_security_group(&router, &tasks).await;
    seed_ecs(&router).await;

    let balancers = [
        (
            Placement::Own,
            load_balancer(&router, "crossvpc-task-alb", &tasks, false).await,
        ),
        (
            Placement::Isolated,
            load_balancer(&router, "crossvpc-isolated-alb", &isolated, true).await,
        ),
        (
            Placement::Overlapping,
            load_balancer(&router, "crossvpc-overlapping-alb", &overlapping, true).await,
        ),
        (
            Placement::Peered,
            load_balancer(&router, "crossvpc-peered-alb", &peered, true).await,
        ),
    ];

    let mut groups: Vec<String> = Vec::new();
    for row in &ROWS {
        let network = match row.placement {
            Placement::Own => &tasks,
            Placement::Isolated => &isolated,
            Placement::Overlapping => &overlapping,
            Placement::Peered => &peered,
        };
        let balancer = balancers
            .iter()
            .find(|(placement, _)| *placement == row.placement)
            .map(|(_, arn)| arn.as_str())
            .unwrap_or_else(|| panic!("case {:?}: no load balancer for it", row.case));
        let group = target_group(&router, network, row).await;
        // A listener forwarding to the group is what makes it one the load balancer checks —
        // and what lets an ECS service bind it at all.
        elbv2(
            &router,
            "CreateListener",
            &[
                ("LoadBalancerArn", balancer),
                ("Protocol", "HTTP"),
                ("Port", "80"),
                ("DefaultActions.member.1.Type", "forward"),
                ("DefaultActions.member.1.TargetGroupArn", group.as_str()),
            ],
        )
        .await;
        create_service(&router, &tasks, row, &group, &task_group).await;
        groups.push(group);
    }

    // Every load balancer this suite created comes into service, whatever its scheme and
    // whatever it forwards to.
    for (placement, arn) in &balancers {
        let code = load_balancer_state(&router, &format!("{placement:?}"), arn).await;
        assert_eq!(
            code, "active",
            "case {placement:?}: expected the load balancer to report active, got {code}"
        );
    }

    for (group, row) in groups.iter().zip(&ROWS) {
        // The task ECS placed, by the address its own interface was allocated: the target the
        // group must hold is that task and not a value this suite made up.
        let address = running_task_address(&router, row.case, row.service_name).await;
        let (reported, seen) = health_reaching(&router, row.case, group, row.state).await;
        assert_eq!(
            reported.len(),
            1,
            "case {:?}: expected the group to hold the service's one task, got {reported:?}",
            row.case
        );
        for (id, state, reason, description) in &reported {
            assert_eq!(
                id, &address,
                "case {:?}: the registration is the task's own address, got {id:?}",
                row.case
            );
            assert_eq!(
                state, row.state,
                "case {:?}: expected {}, got {state}/{reason}/{description:?}",
                row.case, row.state
            );
            if row.reasons.is_empty() {
                // "If the target state is healthy, a reason code is not provided", and
                // neither is a description.
                assert!(
                    reason.is_empty() && description.is_empty(),
                    "case {:?}: expected no reason code and no description, got \
                     {reason:?}/{description:?}",
                    row.case
                );
            } else {
                assert!(
                    row.reasons.contains(&reason.as_str()),
                    "case {:?}: expected one of {:?}, got {reason:?}",
                    row.case,
                    row.reasons
                );
                assert!(
                    !description.is_empty(),
                    "case {:?}: a non-healthy state carries a description, got {state}/{reason}",
                    row.case
                );
            }
        }
        if row.state != "healthy" {
            assert!(
                !seen.iter().any(|state| state == "healthy"),
                "case {:?}: a target whose health check cannot arrive must never be reported \
                 healthy, got the states {seen:?}",
                row.case
            );
        }
    }
}
