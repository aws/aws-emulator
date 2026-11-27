// SPDX-License-Identifier: CC0-1.0

//! An ALB-fronted service placed in the account's DEFAULT network converges: its task runs,
//! the target group it binds holds that task's address, and the health check settles on a
//! verdict.
//!
//! RULE: a service placed in a default subnet of the account's default VPC needs no network
//! of the caller's own — "when you create an AWS account, we create a default VPC for you in
//! each AWS Region" with "a default subnet in each Availability Zone", an internet gateway
//! attached to it and a main route table routing internet-bound traffic to that gateway
//! (Amazon VPC User Guide, "Default VPCs") — so a task of such a service fetches its image
//! from a public registry, reaches `RUNNING`, is registered as an `ip` target of every group
//! its `loadBalancers` bind, and the service reports its deployment `COMPLETED` once it runs
//! the count it was asked for.
//!
//! Three stores have to agree for that to hold, so all three services are mounted on one
//! engine: Amazon EC2 owns the default VPC, its gateway, its route table and its subnets;
//! this service places the task, decides its image fetch against those records and registers
//! it; Elastic Load Balancing v2 holds the registration and reports its health. Everything is
//! driven through the real operations, so what is asserted is what each service persisted.
//!
//! The rows differ along what the rule generalises over: whether the placement is a default
//! subnet of the account's own default VPC or a subnet of a VPC the caller built and gave an
//! internet gateway, and whether the task is given the public address an internet gateway's
//! route carries traffic for — the last row being the one that must NOT converge, so a row
//! above it cannot pass by a path being assumed for every placement.
//!
//! The second suite below holds the three views of ONE address to each other, which is what
//! makes that address something a client can reach. RULE: a task in the `awsvpc` network mode
//! "is allocated an elastic network interface" (`RegisterTaskDefinition$networkMode`), and the
//! attachment that stands for it reports, once the interface is in place, "the network
//! interface ID, the MAC address, the subnet ID, and the private IPv4 address"
//! (`Attachment$details`) — of an interface Amazon VPC really holds, in the subnet the
//! placement named, at that very address; and every running task of a service whose
//! `loadBalancers` bind a group of the `ip` target type is a target of that group AT that
//! address. So the attachment, Amazon EC2's own `DescribeNetworkInterfaces` and Elastic Load
//! Balancing v2's `DescribeTargetHealth` name one address, whichever of them is asked.
//!
//! Its rows differ along what that rule generalises over: how many tasks the service places
//! (so no row passes on one interface being the only one there is), the block of the subnet it
//! places them in, and whether the service binds a target group at all — the unbound row
//! showing the interface is reported because the task holds it and not because something
//! registered it.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{count, create_cluster, ec2_element, ec2_form, list, ok, string, AWSVPC_START};
use crate::load_balancer_target_groups::{all_texts, elb_call, first_text, health_texts};

/// The cluster every row's service runs in.
const CLUSTER: &str = "defaultnet";

/// The container every row's revision runs, and the port it listens on.
const CONTAINER: &str = "app";
const CONTAINER_PORT: i64 = 80;

/// An image held in a PUBLIC container registry: its registry is no AWS service, so it is
/// reachable only over the internet.
const IMAGE: &str = "public.ecr.aws/nginx/nginx:latest";

/// The CIDR blocks of the VPC the caller-built row runs in, and of its two subnets.
const OWN_VPC_CIDR: &str = "10.60.0.0/16";
const OWN_SUBNET_CIDRS: [&str; 2] = ["10.60.1.0/24", "10.60.2.0/24"];

/// The destination of the route that carries traffic off a VPC.
const INTERNET_DESTINATION: &str = "0.0.0.0/0";

/// How often a poll re-reads the surface it waits on.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How long a poll keeps trying: the whole start of an `awsvpc` task
/// ([`AWSVPC_START`]), the registration that start owes, the checks the accelerated
/// health-check schedule then runs, and for the negative row the attempt a launch failure
/// waits out where nothing answers the fetch (`state::FAILED_IMAGE_FETCH_TIMEOUT`) and the
/// two further spans such a failure takes — with generous margin, so a loaded machine
/// cannot fail a row the emulator would satisfy. The deadline is only here so a failure is
/// a failure rather than a hang.
const POLL_BUDGET: Duration = AWSVPC_START
    .saturating_add(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::FAILED_IMAGE_FETCH_TIMEOUT,
    )
    .saturating_add(Duration::from_secs(30));

