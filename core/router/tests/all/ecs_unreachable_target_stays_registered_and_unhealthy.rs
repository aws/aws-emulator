// SPDX-License-Identifier: CC0-1.0

//! An Amazon ECS service whose task the load balancer's health check cannot be DELIVERED to:
//! the verdict Elastic Load Balancing reaches for that registration, and the registration
//! that stays exactly where it is.
//!
//! RULE: **a registered target whose health-check probe cannot be delivered — because no
//! inbound rule of the security groups governing it admits the load balancer on the checked
//! port, or because nothing behind it listens on that port — leaves `initial` and settles at
//! `unhealthy` with the reason the family of load balancer performing the checks words an
//! unanswered check as (`Target.Timeout`, "the health check requests timed out", for an
//! Application Load Balancer), never reaching `healthy` on the way; and it STAYS REGISTERED,
//! because a failing load-balancer health check is not a stop the Amazon ECS service
//! scheduler makes — so the group holds exactly one registration per running task of the
//! service, none of them `draining`, and a request the load balancer cannot deliver to any
//! target of the group is answered with the load balancer's own status rather than a
//! target's response.**
//!
//! `TargetHealthStateEnum`
//! (`smithy_query(query="shape",shape="TargetHealthStateEnum")`) is `[initial, healthy,
//! unhealthy, unhealthy.draining, unused, draining, unavailable]` and `TargetHealth$Reason`
//! gives the verdict's reason: "`Target.Timeout` - The health check requests timed out",
//! reached after `UnhealthyThresholdCount`, "the number of consecutive health check failures
//! required before considering a target unhealthy"
//! (`smithy_query(query="doc",shape="CreateTargetGroupInput",member="UnhealthyThresholdCount")`).
//! `Target.DeregistrationInProgress` belongs to a registration something DEREGISTERED, which
//! is why a group that keeps presenting one for a service nobody scaled or redeployed is the
//! defect this suite pins.
//!
//! Three crates have to agree for that to hold, which is why this suite lives here: Amazon ECS
//! places the task, declares the security groups of its interface and registers it, EC2 owns
//! those groups and their inbound rules, and Elastic Load Balancing v2 checks the
//! registration, reports its health and answers the client. Everything is driven through the
//! real operations on one engine, and every wait polls the CONDITION rather than a fixed time.
//!
//! The rows differ along what the rule generalises over: WHY the probe cannot be delivered (a
//! group with no inbound rule at all, a rule naming a source the load balancer is not, a
//! checked port nothing behind the registration listens on), how many tasks the service keeps
//! (so the count the group holds is the service's own and not one), and — as the control that
//! tells the suite's instrument from its subject — a group whose rule does admit the check.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The ONE percent-encoder and element readers this test crate keeps for awsQuery suites
// (`all/query_wire.rs`: "Nothing new should write another one — call this module").
use crate::query_wire::{element, elements, form_encode, members_of, target_health_of};

/// The region every request below is signed for.
const REGION: &str = "us-east-1";

/// The API versions an ec2Query and an awsQuery request carry.
const EC2_API_VERSION: &str = "2016-11-15";
const ELB_VERSION: &str = "2015-12-01";

/// The status every control-plane call of the setup must answer.
const OK: u16 = 200;

/// The ECS cluster, task-definition family and container every row runs.
const CLUSTER: &str = "unreachable";
const FAMILY: &str = "unreachablefamily";
const CONTAINER: &str = "app";

/// The port the container declares, which is the `containerPort` every binding names and
/// every target group's own traffic port.
const CONTAINER_PORT: i64 = 80;

/// A port NO container of the revision declares, so nothing behind a registration answers a
/// check pointed at it.
const UNSERVED_PORT: &str = "8080";

/// The network every row runs in.
const VPC_CIDR: &str = "10.91.0.0/16";
const SUBNET_CIDRS: [(&str, &str); 2] = [
    ("10.91.1.0/24", "us-east-1a"),
    ("10.91.2.0/24", "us-east-1b"),
];

