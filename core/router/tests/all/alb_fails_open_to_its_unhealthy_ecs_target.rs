// SPDX-License-Identifier: CC0-1.0

//! An Application Load Balancer in front of an Amazon ECS service, over the three answers a
//! client gets as its one target's health moves.
//!
//! RULE: **a `forward` sends the request to a target the group holds — to one the load balancer
//! may send requests to where it holds such a target, and across its UNHEALTHY ones where it
//! holds none, so a group whose only target is failing its checks still serves the target's own
//! response (the load balancer fails open) and only a group with no target to send the request
//! to earns the load balancer's own `503`. Whether those checks pass is decided by the ANSWER
//! the target gives the group's `HealthCheckPath`: a status the group's `Matcher` does not count
//! as a success fails the check, and the registration reaches `unhealthy` with
//! `Target.ResponseCodeMismatch`, while a path the container serves returns it to `healthy`.**
//!
//! `HealthCheckPath` is "[HTTP/HTTPS health checks] The destination for health checks on the
//! targets" and `Matcher` "[HTTP/HTTPS health checks] The HTTP or gRPC codes to use when
//! checking for a successful response from a target"
//! (`smithy_query(query="doc",shape="CreateTargetGroupInput",member="Matcher")`), and
//! `TargetHealth$Reason` gives the verdict's reason: "`Target.ResponseCodeMismatch` - The health
//! checks did not return an expected HTTP code".
//!
//! Three crates have to agree for that to hold, which is why this suite lives here: Amazon ECS
//! runs the container and records the server it listens with at the task's own address, EC2 owns
//! the network and the security-group rule that lets a check arrive, and Elastic Load Balancing
//! v2 checks the registration, reports its health and answers the client. Everything is driven
//! through the real operations on one engine, and every wait polls the CONDITION rather than a
//! fixed time.

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
// The ONE hand-written raw HTTP/1.1 exchange on a load balancer's own published address and the
// in-process request beside it, from the sibling suite that owns the fixtures for this topology —
// nothing here keeps a second copy of them.
use crate::ecs_alb_serves_its_awsvpc_tasks_container::{dialled, get};

/// The region every request below is signed for.
const REGION: &str = "us-east-1";

/// The API versions an ec2Query and an awsQuery request carry.
const EC2_API_VERSION: &str = "2016-11-15";
const ELB_VERSION: &str = "2015-12-01";

/// The status every control-plane call of the setup must answer.
const OK: u16 = 200;

/// The cluster, family, container and service this suite runs.
const CLUSTER: &str = "failopen";
const FAMILY: &str = "failopenfamily";
const CONTAINER: &str = "app";
const SERVICE: &str = "failopensvc";

/// The port the container's server listens on, which is the `containerPort` the revision
/// declares, the port the binding names and the target group's own.
const CONTAINER_PORT: i64 = 80;

/// The network this suite runs in.
const VPC_CIDR: &str = "10.60.0.0/16";
const SUBNET_CIDRS: [(&str, &str); 2] = [
    ("10.60.1.0/24", "us-east-1a"),
    ("10.60.2.0/24", "us-east-1b"),
];

/// The `HealthCheckIntervalSeconds` and the two thresholds the target group is created with: a
/// verdict takes a threshold of consecutive checks, each one interval after the last, so this
/// suite asks for the shortest interval the member admits ("the range is 5-300") and the fewest
/// checks a threshold may ask for ("the range is 2-10").
const INTERVAL_SECONDS: &str = "5";
const THRESHOLD: &str = "2";

/// The path the container's own program writes a document at, and a path it holds nothing at —
/// which the group's health check is pointed at first.
const SERVED_PATH: &str = "/";
const MISSING_PATH: &str = "/not-deployed";

/// The marker the container's program writes into the document it serves, so a body anything
/// else produced fails an assertion.
const MARKER: &str = "FAIL-OPEN-CONTAINER-BODY";