/// Where a row places its service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Placement {
    /// A default subnet of the account's own default VPC, which the row creates nothing in.
    DefaultSubnet,
    /// A subnet of a VPC the row created, with an internet gateway and a default route to it.
    OwnSubnetWithAGateway,
}

/// One row: where its service is placed, the public address the placement asks for, and
/// whether the service must then converge.
struct Row {
    case: &'static str,
    /// The target group's name, the listener port and the service name this row uses.
    group_name: &'static str,
    listener_port: &'static str,
    service_name: &'static str,
    placement: Placement,
    /// `AwsVpcConfiguration$assignPublicIp` the placement declares.
    assign_public_ip: &'static str,
    /// Whether the task must RUN — so the service converges and the group holds its address.
    converges: bool,
}

const ROWS: [Row; 3] = [
    Row {
        case: "a default subnet of the account's default VPC, with a public address",
        group_name: "defaultsubnet",
        listener_port: "80",
        service_name: "defaultsubnet",
        placement: Placement::DefaultSubnet,
        assign_public_ip: "ENABLED",
        converges: true,
    },
    Row {
        case: "a subnet of a VPC the caller built and gave an internet gateway",
        group_name: "ownsubnet",
        listener_port: "8081",
        service_name: "ownsubnet",
        placement: Placement::OwnSubnetWithAGateway,
        assign_public_ip: "ENABLED",
        converges: true,
    },
    Row {
        case: "a default subnet, for a task given no public address",
        group_name: "nopublicip",
        listener_port: "8082",
        service_name: "nopublicip",
        placement: Placement::DefaultSubnet,
        assign_public_ip: "DISABLED",
        converges: false,
    },
];

/// A router carrying this service, Amazon EC2 and Elastic Load Balancing v2 over ONE shared
/// engine — the three stores this rule reads across, mounted the way the production router
/// mounts them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
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

/// One ELBv2 call that must succeed, as the body it answered.
async fn elb_ok(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
    case: &str,
) -> String {
    let (status, body) = elb_call(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {action} to answer 200, got {status} with {body}"
    );
    body
}

/// The network one row places its service in: the VPC, its two subnets, and the two security
/// groups — the load balancer's own, and the task's, which admits the checked port from it.
struct Network {
    vpc: String,
    subnets: Vec<String>,
    load_balancer_group: String,
    task_group: String,
}

/// The account's DEFAULT VPC and two of its default subnets, as the account's own records
/// report them. Nothing is created here.
async fn default_network(router: &MultiServiceRouter, case: &str) -> (String, Vec<String>) {
    let vpcs = ec2_form(
        router,
        "DescribeVpcs",
        &[("Filter.1.Name", "isDefault"), ("Filter.1.Value.1", "true")],
        case,
    )
    .await;
    let vpc = ec2_element(&vpcs, "vpcId", case);
    let subnets = ec2_form(
        router,
        "DescribeSubnets",
        &[
            ("Filter.1.Name", "vpc-id"),
            ("Filter.1.Value.1", &vpc),
            ("Filter.2.Name", "default-for-az"),
            ("Filter.2.Value.1", "true"),
        ],
        case,
    )
    .await;
    let ids: Vec<String> = all_texts(&subnets, "subnetId").into_iter().take(2).collect();
    assert_eq!(
        ids.len(),
        2,
        "case {case:?}: the default VPC holds a default subnet per zone, got {subnets}"
    );
    (vpc, ids)
}