/// The cadence and the thresholds every target group below is created with: a verdict takes a
/// threshold of consecutive checks, each one interval after the last, so this suite asks for
/// the shortest interval the member admits ("the range is 5-300") and the fewest checks a
/// threshold may ask for ("the range is 2-10").
const INTERVAL_SECONDS: &str = "5";
const THRESHOLD: &str = "2";

/// The `TargetHealth` triples this suite reads back: the `healthy` a reachable registration is
/// owed (reported with no reason and no description) and the `unhealthy` an Application Load
/// Balancer words an unanswered check as.
const HEALTHY: (&str, &str, &str) = ("healthy", "", "");
const UNHEALTHY: (&str, &str, &str) = ("unhealthy", "Target.Timeout", "Request timed out");

/// The states that must never be reported for these services: a registration something
/// deregistered, and the health a check nothing answered must not produce.
const DRAINING_STATE: &str = "draining";
const HEALTHY_STATE: &str = "healthy";

/// How long a step waits for a clock-owed move — a task's start, the registration it owes, the
/// verdict the group's own cadence reaches — and how often it looks. Generous on purpose: the
/// deadline is only here so a failure is a failure rather than a hang.
const DEADLINE: Duration = Duration::from_secs(120);
const POLL: Duration = Duration::from_millis(100);

/// How long the settled estate is WATCHED for a move nobody asked for: three of the cadence
/// every group here runs its checks at ([`INTERVAL_SECONDS`]), so a scheduler that replaced
/// the task behind a failing check, or a group that drained the registration and started
/// another, is observed doing it rather than missed between two reads.
const WATCH_WINDOW: Duration = Duration::from_secs(15);

/// The source an inbound rule of a row's task group names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    /// The security group the load balancer itself is associated with.
    LoadBalancerGroup,
    /// Another security group of the same VPC, which the load balancer is not in.
    OtherGroup,
}

/// One row: the service, the inbound rule its task's security group carries, the port its
/// group checks, and the health every registration of it must settle at.
struct Row {
    case: &'static str,
    /// The name the row's load balancer, target group and service are created under.
    name: &'static str,
    /// `desiredCount` of the row's service, which is how many registrations the group holds.
    desired: usize,
    /// `(IpProtocol, FromPort, ToPort, source)` of the one inbound rule the task's group
    /// carries, or `None` for a group with no inbound rule at all.
    rule: Option<(
        &'static str,
        Option<&'static str>,
        Option<&'static str>,
        Source,
    )>,
    /// `HealthCheckPort`, and `None` for a group checked on the port its targets receive
    /// traffic on.
    health_check_port: Option<&'static str>,
    /// The `(State, Reason, Description)` every registration of the row must settle at.
    health: (&'static str, &'static str, &'static str),
    /// Whether the load balancer's own connection to the target is blocked as well, which is
    /// the case where a client gets the load balancer's own status instead of a response.
    unreachable: bool,
}

const ROWS: [Row; 5] = [
    Row {
        case: "the recorded configuration: a task group with no inbound rule at all",
        name: "noingress",
        desired: 1,
        rule: None,
        health_check_port: None,
        health: UNHEALTHY,
        unreachable: true,
    },
    Row {
        case: "two tasks whose group admits nothing",
        name: "twotasks",
        desired: 2,
        rule: None,
        health_check_port: None,
        health: UNHEALTHY,
        unreachable: true,
    },
    Row {
        case: "the checked port admitted from a group the load balancer is not in",
        name: "othergroup",
        desired: 1,
        rule: Some(("tcp", Some("80"), Some("80"), Source::OtherGroup)),
        health_check_port: None,
        health: UNHEALTHY,
        unreachable: true,
    },
    Row {
        case: "every port admitted from the load balancer, checked on a port nothing serves",
        name: "noserver",
        desired: 1,
        rule: Some(("-1", None, None, Source::LoadBalancerGroup)),
        health_check_port: Some(UNSERVED_PORT),
        health: UNHEALTHY,
        unreachable: false,
    },
    Row {
        case: "the control: the checked port admitted from the load balancer's own group",
        name: "admitted",
        desired: 1,
        rule: Some(("tcp", Some("80"), Some("80"), Source::LoadBalancerGroup)),
        health_check_port: None,
        health: HEALTHY,
        unreachable: false,
    },
];

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
        status, OK,
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
        status, OK,
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
        status, OK,
        "setup: ECS {operation} answered {status} with {parsed}"
    );
    parsed
}