/// The statuses the client is answered with: the target's own for the document it holds, and the
/// load balancer's own for a group it has no target to send the request to.
const SERVED_STATUS: u16 = 200;
const NO_TARGET_STATUS: u16 = 503;

/// The status a static server answers a path its document root holds nothing at, which is what
/// the health check's first destination earns.
const NOT_FOUND_STATUS: u16 = 404;

/// The `TargetHealth$State` values this suite reads back, and the reason an answer the matcher
/// rejects carries.
const HEALTHY: &str = "healthy";
const UNHEALTHY: &str = "unhealthy";
const DRAINING: &str = "draining";
const RESPONSE_CODE_MISMATCH: &str = "Target.ResponseCodeMismatch";

/// How long a step waits for a clock-owed move — the task's start, the registration it owes, the
/// verdict the group's cadence reaches — and how often it looks. Generous on purpose: the
/// deadline is only here so a failure is a failure rather than a hang.
const DEADLINE: Duration = Duration::from_secs(120);
const POLL: Duration = Duration::from_millis(50);

/// One emulator carrying EC2, Elastic Load Balancing v2 and ECS over ONE shared engine, exactly
/// as the production router builds them.
fn emulator() -> MultiServiceRouter {
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

/// Send one form-encoded request to the service `signing_name` addresses, and answer its status
/// and body.
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

/// The network the service and the load balancer run in: a VPC with two subnets, an internet
/// gateway and a route off the VPC (which a task needs to fetch its image), the load balancer's
/// own security group, and the group the tasks are placed in — whose one inbound rule admits the
/// load balancer's checks on the container's port.
struct Network {
    vpc: String,
    subnets: Vec<String>,
    load_balancer_group: String,
    task_group: String,
}

/// Create that network through EC2 itself, so every id asserted against is one EC2 minted.
async fn seed_network(router: &MultiServiceRouter) -> Network {
    let vpc = element(
        &ec2(router, "CreateVpc", &[("CidrBlock", VPC_CIDR)]).await,
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
    let group = |name: &str, description: &str| {
        let vpc = vpc.clone();
        let name = name.to_string();
        let description = description.to_string();
        async move {
            element(
                &ec2(
                    router,
                    "CreateSecurityGroup",
                    &[
                        ("GroupName", name.as_str()),
                        ("GroupDescription", description.as_str()),
                        ("VpcId", vpc.as_str()),
                    ],
                )
                .await,
                "groupId",
            )
        }
    };
    let load_balancer_group = group("failopen-alb", "the load balancers own group").await;
    let task_group = group("failopen-tasks", "the group the tasks are placed in").await;
    ec2(
        router,
        "AuthorizeSecurityGroupIngress",
        &[
            ("GroupId", task_group.as_str()),
            ("IpPermissions.1.IpProtocol", "tcp"),
            ("IpPermissions.1.FromPort", &CONTAINER_PORT.to_string()),
            ("IpPermissions.1.ToPort", &CONTAINER_PORT.to_string()),
            (
                "IpPermissions.1.Groups.1.GroupId",
                load_balancer_group.as_str(),
            ),
        ],
    )
    .await;
    Network {
        vpc,
        subnets,
        load_balancer_group,
        task_group,
    }
}

/// Register the `awsvpc`, Fargate-compatible revision whose one container writes its document
/// and then runs the image's own server in the foreground, and answer its ARN.
async fn register_revision(router: &MultiServiceRouter) -> String {
    let command = format!(
        "echo {MARKER} > /usr/share/nginx/html/index.html && nginx -g 'daemon off;'"
    );
    let registered = ecs_ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": FAMILY,
            "networkMode": "awsvpc",
            "requiresCompatibilities": ["FARGATE"],
            "cpu": "256",
            "memory": "512",
            "containerDefinitions": [{
                "name": CONTAINER,
                "image": "public.ecr.aws/docker/library/nginx:alpine",
                "memory": 128,
                "portMappings": [{ "containerPort": CONTAINER_PORT }],
                "command": ["/bin/sh", "-c", command],
            }],
        }),
    )
    .await;
    registered["taskDefinition"]["taskDefinitionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("the registration must answer an ARN, got {registered}"))
        .to_string()
}