/// A VPC of the caller's own over `vpc_cidr` with one subnet per block of `subnet_cidrs`, each
/// in its own zone, an internet gateway attached to it and a default route to that gateway in
/// its main route table — every one created through the operations a caller calls.
///
/// Shared by every suite of this module that places into a network of its own, so the two
/// cannot come to build different networks: what a row varies is the blocks it passes here.
async fn gatewayed_vpc(
    router: &MultiServiceRouter,
    case: &str,
    vpc_cidr: &str,
    subnet_cidrs: [&str; 2],
) -> (String, Vec<String>) {
    let vpc = ec2_element(
        &ec2_form(router, "CreateVpc", &[("CidrBlock", vpc_cidr)], case).await,
        "vpcId",
        case,
    );
    let mut subnets = Vec::new();
    for (index, cidr) in subnet_cidrs.into_iter().enumerate() {
        let zone = format!("{}{}", crate::ecs_wire::SIGNED_REGION, ["a", "b"][index]);
        subnets.push(ec2_element(
            &ec2_form(
                router,
                "CreateSubnet",
                &[
                    ("VpcId", &vpc),
                    ("CidrBlock", cidr),
                    ("AvailabilityZone", &zone),
                ],
                case,
            )
            .await,
            "subnetId",
            case,
        ));
    }
    let gateway = ec2_element(
        &ec2_form(router, "CreateInternetGateway", &[], case).await,
        "internetGatewayId",
        case,
    );
    ec2_form(
        router,
        "AttachInternetGateway",
        &[("InternetGatewayId", &gateway), ("VpcId", &vpc)],
        case,
    )
    .await;
    let table = ec2_element(
        &ec2_form(
            router,
            "DescribeRouteTables",
            &[("Filter.1.Name", "vpc-id"), ("Filter.1.Value.1", &vpc)],
            case,
        )
        .await,
        "routeTableId",
        case,
    );
    ec2_form(
        router,
        "CreateRoute",
        &[
            ("RouteTableId", &table),
            ("DestinationCidrBlock", INTERNET_DESTINATION),
            ("GatewayId", &gateway),
        ],
        case,
    )
    .await;
    (vpc, subnets)
}

/// One security group of `vpc`, as its id.
async fn security_group(
    router: &MultiServiceRouter,
    vpc: &str,
    name: &str,
    description: &str,
    case: &str,
) -> String {
    ec2_element(
        &ec2_form(
            router,
            "CreateSecurityGroup",
            &[
                ("GroupName", name),
                ("GroupDescription", description),
                ("VpcId", vpc),
            ],
            case,
        )
        .await,
        "groupId",
        case,
    )
}

/// The load balancer's OWN security group and the tasks', named after `label`, with the
/// inbound rule that lets the load balancer's health check reach the task: "a security group
/// controls the traffic that is allowed to reach ... the resources that it is associated with"
/// (Amazon VPC User Guide).
///
/// Shared by the suites of this module for the reason [`gatewayed_vpc`] is.
async fn paired_security_groups(
    router: &MultiServiceRouter,
    vpc: &str,
    label: &str,
    case: &str,
) -> (String, String) {
    let load_balancer_group = security_group(
        router,
        vpc,
        &format!("{label}-alb"),
        "the load balancers own group",
        case,
    )
    .await;
    let task_group = security_group(
        router,
        vpc,
        &format!("{label}-task"),
        "the tasks own group",
        case,
    )
    .await;
    ec2_form(
        router,
        "AuthorizeSecurityGroupIngress",
        &[
            ("GroupId", &task_group),
            ("IpPermissions.1.IpProtocol", "tcp"),
            ("IpPermissions.1.FromPort", "80"),
            ("IpPermissions.1.ToPort", "80"),
            ("IpPermissions.1.Groups.1.GroupId", &load_balancer_group),
        ],
        case,
    )
    .await;
    (load_balancer_group, task_group)
}

/// The whole network one row runs in.
async fn network_of(router: &MultiServiceRouter, row: &Row) -> Network {
    let case = row.case;
    let (vpc, subnets) = match row.placement {
        Placement::DefaultSubnet => default_network(router, case).await,
        Placement::OwnSubnetWithAGateway => {
            gatewayed_vpc(router, case, OWN_VPC_CIDR, OWN_SUBNET_CIDRS).await
        }
    };
    let (load_balancer_group, task_group) =
        paired_security_groups(router, &vpc, row.group_name, case).await;
    Network {
        vpc,
        subnets,
        load_balancer_group,
        task_group,
    }
}

/// The `ip` target group a row binds under `name`, with a load balancer in its network
/// forwarding to it from `listener_port` — a group nothing forwards to is a group whose
/// targets nothing checks.
async fn load_balanced_target_group(
    router: &MultiServiceRouter,
    name: &str,
    listener_port: &str,
    network: &Network,
    case: &str,
) -> String {
    let created = elb_ok(
        router,
        "CreateTargetGroup",
        &[
            ("Name", name),
            ("Protocol", "HTTP"),
            ("Port", "80"),
            ("TargetType", "ip"),
            ("VpcId", &network.vpc),
            ("HealthCheckProtocol", "HTTP"),
            ("HealthCheckPath", "/"),
        ],
        case,
    )
    .await;
    let group = first_text(&created, "TargetGroupArn")
        .unwrap_or_else(|| panic!("case {case:?}: no TargetGroupArn in {created}"));
    let load_balancer = first_text(
        &elb_ok(
            router,
            "CreateLoadBalancer",
            &[
                ("Name", name),
                ("Subnets.member.1", &network.subnets[0]),
                ("Subnets.member.2", &network.subnets[1]),
                ("SecurityGroups.member.1", &network.load_balancer_group),
            ],
            case,
        )
        .await,
        "LoadBalancerArn",
    )
    .unwrap_or_else(|| panic!("case {case:?}: the load balancer must be created"));
    elb_ok(
        router,
        "CreateListener",
        &[
            ("LoadBalancerArn", &load_balancer),
            ("Protocol", "HTTP"),
            ("Port", listener_port),
            ("DefaultActions.member.1.Type", "forward"),
            ("DefaultActions.member.1.TargetGroupArn", &group),
        ],
        case,
    )
    .await;
    group
}