/// The network every row runs in: a VPC with a path off it, two subnets, the group the load
/// balancers are associated with, and one more group no load balancer is in.
struct Network {
    vpc: String,
    subnets: Vec<String>,
    load_balancer_group: String,
    other_group: String,
}

/// Create that network through EC2 itself, so every id asserted against is one EC2 minted.
async fn seed_network(router: &MultiServiceRouter) -> Network {
    let vpc = element(
        &ec2(router, "CreateVpc", &[("CidrBlock", VPC_CIDR)]).await,
        "vpcId",
    );
    // The internet gateway an internet-facing load balancer's subnets need, attached through
    // EC2's own operation.
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
    for (cidr, zone) in SUBNET_CIDRS {
        subnets.push(element(
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
        ));
    }
    let group = |name: &'static str, description: &'static str| {
        let vpc = vpc.clone();
        async move {
            element(
                &ec2(
                    router,
                    "CreateSecurityGroup",
                    &[
                        ("GroupName", name),
                        ("GroupDescription", description),
                        ("VpcId", vpc.as_str()),
                    ],
                )
                .await,
                "groupId",
            )
        }
    };
    let load_balancer_group = group("unreachable-alb", "the load balancers own group").await;
    let other_group = group("unreachable-other", "a group no load balancer is in").await;
    Network {
        vpc,
        subnets,
        load_balancer_group,
        other_group,
    }
}

/// Create the security group of one row's task, carrying exactly the inbound rule that row
/// describes, and answer its id.
async fn task_security_group(router: &MultiServiceRouter, network: &Network, row: &Row) -> String {
    let created = ec2(
        router,
        "CreateSecurityGroup",
        &[
            ("GroupName", row.name),
            ("GroupDescription", "the tasks own group"),
            ("VpcId", network.vpc.as_str()),
        ],
    )
    .await;
    let group = element(&created, "groupId");
    if let Some((protocol, from, to, source)) = row.rule {
        let mut params: Vec<(&str, &str)> = vec![
            ("GroupId", group.as_str()),
            ("IpPermissions.1.IpProtocol", protocol),
        ];
        if let Some(from) = from {
            params.push(("IpPermissions.1.FromPort", from));
        }
        if let Some(to) = to {
            params.push(("IpPermissions.1.ToPort", to));
        }
        let named = match source {
            Source::LoadBalancerGroup => network.load_balancer_group.as_str(),
            Source::OtherGroup => network.other_group.as_str(),
        };
        params.push(("IpPermissions.1.Groups.1.GroupId", named));
        ec2(router, "AuthorizeSecurityGroupIngress", &params).await;
    }
    group
}

/// Create the cluster and the revision every row's service runs: an `awsvpc`,
/// Fargate-compatible revision whose one container declares [`CONTAINER_PORT`].
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