/// Create the `ip` target group whose health check requests `path`, the internet-facing load
/// balancer and the listener that forwards to it — and answer the group's ARN with the address
/// the load balancer published.
async fn front_door(router: &MultiServiceRouter, network: &Network, path: &str) -> (String, String) {
    let created = elbv2(
        router,
        "CreateTargetGroup",
        &[
            ("Name", "failopen-group"),
            ("Protocol", "HTTP"),
            ("Port", &CONTAINER_PORT.to_string()),
            ("TargetType", "ip"),
            ("VpcId", network.vpc.as_str()),
            ("HealthCheckProtocol", "HTTP"),
            ("HealthCheckPath", path),
            ("HealthCheckIntervalSeconds", INTERVAL_SECONDS),
            ("HealthyThresholdCount", THRESHOLD),
            ("UnhealthyThresholdCount", THRESHOLD),
        ],
    )
    .await;
    let group = opt_element(&created, "TargetGroupArn")
        .unwrap_or_else(|| panic!("the create must answer a TargetGroupArn, got {created}"));
    let balanced = elbv2(
        router,
        "CreateLoadBalancer",
        &[
            ("Name", "failopen-lb"),
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

/// Create the ECS service of `revision`, bound to `group` on the container's port, with one task.
async fn create_service(
    router: &MultiServiceRouter,
    network: &Network,
    revision: &str,
    group: &str,
) {
    ecs_ok(
        router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": SERVICE,
            "taskDefinition": revision,
            "desiredCount": 1,
            "launchType": "FARGATE",
            "networkConfiguration": {
                "awsvpcConfiguration": {
                    "subnets": [network.subnets[0]],
                    "securityGroups": [network.task_group],
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
/// Polls the CONDITION — the task's start, the registration that start owes and the verdict its
/// checks reach are all clock-owed — rather than waiting a fixed time.
async fn reaching(
    router: &MultiServiceRouter,
    case: &str,
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
            let described = ecs_ok(
                router,
                "DescribeServices",
                json!({ "cluster": CLUSTER, "services": [SERVICE] }),
            )
            .await;
            panic!(
                "case {case:?}: expected one registration reported {state} {reason:?}, got \
                 {reported:?}; the service reports {described}"
            );
        }
        ::tokio::time::sleep(POLL).await;
    }
}

/// Look at the load balancer until it reports `State.Code` `active`, which is when its listener
/// serves traffic on the name it published.
async fn active(router: &MultiServiceRouter, dns_name: &str) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let body = elbv2(router, "DescribeLoadBalancers", &[("Names.member.1", "failopen-lb")]).await;
        let code = elements(&body, "Code").into_iter().next().unwrap_or_default();
        if code == "active" {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "setup: expected the load balancer {dns_name} to reach active, got {code:?} in {body}"
        );
        ::tokio::time::sleep(POLL).await;
    }
}

/// RULE: the load balancer fails open to the unhealthy target its group holds, returns it to
/// service once its checks pass again, and answers its own 503 only once the group has no target
/// to send the request to.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_alb_fails_open_to_its_unhealthy_ecs_target_and_503s_with_no_target() {
    let router = emulator();
    let network = seed_network(&router).await;
    ecs_ok(&router, "CreateCluster", json!({ "clusterName": CLUSTER })).await;
    let revision = register_revision(&router).await;
    // The group's health check is pointed at a path the container holds nothing at, so the
    // target's own answer to it is the status a static server gives such a path.
    let (group, dns_name) = front_door(&router, &network, MISSING_PATH).await;
    create_service(&router, &network, &revision, &group).await;
    active(&router, &dns_name).await;

    // The checks were ANSWERED, and the answer is not one the group's matcher counts as a
    // success: the registration reaches `unhealthy` with the model's own reason for that, which
    // names the status the checks got back.
    let case = "the only target answering the checked path outside the matcher";
    let unhealthy = reaching(
        &router,
        case,
        &group,
        UNHEALTHY,
        Some(RESPONSE_CODE_MISMATCH),
    )
    .await;
    assert_eq!(
        unhealthy.description,
        format!("Health checks failed with these codes: [{NOT_FOUND_STATUS}]"),
        "case {case:?}: the reason must be described by the status the checks got back, got \
         {unhealthy:?}"
    );
    assert!(
        unhealthy.id.starts_with("10.60."),
        "case {case:?}: the registration must name the task's own address in the VPC, got \
         {unhealthy:?}"
    );

    // FAIL OPEN: the group holds no healthy target, so the request goes to the unhealthy one
    // anyway and the client is answered with the CONTAINER's own response.
    let answered = get(&router, &dns_name, SERVED_PATH).await;
    assert_eq!(
        answered.status, SERVED_STATUS,
        "case {case:?}: expected the unhealthy target's own {SERVED_STATUS}, got {} with body \
         {:?}",
        answered.status, answered.body
    );
    assert!(
        answered.body.contains(MARKER),
        "case {case:?}: expected the container's own document, got {:?}",
        answered.body
    );
    // And over a REAL connection to the address the load balancer published, which is what a
    // client handed that name dials: the same fail-open answer, off the socket.
    let off_the_socket = dialled(case, &dns_name, SERVED_PATH).await;
    assert_eq!(
        off_the_socket.status, SERVED_STATUS,
        "case {case:?}: expected the unhealthy target's own {SERVED_STATUS} on the published \
         address, got {} with body {:?}",
        off_the_socket.status, off_the_socket.body
    );
    assert!(
        off_the_socket.body.contains(MARKER),
        "case {case:?}: expected the container's own document on the published address, got {:?}",
        off_the_socket.body
    );

    // RESTORED: the check is pointed at the path the container serves, so its checks pass and the
    // registration returns to `healthy` with no reason at all.
    let restored = "the health check pointed back at the path the container serves";
    elbv2(
        &router,
        "ModifyTargetGroup",
        &[
            ("TargetGroupArn", group.as_str()),
            ("HealthCheckPath", SERVED_PATH),
        ],
    )
    .await;
    let healthy = reaching(&router, restored, &group, HEALTHY, None).await;
    assert_eq!(
        (healthy.reason.as_str(), healthy.description.as_str()),
        ("", ""),
        "case {restored:?}: a healthy registration carries no reason and no description, got \
         {healthy:?}"
    );
    let answered = get(&router, &dns_name, SERVED_PATH).await;
    assert_eq!(
        answered.status, SERVED_STATUS,
        "case {restored:?}: expected the healthy target's own {SERVED_STATUS}, got {} with body \
         {:?}",
        answered.status, answered.body
    );

    // NO TARGET TO SEND IT TO: the one registration is deregistered, which stops it receiving new
    // requests while it drains out of the group — so the load balancer has no target for this
    // request and answers its own status, carrying no document of the target.
    let drained = "the only target deregistered";
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
    reaching(&router, drained, &group, DRAINING, None).await;
    let answered = get(&router, &dns_name, SERVED_PATH).await;
    assert_eq!(
        answered.status, NO_TARGET_STATUS,
        "case {drained:?}: expected the load balancer's own {NO_TARGET_STATUS}, got {} with body \
         {:?}",
        answered.status, answered.body
    );
    assert!(
        !answered.body.contains(MARKER),
        "case {drained:?}: the load balancer's own status carries no document of the target, got \
         {:?}",
        answered.body
    );
    let off_the_socket = dialled(drained, &dns_name, SERVED_PATH).await;
    assert_eq!(
        off_the_socket.status, NO_TARGET_STATUS,
        "case {drained:?}: expected the load balancer's own {NO_TARGET_STATUS} on the published \
         address, got {} with body {:?}",
        off_the_socket.status, off_the_socket.body
    );
}