/// Register the revision every row's service runs: an `awsvpc`, Fargate-compatible revision
/// whose one container listens on [`CONTAINER_PORT`] and runs an image held in a public
/// registry.
async fn register_revision(router: &MultiServiceRouter, family: &str, case: &str) {
    ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
            "networkMode": "awsvpc",
            "cpu": "256",
            "memory": "512",
            "requiresCompatibilities": ["FARGATE"],
            "containerDefinitions": [{
                "name": CONTAINER,
                "image": IMAGE,
                "memory": 128,
                "portMappings": [{ "containerPort": CONTAINER_PORT }],
            }],
        }),
        case,
    )
    .await;
}

/// What ONE `CreateService` of these suites places: which cluster and service, which
/// revision, the public address its interface asks for, how many tasks it wants, and the `ip`
/// target group it binds where it binds one.
struct ServicePlacement<'a> {
    case: &'a str,
    cluster: &'a str,
    service_name: &'a str,
    family: &'a str,
    /// `AwsVpcConfiguration$assignPublicIp` the placement declares.
    assign_public_ip: &'a str,
    /// `CreateService$desiredCount`.
    desired: usize,
    /// `ServiceLoadBalancer$targetGroupArn`, for a service that binds a group.
    group: Option<&'a str>,
}

/// Create one service into `network`, as `placement` describes it.
///
/// Shared by the suites of this module for the reason [`gatewayed_vpc`] is: the only
/// differences between their placements are the values above.
async fn place_service(
    router: &MultiServiceRouter,
    placement: &ServicePlacement<'_>,
    network: &Network,
) {
    let mut request = json!({
        "cluster": placement.cluster,
        "serviceName": placement.service_name,
        "taskDefinition": placement.family,
        "desiredCount": placement.desired,
        "launchType": "FARGATE",
        "networkConfiguration": {
            "awsvpcConfiguration": {
                "subnets": [network.subnets[0]],
                "securityGroups": [network.task_group],
                "assignPublicIp": placement.assign_public_ip,
            }
        },
    });
    if let Some(group) = placement.group {
        request["loadBalancers"] = json!([{
            "targetGroupArn": group,
            "containerName": CONTAINER,
            "containerPort": CONTAINER_PORT,
        }]);
    }
    ok(router, "CreateService", request, placement.case).await;
}

/// One row's service as `DescribeServices` reports it now.
async fn described_service(router: &MultiServiceRouter, row: &Row) -> Value {
    let body = ok(
        router,
        "DescribeServices",
        json!({ "cluster": CLUSTER, "services": [row.service_name] }),
        row.case,
    )
    .await;
    list(&body, "services", row.case)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {:?}: the service must read back", row.case))
}

/// The `DescribeTargetHealth` answer for one target group — the ONE read both suites of this
/// module ask Elastic Load Balancing v2 for what a group holds through.
async fn target_health(router: &MultiServiceRouter, group: &str, case: &str) -> String {
    elb_ok(
        router,
        "DescribeTargetHealth",
        &[("TargetGroupArn", group)],
        case,
    )
    .await
}

/// `(target id, state)` for every registration one group reports.
async fn health(router: &MultiServiceRouter, group: &str, case: &str) -> Vec<(String, String)> {
    let body = target_health(router, group, case).await;
    let ids = all_texts(&body, "Id");
    // One state per DESCRIPTION, read inside that description's own health block: the
    // administrative override reported beside it carries a `State` of its own
    // ([`health_texts`]).
    let states = health_texts(&body, "State");
    ids.into_iter().zip(states).collect()
}