/// Create one row's front door: its own Application Load Balancer, the `ip` target group the
/// row describes and a listener forwarding to it — which is what makes the group one the load
/// balancer health-checks at all.
///
/// Answered as `(the target group's ARN, the address the load balancer published)`.
async fn front_door(
    router: &MultiServiceRouter,
    network: &Network,
    row: &Row,
) -> (String, String) {
    let balanced = elbv2(
        router,
        "CreateLoadBalancer",
        &[
            ("Name", row.name),
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
    let mut params: Vec<(&str, &str)> = vec![
        ("Name", row.name),
        ("Protocol", "HTTP"),
        ("Port", "80"),
        ("TargetType", "ip"),
        ("VpcId", network.vpc.as_str()),
        ("HealthCheckProtocol", "HTTP"),
        ("HealthCheckPath", "/"),
        ("HealthCheckIntervalSeconds", INTERVAL_SECONDS),
        ("UnhealthyThresholdCount", THRESHOLD),
        ("HealthyThresholdCount", THRESHOLD),
    ];
    if let Some(port) = row.health_check_port {
        params.push(("HealthCheckPort", port));
    }
    let created = elbv2(router, "CreateTargetGroup", &params).await;
    let group = element(&created, "TargetGroupArn");
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

/// Create the ECS service one row describes: its tasks in the first subnet, in the security
/// group that row created, bound to `group` on the container's port.
async fn create_service(
    router: &MultiServiceRouter,
    network: &Network,
    row: &Row,
    group: &str,
    task_group: &str,
) {
    ecs_ok(
        router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": row.name,
            "taskDefinition": FAMILY,
            "desiredCount": row.desired,
            "launchType": "FARGATE",
            "networkConfiguration": {
                "awsvpcConfiguration": {
                    "subnets": [network.subnets[0]],
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
}

/// The ARNs of the RUNNING tasks of `service`, in one order, as Amazon ECS lists them.
async fn running_tasks(router: &MultiServiceRouter, service: &str) -> Vec<String> {
    let listed = ecs_ok(
        router,
        "ListTasks",
        json!({ "cluster": CLUSTER, "serviceName": service, "desiredStatus": "RUNNING" }),
    )
    .await;
    let mut arns: Vec<String> = listed["taskArns"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|arn| arn.as_str().map(str::to_string))
        .collect();
    arns.sort();
    arns
}

/// The private IPv4 addresses the interfaces of `arns` hold, in one order: the targets Amazon
/// ECS registers for those tasks.
async fn task_addresses(router: &MultiServiceRouter, case: &str, arns: &[String]) -> Vec<String> {
    let described = ecs_ok(
        router,
        "DescribeTasks",
        json!({ "cluster": CLUSTER, "tasks": arns }),
    )
    .await;
    let mut addresses: Vec<String> = described["tasks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|task| {
            task["attachments"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|attachment| attachment["details"].as_array())
                .flatten()
                .find(|detail| detail["name"].as_str() == Some("privateIPv4Address"))
                .and_then(|detail| detail["value"].as_str())
                .map(str::to_string)
        })
        .collect();
    assert_eq!(
        addresses.len(),
        arns.len(),
        "case {case:?}: every task carries its interface's address, got {described}"
    );
    addresses.sort();
    addresses
}

/// Wait until `service` reports `desired` running tasks, and answer their ARNs and addresses.
async fn placed_tasks(router: &MultiServiceRouter, row: &Row) -> (Vec<String>, Vec<String>) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let described = ecs_ok(
            router,
            "DescribeServices",
            json!({ "cluster": CLUSTER, "services": [row.name] }),
        )
        .await;
        if described["services"][0]["runningCount"] == json!(row.desired) {
            let arns = running_tasks(router, row.name).await;
            if arns.len() == row.desired {
                let addresses = task_addresses(router, row.case, &arns).await;
                return (arns, addresses);
            }
        }
        assert!(
            Instant::now() < deadline,
            "case {:?}: expected the service to run its {} task(s), got {described}",
            row.case,
            row.desired
        );
        ::tokio::time::sleep(POLL).await;
    }
}

/// One reported registration, as `DescribeTargetHealth` carries it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Reported {
    id: String,
    state: String,
    reason: String,
    description: String,
}

/// Every registration `group` reports, ordered by target id.
async fn health(router: &MultiServiceRouter, group: &str) -> Vec<Reported> {
    let body = elbv2(router, "DescribeTargetHealth", &[("TargetGroupArn", group)]).await;
    let mut reported: Vec<Reported> = members_of(&body)
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
        .collect();
    reported.sort();
    reported
}

/// Wait until every registration of `group` reports the row's state, and answer what it
/// reported together with every state seen on the way there.
async fn settled(
    router: &MultiServiceRouter,
    row: &Row,
    group: &str,
) -> (Vec<Reported>, Vec<String>) {
    let deadline = Instant::now() + DEADLINE;
    let mut seen: Vec<String> = Vec::new();
    loop {
        let reported = health(router, group).await;
        for entry in &reported {
            if !seen.contains(&entry.state) {
                seen.push(entry.state.clone());
            }
        }
        let settled = reported.len() == row.desired
            && reported.iter().all(|entry| entry.state == row.health.0);
        if settled {
            return (reported, seen);
        }
        assert!(
            Instant::now() < deadline,
            "case {:?}: expected {} registration(s) reporting {}, got {reported:?}",
            row.case,
            row.desired,
            row.health.0
        );
        ::tokio::time::sleep(POLL).await;
    }
}

/// Look at `name`'s load balancer until it reports `State.Code` `active`, which is when its
/// listener serves traffic on the name it published.
async fn active(router: &MultiServiceRouter, name: &str) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let body = elbv2(router, "DescribeLoadBalancers", &[("Names.member.1", name)]).await;
        let code = elements(&body, "Code").into_iter().next().unwrap_or_default();
        if code == "active" {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "setup: expected the load balancer {name} to reach active, got {code:?} in {body}"
        );
        ::tokio::time::sleep(POLL).await;
    }
}

/// Send one request to the address a load balancer published, carrying no AWS credential —
/// exactly as a client handed that name sends it.
async fn get(router: &MultiServiceRouter, dns_name: &str) -> (u16, String) {
    let request = http::Request::builder()
        .method("GET")
        .uri(format!("http://{dns_name}/"))
        .header("host", dns_name)
        .body(Bytes::new())
        .expect("a well-formed data-plane request");
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).to_string(),
    )
}

/// What one row's estate settled at, which is what the watch below re-reads.
struct Settled<'a> {
    row: &'a Row,
    group: String,
    dns_name: String,
    /// The task ARNs the service placed, and the addresses they are registered at.
    tasks: Vec<String>,
    addresses: Vec<String>,
}

/// Assert one row's registrations are still exactly the ones its tasks are registered at, in
/// the health it settled at — no entry drained, none registering again, and none healthy
/// where the checks cannot be delivered.
async fn still_holds(router: &MultiServiceRouter, settled: &Settled<'_>) {
    let row = settled.row;
    let reported = health(router, &settled.group).await;
    let ids: Vec<String> = reported.iter().map(|entry| entry.id.clone()).collect();
    assert_eq!(
        ids, settled.addresses,
        "case {:?}: expected the group to hold exactly its {} task(s) at {:?}, got {reported:?}",
        row.case, row.desired, settled.addresses
    );
    for entry in &reported {
        assert_eq!(
            (
                entry.state.as_str(),
                entry.reason.as_str(),
                entry.description.as_str()
            ),
            row.health,
            "case {:?}: expected {:?}, got {entry:?}",
            row.case,
            row.health
        );
    }
    // And the tasks themselves: the same ones, still running, with nothing stopped behind
    // them — a replacement would show as a new ARN here and as a stopped task beside it.
    let running = running_tasks(router, row.name).await;
    assert_eq!(
        running, settled.tasks,
        "case {:?}: expected the service's own tasks {:?} to go on running, got {running:?}",
        row.case, settled.tasks
    );
    let stopped = ecs_ok(
        router,
        "ListTasks",
        json!({ "cluster": CLUSTER, "serviceName": row.name, "desiredStatus": "STOPPED" }),
    )
    .await;
    let stopped_arns = stopped["taskArns"].as_array().map(Vec::len).unwrap_or(0);
    assert_eq!(
        stopped_arns, 0,
        "case {:?}: a target failing its load-balancer health check is no task the scheduler \
         stops, got {stopped}",
        row.case
    );
}