/// The private IPv4 address of the one RUNNING task of one row's service.
async fn running_task_address(router: &MultiServiceRouter, row: &Row) -> String {
    let case = row.case;
    let listed = ok(
        router,
        "ListTasks",
        json!({ "cluster": CLUSTER, "serviceName": row.service_name }),
        case,
    )
    .await;
    let arns: Vec<Value> = list(&listed, "taskArns", case);
    let described = ok(
        router,
        "DescribeTasks",
        json!({ "cluster": CLUSTER, "tasks": arns }),
        case,
    )
    .await;
    let task = list(&described, "tasks", case)
        .into_iter()
        .find(|task| task["lastStatus"].as_str() == Some("RUNNING"))
        .unwrap_or_else(|| panic!("case {case:?}: the service must run a task, got {described}"));
    list(&task, "attachments", case)
        .iter()
        .flat_map(|attachment| list(attachment, "details", case))
        .find(|detail| detail["name"].as_str() == Some("privateIPv4Address"))
        .and_then(|detail| detail["value"].as_str().map(str::to_string))
        .unwrap_or_else(|| panic!("case {case:?}: the task carries its interface's address"))
}

#[tokio::test]
async fn an_alb_fronted_service_in_the_default_network_converges_to_a_healthy_target() {
    let router = emulator();
    create_cluster(&router, CLUSTER, "the cluster").await;

    let mut built: Vec<(&Row, String)> = Vec::new();
    for row in &ROWS {
        let network = network_of(&router, row).await;
        let family = format!("{}family", row.group_name);
        register_revision(&router, &family, row.case).await;
        let group =
            load_balanced_target_group(&router, row.group_name, row.listener_port, &network, row.case)
                .await;
        place_service(
            &router,
            &ServicePlacement {
                case: row.case,
                cluster: CLUSTER,
                service_name: row.service_name,
                family: &family,
                assign_public_ip: row.assign_public_ip,
                desired: 1,
                group: Some(&group),
            },
            &network,
        )
        .await;
        built.push((row, group));
    }

    for (row, group) in &built {
        let case = row.case;
        let deadline = Instant::now() + POLL_BUDGET;
        loop {
            let service = described_service(&router, row).await;
            let running = count(&service, "runningCount", case);
            let primary = list(&service, "deployments", case)
                .into_iter()
                .find(|deployment| deployment["status"].as_str() == Some("PRIMARY"));
            let rollout = primary
                .as_ref()
                .and_then(|deployment| deployment["rolloutState"].as_str())
                .unwrap_or_default()
                .to_string();
            let reported = health(&router, group, case).await;
            if row.converges {
                let settled = running == 1
                    && rollout == "COMPLETED"
                    && reported.len() == 1
                    && reported[0].1 == "healthy";
                if settled {
                    let address = running_task_address(&router, row).await;
                    assert_eq!(
                        reported[0].0, address,
                        "case {case:?}: the healthy target is the task's own address, got \
                         {reported:?}"
                    );
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "case {case:?}: expected the service to run its task and the group to \
                     hold it healthy, got runningCount {running}, rolloutState {rollout:?} \
                     and the targets {reported:?} with the service {service}"
                );
            } else {
                // The negative row: no path to the registry, so no container ever runs and
                // nothing is ever a healthy target of the group.
                assert!(
                    !reported.iter().any(|(_, state)| state == "healthy"),
                    "case {case:?}: a task that never fetched its image is no healthy \
                     target, got {reported:?}"
                );
                let stopped = ok(
                    &router,
                    "ListTasks",
                    json!({
                        "cluster": CLUSTER,
                        "serviceName": row.service_name,
                        "desiredStatus": "STOPPED",
                    }),
                    case,
                )
                .await;
                if !list(&stopped, "taskArns", case).is_empty() {
                    let described = ok(
                        &router,
                        "DescribeTasks",
                        json!({
                            "cluster": CLUSTER,
                            "tasks": list(&stopped, "taskArns", case),
                        }),
                        case,
                    )
                    .await;
                    let failed = list(&described, "tasks", case).into_iter().find(|task| {
                        task["stopCode"].as_str() == Some("TaskFailedToStart")
                    });
                    if let Some(task) = failed {
                        let reason = string(&task, "stoppedReason", case);
                        assert!(
                            reason.contains("CannotPullContainerError"),
                            "case {case:?}: expected the runtime's failed-fetch reason, got \
                             {reason:?}"
                        );
                        break;
                    }
                }
                assert!(
                    Instant::now() < deadline,
                    "case {case:?}: expected the task to be stopped for a failed image \
                     fetch, got {stopped}"
                );
            }
            ::tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

// The three views of ONE `awsvpc` task's address: the attachment Amazon ECS reports, the
// interface Amazon VPC holds, and the target Elastic Load Balancing v2 registered.

/// The cluster the services of the join below run in — its own, so what it reads back is what
/// it placed.
const JOIN_CLUSTER: &str = "joinedviews";

/// The `Attachment$details` keys the attachment of an interface that is IN PLACE reports, from
/// that member's own doc sentence: for an elastic network interface the details include "the
/// network interface ID, the MAC address, the subnet ID, and the private IPv4 address".
const INTERFACE_DETAILS: [&str; 4] = [
    "networkInterfaceId",
    "macAddress",
    "subnetId",
    "privateIPv4Address",
];

/// One row of the join: the network it builds, the service it places there, and whether that
/// service binds an `ip` target group.
struct JoinRow {
    case: &'static str,
    /// What every resource of the row is named after: its VPC's groups, its revision, its
    /// service and its target group.
    label: &'static str,
    /// The CIDR block of the row's VPC, which holds both of its subnets'.
    vpc_cidr: &'static str,
    /// `Subnet$cidrBlock` of the subnet the service places its tasks in, and of the second
    /// subnet of the row's VPC: a load balancer is created across subnets of two zones, so a
    /// row that fronts its service needs one beside the one its tasks run in.
    task_subnet_cidr: &'static str,
    second_subnet_cidr: &'static str,
    /// The port the row's listener forwards from, where the row has one.
    listener_port: &'static str,
    /// `CreateService$desiredCount`: how many tasks of its own the service places.
    desired: usize,
    /// Whether the service binds an `ip` target group that a load balancer forwards to.
    binds_a_target_group: bool,
}

const JOIN_ROWS: [JoinRow; 3] = [
    JoinRow {
        case: "one task of a service behind an ip target group",
        label: "joinone",
        vpc_cidr: "10.80.0.0/16",
        task_subnet_cidr: "10.80.1.0/24",
        second_subnet_cidr: "10.80.2.0/24",
        listener_port: "9080",
        desired: 1,
        binds_a_target_group: true,
    },
    JoinRow {
        case: "two tasks of a service behind an ip target group, in a /28 subnet",
        label: "jointwo",
        vpc_cidr: "10.81.0.0/16",
        task_subnet_cidr: "10.81.3.16/28",
        second_subnet_cidr: "10.81.4.0/24",
        listener_port: "9081",
        desired: 2,
        binds_a_target_group: true,
    },
    JoinRow {
        case: "one task of a service bound to no load balancer",
        label: "joinnone",
        vpc_cidr: "10.82.0.0/16",
        task_subnet_cidr: "10.82.5.0/26",
        second_subnet_cidr: "10.82.6.0/24",
        listener_port: "9082",
        desired: 1,
        binds_a_target_group: false,
    },
];

/// The network one join row places into: the gatewayed VPC of its own blocks
/// ([`gatewayed_vpc`]) with the two security groups every placement of this module runs under
/// ([`paired_security_groups`]).
async fn join_network(router: &MultiServiceRouter, row: &JoinRow) -> Network {
    let (vpc, subnets) = gatewayed_vpc(
        router,
        row.case,
        row.vpc_cidr,
        [row.task_subnet_cidr, row.second_subnet_cidr],
    )
    .await;
    let (load_balancer_group, task_group) =
        paired_security_groups(router, &vpc, row.label, row.case).await;
    Network {
        vpc,
        subnets,
        load_balancer_group,
        task_group,
    }
}

/// One `Attachment$details` entry, by name.
fn attachment_detail(attachment: &Value, name: &str, case: &str) -> Option<String> {
    list(attachment, "details", case)
        .into_iter()
        .find(|entry| entry["name"].as_str() == Some(name))
        .and_then(|entry| entry["value"].as_str().map(str::to_string))
}

/// Whether one attachment reports the interface it stands for as in place: `ATTACHED`,
/// carrying every detail [`INTERFACE_DETAILS`] names.
fn reports_its_interface(attachment: &Value, case: &str) -> bool {
    attachment["status"].as_str() == Some("ATTACHED")
        && INTERFACE_DETAILS.iter().all(|name| {
            attachment_detail(attachment, name, case).is_some_and(|value| !value.is_empty())
        })
}

/// The `ElasticNetworkInterface` attachment of every RUNNING task of one join row's service,
/// read back through `ListTasks` and `DescribeTasks`.
async fn running_interface_attachments(router: &MultiServiceRouter, row: &JoinRow) -> Vec<Value> {
    let case = row.case;
    let listed = ok(
        router,
        "ListTasks",
        json!({
            "cluster": JOIN_CLUSTER,
            "serviceName": row.label,
            "desiredStatus": "RUNNING",
        }),
        case,
    )
    .await;
    let arns: Vec<Value> = list(&listed, "taskArns", case);
    if arns.is_empty() {
        return Vec::new();
    }
    let described = ok(
        router,
        "DescribeTasks",
        json!({ "cluster": JOIN_CLUSTER, "tasks": arns }),
        case,
    )
    .await;
    list(&described, "tasks", case)
        .into_iter()
        .filter(|task| task["lastStatus"].as_str() == Some("RUNNING"))
        .flat_map(|task| list(&task, "attachments", case))
        .filter(|attachment| attachment["type"].as_str() == Some("ElasticNetworkInterface"))
        .collect()
}

/// One interface as Amazon EC2 holds it, reduced to the three members the task's attachment
/// reports for it.
#[derive(Debug)]
struct HeldInterface {
    subnet_id: String,
    mac_address: String,
    private_ip_address: String,
}

/// What Amazon EC2's own `DescribeNetworkInterfaces` reports for ONE interface id — the read
/// that says whether the interface a task's attachment names exists at all.
async fn held_interface(
    router: &MultiServiceRouter,
    network_interface_id: &str,
    case: &str,
) -> HeldInterface {
    let body = ec2_form(
        router,
        "DescribeNetworkInterfaces",
        &[("NetworkInterfaceId.1", network_interface_id)],
        case,
    )
    .await;
    assert_eq!(
        all_texts(&body, "networkInterfaceId"),
        vec![network_interface_id.to_string()],
        "case {case:?}: expected Amazon EC2 to hold exactly the interface the attachment \
         names, got {body}"
    );
    // Each of the three is a member of the interface ITSELF, and an answer about ONE interface
    // carries one value for each: the address is spelled both on the interface and in the set
    // of addresses it holds, so every spelling of it has to be the same address. Read as a
    // SET, so a reordered or repeated member fails loudly rather than being read as the first
    // tag that matched.
    let one = |name: &str| {
        let found: BTreeSet<String> = all_texts(&body, name).into_iter().collect();
        assert_eq!(
            found.len(),
            1,
            "case {case:?}: expected the interface {network_interface_id} to report one \
             <{name}>, got {found:?} in {body}"
        );
        found.into_iter().next().unwrap_or_default()
    };
    HeldInterface {
        subnet_id: one("subnetId"),
        mac_address: one("macAddress"),
        private_ip_address: one("privateIpAddress"),
    }
}

/// Every `(address, port)` one target group reports a target at, read off the same answer
/// [`health`] reads its states off.
async fn registered_targets(
    router: &MultiServiceRouter,
    group: &str,
    case: &str,
) -> Vec<(String, String)> {
    let body = target_health(router, group, case).await;
    all_texts(&body, "Id")
        .into_iter()
        .zip(all_texts(&body, "Port"))
        .collect()
}

#[tokio::test]
async fn a_service_placed_tasks_address_is_the_interface_ec2_holds_and_the_target_registered() {
    let router = emulator();
    create_cluster(&router, JOIN_CLUSTER, "the cluster").await;

    // Every row is placed first, so the rows' interfaces are created and attached at the same
    // time rather than one row's wait running after another's.
    let mut built: Vec<(&JoinRow, Network, Option<String>)> = Vec::new();
    for row in &JOIN_ROWS {
        let network = join_network(&router, row).await;
        let family = format!("{}family", row.label);
        register_revision(&router, &family, row.case).await;
        let group = if row.binds_a_target_group {
            Some(
                load_balanced_target_group(
                    &router,
                    row.label,
                    row.listener_port,
                    &network,
                    row.case,
                )
                .await,
            )
        } else {
            None
        };
        place_service(
            &router,
            &ServicePlacement {
                case: row.case,
                cluster: JOIN_CLUSTER,
                service_name: row.label,
                family: &family,
                // The task's own public address, which is what makes the internet gateway its
                // subnet routes through a path to the image's registry.
                assign_public_ip: "ENABLED",
                desired: row.desired,
                group: group.as_deref(),
            },
            &network,
        )
        .await;
        built.push((row, network, group));
    }

    // Nothing handed out by the ONE store every row shares may be handed out twice: an
    // interface, an address and a MAC address belong to one task.
    let mut addresses: BTreeSet<String> = BTreeSet::new();
    let mut interface_ids: BTreeSet<String> = BTreeSet::new();
    let mut mac_addresses: BTreeSet<String> = BTreeSet::new();
    let mut joined = 0usize;

    for (row, network, group) in &built {
        let case = row.case;
        let subnet = network.subnets[0].as_str();
        let deadline = Instant::now() + POLL_BUDGET;
        // The CONDITION is polled for, never a span waited out: every task the service was
        // asked for runs and reports the interface it was allocated, and the group it binds
        // holds one target per task.
        let attachments = loop {
            let attachments = running_interface_attachments(&router, row).await;
            let targets = match group {
                Some(group) => registered_targets(&router, group, case).await,
                None => Vec::new(),
            };
            let wanted_targets = if group.is_some() { row.desired } else { 0 };
            if attachments.len() == row.desired
                && attachments
                    .iter()
                    .all(|attachment| reports_its_interface(attachment, case))
                && targets.len() == wanted_targets
            {
                break attachments;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: expected {} running task(s) each reporting the interface it \
                 was allocated and {wanted_targets} registered target(s), got the attachments \
                 {attachments:?} and the targets {targets:?}",
                row.desired
            );
            ::tokio::time::sleep(POLL_INTERVAL).await;
        };

        for attachment in &attachments {
            let read = |name: &str| {
                attachment_detail(attachment, name, case).unwrap_or_else(|| {
                    panic!("case {case:?}: expected the attachment to report {name}, got {attachment}")
                })
            };
            let interface_id = read("networkInterfaceId");
            let address = read("privateIPv4Address");
            let mac_address = read("macAddress");
            assert_eq!(
                read("subnetId"),
                subnet,
                "case {case:?}: the interface is created in the subnet the placement named, \
                 got {attachment}"
            );
            // The interface the attachment NAMES is one Amazon VPC really holds, in that same
            // subnet and at that same address: an id resolving to nothing is an address no
            // client could reach.
            let held = held_interface(&router, &interface_id, case).await;
            assert_eq!(
                (
                    held.private_ip_address.as_str(),
                    held.subnet_id.as_str(),
                    held.mac_address.as_str()
                ),
                (address.as_str(), subnet, mac_address.as_str()),
                "case {case:?}: expected Amazon EC2 to hold the interface the attachment \
                 reports, got {held:?} against {attachment}"
            );
            assert!(
                addresses.insert(address.clone()),
                "case {case:?}: expected an address no other interface holds, got {address} \
                 among {addresses:?}"
            );
            assert!(
                interface_ids.insert(interface_id.clone()),
                "case {case:?}: expected an interface id no other task reports, got \
                 {interface_id} among {interface_ids:?}"
            );
            assert!(
                mac_addresses.insert(mac_address.clone()),
                "case {case:?}: expected a MAC address no other interface holds, got \
                 {mac_address} among {mac_addresses:?}"
            );
            joined += 1;
        }

        // Every running task of a service that binds an `ip` group is a target of that group
        // AT the address its own interface holds, on the port the binding named.
        if let Some(group) = group {
            let expected: BTreeSet<(String, String)> = attachments
                .iter()
                .map(|attachment| {
                    (
                        attachment_detail(attachment, "privateIPv4Address", case)
                            .unwrap_or_default(),
                        CONTAINER_PORT.to_string(),
                    )
                })
                .collect();
            let reported: BTreeSet<(String, String)> = registered_targets(&router, group, case)
                .await
                .into_iter()
                .collect();
            assert_eq!(
                reported, expected,
                "case {case:?}: expected the group to hold one target per running task, at \
                 the address and port each task's own interface listens on"
            );
        }

        // Two reads report ONE interface per task: the values are stored, not minted per read.
        let again: BTreeSet<String> = running_interface_attachments(&router, row)
            .await
            .iter()
            .map(Value::to_string)
            .collect();
        let settled: BTreeSet<String> = attachments.iter().map(Value::to_string).collect();
        assert_eq!(
            again, settled,
            "case {case:?}: expected a second read to report the same interface for every \
             task"
        );
    }

    let placed: usize = JOIN_ROWS.iter().map(|row| row.desired).sum();
    assert_eq!(
        joined, placed,
        "case {:?}: expected every task placed to report an interface the three views agree \
         on, got {joined} of {placed}",
        "the whole join"
    );
}