/// RULE: a registration whose health-check probe cannot be delivered settles at `unhealthy`
/// with `Target.Timeout`, is never reported `healthy`, and STAYS registered — one entry per
/// running task of the service, never a draining one — while a request the load balancer
/// cannot deliver is answered with its own status.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unreachable_ecs_target_settles_unhealthy_and_stays_registered() {
    let (router, engine) = emulator();
    let network = seed_network(&router).await;
    // The tasks below have to RUN, and ECS reads the path to the image registry off the
    // account's network records before it starts one.
    crate::ec2_network::register_internet_route(&engine, &network.vpc);
    seed_ecs(&router).await;

    let mut estate: Vec<Settled> = Vec::new();
    for row in &ROWS {
        let task_group = task_security_group(&router, &network, row).await;
        let (group, dns_name) = front_door(&router, &network, row).await;
        create_service(&router, &network, row, &group, &task_group).await;
        estate.push(Settled {
            row,
            group,
            dns_name,
            tasks: Vec::new(),
            addresses: Vec::new(),
        });
    }

    for entry in &mut estate {
        let (tasks, addresses) = placed_tasks(&router, entry.row).await;
        entry.tasks = tasks;
        entry.addresses = addresses;
        let (reported, seen) = settled(&router, entry.row, &entry.group).await;
        let ids: Vec<String> = reported.iter().map(|one| one.id.clone()).collect();
        assert_eq!(
            ids, entry.addresses,
            "case {:?}: the registrations are the tasks' own addresses, got {reported:?}",
            entry.row.case
        );
        for one in &reported {
            assert_eq!(
                (one.state.as_str(), one.reason.as_str(), one.description.as_str()),
                entry.row.health,
                "case {:?}: expected {:?}, got {one:?}",
                entry.row.case,
                entry.row.health
            );
        }
        if entry.row.health.0 != HEALTHY_STATE {
            // Nothing answers the checks, so the load balancer has not determined this
            // registration healthy at any point on the way to its verdict.
            assert!(
                !seen.iter().any(|state| state == HEALTHY_STATE),
                "case {:?}: a target whose checks cannot be delivered must never be reported \
                 {HEALTHY_STATE}, got the states {seen:?}",
                entry.row.case
            );
        }
        // And nothing deregistered any of them: `draining` is the state a deregistration
        // puts a registration in, and nobody scaled or redeployed these services.
        assert!(
            !seen.iter().any(|state| state == DRAINING_STATE),
            "case {:?}: no registration of a service nobody scaled drains, got the states \
             {seen:?}",
            entry.row.case
        );
    }

    // WATCHED, not read once: a scheduler that replaced the task behind a failing check, or a
    // group that drained its registration and started another, does it on the clock rather
    // than on a call, so the settled estate is re-read across several of the cadence the
    // checks themselves run at.
    let until = Instant::now() + WATCH_WINDOW;
    while Instant::now() < until {
        for entry in &estate {
            still_holds(&router, entry).await;
        }
        ::tokio::time::sleep(POLL).await;
    }

    // What a CLIENT gets: a load balancer whose group holds no target it can deliver the
    // request to answers its own status — a server error of its own, never a target's
    // response and never a 200.
    for entry in &estate {
        if !entry.row.unreachable {
            continue;
        }
        active(&router, entry.row.name).await;
        let (status, body) = get(&router, &entry.dns_name).await;
        assert!(
            (500..600).contains(&status),
            "case {:?}: expected the load balancer's own server error for a target it cannot \
             reach, got {status} with body {body:?}",
            entry.row.case
        );
    }
}
