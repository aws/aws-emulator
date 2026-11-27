// SPDX-License-Identifier: CC0-1.0

//! An Application Load Balancer forwarding to the `ip` target an AWSVPC Amazon ECS task
//! occupies is answered by the server that task's CONTAINER started.
//!
//! RULE: a request a `forward` action sends to a healthy `ip` target that is an `awsvpc` Amazon
//! ECS task is answered with the status, media type and bytes the server the task's container
//! STARTED answers for it, served out of the container's own filesystem on the port that server
//! listens on; a target whose container started no server is one the load balancer cannot
//! establish a connection to, and only then does it answer a status of its own.
//!
//! The grounds. `LoadBalancer$targetGroupArn` binds a service's tasks into a target group and
//! "tasks that use the `awsvpc` network mode are associated with an elastic network interface",
//! so the target is the task's own address; `TargetDescription$Port` is "the port on which the
//! target is listening"; and what is listening there is what the container's own
//! `entryPoint`/`command` — "the command that's passed to the container" — started, which for a
//! web server image is that server in the foreground. The document it answers with is the file
//! the request's path names under the directory that server serves, which is a directory of the
//! container's own filesystem.
//!
//! Three crates have to agree for that to hold, which is why this suite lives here: Amazon ECS
//! owns the task, its interface's address and the container's program, Amazon EC2 owns the
//! security-group rules that decide whether the load balancer's health check arrives, and
//! Elastic Load Balancing v2 holds the registration and answers the client. Everything is
//! driven through the real operations on ONE shared engine, so every assertion is about what
//! each service persisted.
//!
//! The rows differ along what the rule generalises over: which server the image ships and under
//! which command it runs it (the `nginx` image's own command, the Apache HTTP Server image's
//! foreground wrapper behind an `exec`, and the interpreter's own HTTP server module), the port
//! it listens on and the binding names, the directory it serves, a path that directory holds
//! nothing at (the TARGET's own 404), and a container whose program started no server at all
//! (the load balancer's own 502, because nothing is listening).
//!
//! The second half pins the NEGATIVE contract that stands on that converged state: a
//! registration of a Fargate-compatible revision in the `bridge` network mode is refused, an
//! update that moves a live Fargate service onto a revision that does not support that launch
//! type is refused, neither refusal changes what the service runs or what the load balancer
//! serves, and an update onto a repaired `awsvpc` revision rolls the primary deployment to
//! `COMPLETED` and serves that revision's own bytes.
//!
//! Every clock-owed move — the task's start, the registration it owes, the health the checks
//! determine — is POLLED for until it holds, never slept for, and each deadline is generous: a
//! row asserts that the move HAPPENS, not how fast this host runs.

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

/// The ECS cluster every service below runs in, and the container name every binding names.
///
/// `pub(crate)` for the reason [`emulator`] gives: the sibling suite that stands THIS topology up
/// to measure another rule on it drives the same cluster and the same binding through these
/// helpers.
pub(crate) const CLUSTER: &str = "served";
pub(crate) const CONTAINER: &str = "app";

/// The network every row runs in.
const VPC_CIDR: &str = "10.90.0.0/16";
const SUBNET_CIDRS: [(&str, &str); 2] = [
    ("10.90.1.0/24", "us-east-1a"),
    ("10.90.2.0/24", "us-east-1b"),
];

/// The `HealthCheckIntervalSeconds` every target group below is created with: the shortest the
/// member admits ("the range is 5-300"), so a suite waiting for a verdict waits the least the
/// configuration allows.
const HEALTH_CHECK_INTERVAL_SECONDS: &str = "5";

/// The `TargetHealth$State` a target the load balancer's checks reached carries.
pub(crate) const HEALTHY: &str = "healthy";

/// How long a clock-owed move is waited for before a row gives up, and how often it looks
/// again. Generous on purpose: the deadline is here so a failure is a failure rather than a
/// hang.
const DEADLINE: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_millis(100);

/// The status a static server answers the document it was asked for with, the one it answers a
/// path its document root holds nothing at with, and the one the load balancer answers for a
/// target it could not establish a connection to.
const OK: u16 = 200;
const NOT_FOUND: u16 = 404;
const TARGET_UNREACHABLE: u16 = 502;

/// One row: the container's program, where the server it starts listens, what the client asks
/// for, and what the client must be answered with.
struct Row {
    case: &'static str,
    /// The names the row's own resources take.
    family: &'static str,
    group_name: &'static str,
    load_balancer_name: &'static str,
    service_name: &'static str,
    /// The container's `command`, which is what starts the server.
    command: &'static [&'static str],
    /// The port the server listens on, which is the `containerPort` the revision declares and
    /// the binding names, and the target group's own port.
    port: i64,
    /// What the client asks the load balancer for.
    path: &'static str,
    /// The status it must be answered with, and the first whitespace-separated field that
    /// answer's body must carry — the marker the container's own program wrote.
    status: u16,
    body: &'static str,
    /// Whether the body carries the container's own HOSTNAME as a second field, which is the
    /// name the runtime gave it: the private DNS hostname of the address its task holds.
    hostname_in_body: bool,
}

/// The marker each row's program writes into the directory its server serves, so a body
/// anything but that program produced fails the row.
const MARKER: &str = "CONTAINER-SERVED-BODY";

const ROWS: &[Row] = &[
    Row {
        case: "the nginx image's own command, over the page its program wrote",
        family: "servednginx",
        group_name: "served-nginx",
        load_balancer_name: "served-nginx-lb",
        service_name: "servednginx",
        command: &[
            "/bin/sh",
            "-c",
            "echo CONTAINER-SERVED-BODY > /usr/share/nginx/html/index.html && \
             nginx -g 'daemon off;'",
        ],
        port: 80,
        path: "/",
        status: OK,
        body: MARKER,
        hostname_in_body: false,
    },
    Row {
        case: "the Apache HTTP Server image's foreground command, reached through exec",
        family: "servedhttpd",
        group_name: "served-httpd",
        load_balancer_name: "served-httpd-lb",
        service_name: "servedhttpd",
        command: &[
            "/bin/sh",
            "-c",
            "echo CONTAINER-SERVED-BODY > /usr/local/apache2/htdocs/index.html && \
             exec httpd-foreground",
        ],
        port: 80,
        path: "/",
        status: OK,
        body: MARKER,
        hostname_in_body: false,
    },
    Row {
        case: "the interpreter's own server module on the port and directory it names",
        family: "servedmodule",
        group_name: "served-module",
        load_balancer_name: "served-module-lb",
        service_name: "servedmodule",
        command: &[
            "/bin/sh",
            "-c",
            "mkdir -p /srv/app && echo CONTAINER-SERVED-BODY > /srv/app/index.html && \
             python3 -m http.server 8080 --directory /srv/app",
        ],
        port: 8080,
        path: "/",
        status: OK,
        body: MARKER,
        hostname_in_body: false,
    },
    Row {
        case: "the small server's own invocation, over a page carrying the container's hostname",
        family: "servedsmall",
        group_name: "served-small",
        load_balancer_name: "served-small-lb",
        service_name: "servedsmall",
        command: &[
            "/bin/sh",
            "-c",
            "mkdir -p /www && echo \"CONTAINER-SERVED-BODY $(hostname)\" > /www/index.html && \
             httpd -f -p 80 -h /www",
        ],
        port: 80,
        path: "/",
        status: OK,
        body: MARKER,
        hostname_in_body: true,
    },
    Row {
        case: "a path the served directory holds nothing at is the target's own 404",
        family: "servedmissing",
        group_name: "served-missing",
        load_balancer_name: "served-missing-lb",
        service_name: "servedmissing",
        command: &[
            "/bin/sh",
            "-c",
            "echo CONTAINER-SERVED-BODY > /usr/share/nginx/html/index.html && \
             nginx -g 'daemon off;'",
        ],
        port: 80,
        path: "/nothing-here",
        status: NOT_FOUND,
        body: "",
        hostname_in_body: false,
    },
    // The registration of this row is reported `healthy` all the same, and that is the
    // emulator's own rule rather than an accident of this suite: a task is registered and
    // health-checked on the `containerPort` its REVISION declares — "for containers in a task
    // with the `awsvpc` network mode, the `hostPortRange` is set to the same value as the
    // `containerPortRange`" — which is the fact
    // `ecs_task_registration_precedes_its_health_verdict.rs` pins, and it is decided by the
    // declaration and by whether the check arrives at all. WHAT the target then serves is the
    // separate fact this suite is about, and it is decided by the container's program; this row
    // is the pair coming apart, which is what makes the load balancer's own status the answer.
    Row {
        case: "a container whose program started no server is a target nothing answers at",
        family: "servedsilent",
        group_name: "served-silent",
        load_balancer_name: "served-silent-lb",
        service_name: "servedsilent",
        command: &[
            "/bin/sh",
            "-c",
            "echo CONTAINER-SERVED-BODY > /usr/share/nginx/html/index.html && sleep infinity",
        ],
        port: 80,
        path: "/",
        status: TARGET_UNREACHABLE,
        body: "",
        hostname_in_body: false,
    },
];

/// One emulator carrying EC2, Elastic Load Balancing v2 and ECS over ONE shared engine, exactly
/// as the production router builds them.
///
/// `pub(crate)` — with the helpers below it — because this suite OWNS the fixture for "an
/// Application Load Balancer in front of an `awsvpc` Amazon ECS service": the network, the front
/// door, the revision and the service. A sibling suite measuring another rule of that same
/// topology calls these rather than keeping a second copy of them, exactly as every awsQuery suite
/// calls the one `all/query_wire.rs`.
pub(crate) fn emulator() -> MultiServiceRouter {
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
///
/// `pub(crate)` for the reason [`emulator`] gives: a sibling suite measuring another rule of this
/// topology drives Elastic Load Balancing v2 through this one call rather than keeping a second
/// awsQuery sender of its own.
pub(crate) async fn elbv2(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
) -> String {
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

/// One awsJson1_1 call to ECS, answered with its status and parsed body.
pub(crate) async fn ecs(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
) -> (u16, Value) {
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
    (status, parsed)
}

/// One ECS call that must succeed, answered with its parsed body.
pub(crate) async fn ecs_ok(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let (status, parsed) = ecs(router, operation, body).await;
    assert_eq!(
        status, OK,
        "setup: ECS {operation} answered {status} with {parsed}"
    );
    parsed
}

/// The network every row runs in: a VPC with two subnets and an internet gateway, the load
/// balancers' own security group, and the group the tasks are placed in — which admits the load
/// balancers' checks and traffic on both ports a row may serve on.
pub(crate) struct Network {
    pub(crate) vpc: String,
    pub(crate) subnets: Vec<String>,
    pub(crate) load_balancer_group: String,
    pub(crate) task_group: String,
}

/// Create that network through EC2 itself, so every id asserted against is one EC2 minted.
pub(crate) async fn seed_network(router: &MultiServiceRouter) -> Network {
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
    // The path off the VPC a task needs to fetch the image its container runs: Amazon ECS reads
    // the route table that applies to the placement's subnet before it starts a task, and a
    // placement with no route off the VPC cannot reach a public registry. Written through Amazon
    // EC2's own operations, so the table, the route and the association are that service's.
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
    let load_balancer_group = group("served-alb", "the load balancers own group").await;
    let task_group = group("served-tasks", "the group the tasks are placed in").await;
    // The ONE rule this suite authorizes: the tasks' group takes the load balancers' own group
    // as a source on every port a row serves on, which is what lets a check of a target in it
    // arrive.
    ec2(
        router,
        "AuthorizeSecurityGroupIngress",
        &[
            ("GroupId", task_group.as_str()),
            ("IpPermissions.1.IpProtocol", "tcp"),
            ("IpPermissions.1.FromPort", "80"),
            ("IpPermissions.1.ToPort", "8080"),
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

/// Register one `awsvpc`, Fargate-compatible revision of `family` whose single container
/// listens on `port` and runs `command`, and answer its ARN.
pub(crate) async fn register_revision(
    router: &MultiServiceRouter,
    family: &str,
    port: i64,
    command: &[&str],
) -> String {
    let registered = ecs_ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
            "networkMode": "awsvpc",
            "requiresCompatibilities": ["FARGATE"],
            "cpu": "256",
            "memory": "512",
            "containerDefinitions": [{
                "name": CONTAINER,
                "image": "public.ecr.aws/docker/library/nginx:alpine",
                "memory": 128,
                "portMappings": [{ "containerPort": port }],
                "command": command,
            }],
        }),
    )
    .await;
    registered["taskDefinition"]["taskDefinitionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("the registration must answer an ARN, got {registered}"))
        .to_string()
}

/// Create the `ip` target group, the internet-facing load balancer and the listener that
/// forwards to it, and answer the group's ARN with the address the load balancer published.
pub(crate) async fn front_door(
    router: &MultiServiceRouter,
    network: &Network,
    case: &str,
    group_name: &str,
    load_balancer_name: &str,
    port: i64,
) -> (String, String) {
    let created = elbv2(
        router,
        "CreateTargetGroup",
        &[
            ("Name", group_name),
            ("Protocol", "HTTP"),
            ("Port", &port.to_string()),
            ("TargetType", "ip"),
            ("VpcId", network.vpc.as_str()),
            ("HealthCheckProtocol", "HTTP"),
            ("HealthCheckPath", "/"),
            (
                "HealthCheckIntervalSeconds",
                HEALTH_CHECK_INTERVAL_SECONDS,
            ),
        ],
    )
    .await;
    let group = opt_element(&created, "TargetGroupArn")
        .unwrap_or_else(|| panic!("case {case:?}: no TargetGroupArn in {created}"));
    let balanced = elbv2(
        router,
        "CreateLoadBalancer",
        &[
            ("Name", load_balancer_name),
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

/// Create one ECS service of `revision`, bound to `group` on `port`, with one task.
pub(crate) async fn create_service(
    router: &MultiServiceRouter,
    network: &Network,
    service_name: &str,
    revision: &str,
    group: &str,
    port: i64,
) {
    ecs_ok(
        router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": service_name,
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
                "containerPort": port,
            }],
            "healthCheckGracePeriodSeconds": 120,
        }),
    )
    .await;
}

/// One registration as `DescribeTargetHealth` reports it: the address it names, the port it
/// receives traffic on, the state its checks reached and — for a state the model gives them —
/// that state's reason code and description, each read as empty where the answer carries none.
///
/// `pub(crate)` for the reason [`emulator`] gives: the whole description belongs to the suite
/// that owns this topology's fixtures, so a sibling suite whose rule turns on the REASON a
/// verdict carries reads it through this rather than keeping a reader of its own.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ReportedHealth {
    pub(crate) id: String,
    pub(crate) port: String,
    pub(crate) state: String,
    pub(crate) reason: String,
    pub(crate) description: String,
}

/// Every registration `group` reports, in the order the answer carries them.
pub(crate) async fn reported_health(
    router: &MultiServiceRouter,
    group: &str,
) -> Vec<ReportedHealth> {
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
            ReportedHealth {
                id: text(description, "Id"),
                port: text(description, "Port"),
                state: text(health, "State"),
                reason: text(health, "Reason"),
                description: text(health, "Description"),
            }
        })
        .collect()
}

/// The `(Id, State)` of every registration `group` reports — the two members most rows of this
/// topology judge a registration by, read through the ONE reading of the answer
/// ([`reported_health`]).
pub(crate) async fn health(router: &MultiServiceRouter, group: &str) -> Vec<(String, String)> {
    reported_health(router, group)
        .await
        .into_iter()
        .map(|entry| (entry.id, entry.state))
        .collect()
}

/// Wait until `group` reports exactly one registration and that registration is `healthy`,
/// answering the address it is registered at.
///
/// One target is the shape most rows want; a service whose desired count places several is waited
/// for through [`healthy_targets`], which this delegates to so there is ONE poll of this condition.
pub(crate) async fn healthy_target(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
    service_name: &str,
) -> String {
    healthy_targets(router, case, group, service_name, 1)
        .await
        .into_iter()
        .next()
        .expect("a settled poll answers one address per expected registration")
}

/// Wait until `group` reports `expected` registrations and every one of them is `healthy`,
/// answering the addresses they name, in order.
///
/// Polls the CONDITION — the tasks' start, the registrations that start owes and the health their
/// checks determine are all clock-owed — rather than waiting a fixed time.
pub(crate) async fn healthy_targets(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
    service_name: &str,
    expected: usize,
) -> Vec<String> {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let reported = health(router, group).await;
        let settled =
            reported.len() == expected && reported.iter().all(|(_, state)| state == HEALTHY);
        if settled {
            let mut addresses: Vec<String> =
                reported.into_iter().map(|(address, _)| address).collect();
            addresses.sort();
            return addresses;
        }
        if Instant::now() >= deadline {
            // What the service itself says about the tasks it placed, so a row that never
            // converges names the reason rather than only the empty read.
            let described = ecs_ok(
                router,
                "DescribeServices",
                json!({ "cluster": CLUSTER, "services": [service_name] }),
            )
            .await;
            panic!(
                "case {case:?}: expected {expected} registration(s) reported {HEALTHY}, got \
                 {reported:?}; the service reports {described}"
            );
        }
        ::tokio::time::sleep(POLL).await;
    }
}

/// The hostname the container of the task registered at `address` carries, which is the private
/// DNS hostname of that very address — so a body carrying it is a body that task's own container
/// wrote.
///
/// `pub(crate)` for the reason [`emulator`] gives: the rule belongs once, beside the fixtures every
/// suite of this topology reads it with.
pub(crate) fn container_hostname(address: &str) -> String {
    format!("ip-{}", address.replace('.', "-"))
}

/// One answered data-plane request: the status and the body, as the wire carries them.
pub(crate) struct Answered {
    pub(crate) status: u16,
    pub(crate) body: String,
}

/// Send one request to the address the load balancer published, carrying no AWS credential —
/// exactly as a client handed that name sends it.
pub(crate) async fn get(router: &MultiServiceRouter, dns_name: &str, path: &str) -> Answered {
    let request = http::Request::builder()
        .method("GET")
        .uri(format!("http://{dns_name}{path}"))
        .header("host", dns_name)
        .body(Bytes::new())
        .expect("a well-formed data-plane request");
    let response = router.dispatch(request).await;
    Answered {
        status: response.status().as_u16(),
        body: String::from_utf8_lossy(response.body()).to_string(),
    }
}

/// How long one exchange on the published address waits to connect, to send and to be answered.
/// Generous on purpose, for the same reason [`DEADLINE`] is.
const EXCHANGE_DEADLINE: Duration = Duration::from_secs(30);

/// DIAL the address the load balancer published and exchange one request on it, as the client
/// handed that address does: a connection in the clear, HTTP/1.1, with the authority it dialled as
/// `Host`.
///
/// What [`get`] asserts of the PLANE, this asserts of the ADDRESS. Only the port is taken from the
/// published authority, because the core binds the socket it opens for a load balancer on
/// loopback.
///
/// `pub(crate)` for the reason [`emulator`] gives, and because there is ONE hand-written raw HTTP/1.1
/// exchange for this topology rather than one per suite: every suite that dials a load balancer's
/// own address calls this.
pub(crate) async fn dialled(case: &str, dns_name: &str, path: &str) -> Answered {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let port = dns_name
        .rsplit(':')
        .next()
        .and_then(|port| port.parse::<u16>().ok())
        .unwrap_or_else(|| {
            panic!("case {case:?}: the published address {dns_name:?} carries no port to dial")
        });
    let mut stream = tokio::time::timeout(
        EXCHANGE_DEADLINE,
        tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)),
    )
    .await
    .unwrap_or_else(|_| panic!("case {case:?}: {dns_name} did not accept a connection"))
    .unwrap_or_else(|error| panic!("case {case:?}: {dns_name} refused a connection: {error}"));
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {dns_name}\r\nAccept: */*\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: the request was not sent: {error}"));
    let mut answer = Vec::new();
    tokio::time::timeout(EXCHANGE_DEADLINE, stream.read_to_end(&mut answer))
        .await
        .unwrap_or_else(|_| panic!("case {case:?}: {dns_name} did not answer in time"))
        .unwrap_or_else(|error| panic!("case {case:?}: the answer was not read: {error}"));
    let raw = String::from_utf8_lossy(&answer).to_string();
    let status = raw
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("case {case:?}: no status line in {raw:?}"));
    let body = raw
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_string())
        .unwrap_or_default();
    Answered { status, body }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_load_balancer_answers_with_the_ecs_containers_own_server() {
    let router = emulator();
    let network = seed_network(&router).await;
    ecs_ok(&router, "CreateCluster", json!({ "clusterName": CLUSTER })).await;
    for row in ROWS {
        let revision = register_revision(&router, row.family, row.port, row.command).await;
        let (group, dns_name) = front_door(
            &router,
            &network,
            row.case,
            row.group_name,
            row.load_balancer_name,
            row.port,
        )
        .await;
        create_service(
            &router,
            &network,
            row.service_name,
            &revision,
            &group,
            row.port,
        )
        .await;
        // The registration and its health come first, because a load balancer forwards only to
        // a target its own checks found healthy.
        let registered = healthy_target(&router, row.case, &group, row.service_name).await;
        assert!(
            registered.starts_with("10.90."),
            "case {:?}: the registration must name the task's own address in the VPC, got \
             {registered:?}",
            row.case
        );
        let answered = get(&router, &dns_name, row.path).await;
        assert_eq!(
            answered.status, row.status,
            "case {:?}: expected HTTP {}, got {} with body {:?}",
            row.case, row.status, answered.status, answered.body
        );
        let fields: Vec<&str> = answered.body.split_whitespace().collect();
        if row.status == OK {
            // The marker the container's own program wrote, and — where the program wrote it —
            // the hostname the runtime gave that container, which is the private DNS hostname
            // of the very address the registration names.
            let wanted = if row.hostname_in_body {
                vec![row.body.to_string(), container_hostname(&registered)]
            } else {
                vec![row.body.to_string()]
            };
            assert_eq!(
                fields, wanted,
                "case {:?}: expected the body {wanted:?}, got {:?}",
                row.case, answered.body
            );
        } else {
            // A status the LOAD BALANCER produced, or one the target answered with no document:
            // no body is invented for either.
            assert!(
                fields.is_empty(),
                "case {:?}: expected no body, got {:?}",
                row.case,
                answered.body
            );
        }
    }
}

/// The two refusals the negative contract measures, and the markers the revisions of the
/// control serve.
const REFUSAL_FAMILY: &str = "refusedrevision";
const BRIDGE_FAMILY: &str = "refusedbridge";
const REPAIRED_FAMILY: &str = "repairedrevision";
const FIRST_MARKER: &str = "REVISION-A";
const CONTROL_MARKER: &str = "REVISION-B";

/// The wording Amazon ECS refuses a Fargate-compatible registration in another network mode
/// with, and the one it refuses a placement onto a revision that does not support the launch
/// type with. Both are entirely fixed text, with no value of the request interpolated.
///
/// `pub(crate)` for the reason [`emulator`] gives: the sibling suite that measures this refusal
/// over the WHOLE `NetworkMode` enum asserts the same two sentences, and one copy of a refusal's
/// wording cannot drift from itself.
pub(crate) const FARGATE_NETWORK_MODE_MESSAGE: &str =
    "Fargate only supports network mode \u{2018}awsvpc\u{2019}.";
pub(crate) const LAUNCH_TYPE_MESSAGE: &str = "Task definition does not support launch_type FARGATE.";

/// The container command that writes `marker` into the nginx image's document root and then
/// runs that image's own command.
pub(crate) fn serving_command(marker: &str) -> Vec<String> {
    vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        format!(
            "echo {marker} > /usr/share/nginx/html/index.html && nginx -g 'daemon off;'"
        ),
    ]
}

/// The `rolloutState` of the service's PRIMARY deployment, and the revision that deployment
/// runs.
fn primary_rollout(described: &Value) -> Option<(String, String)> {
    described["services"][0]["deployments"]
        .as_array()?
        .iter()
        .find(|deployment| deployment["status"].as_str() == Some("PRIMARY"))
        .and_then(|deployment| {
            Some((
                deployment["rolloutState"].as_str()?.to_string(),
                deployment["taskDefinition"].as_str()?.to_string(),
            ))
        })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_update_leaves_the_live_fargate_service_serving_its_own_revision() {
    let router = emulator();
    let network = seed_network(&router).await;
    ecs_ok(&router, "CreateCluster", json!({ "clusterName": CLUSTER })).await;
    let case = "a live ALB-attached awsvpc Fargate service";
    let first = serving_command(FIRST_MARKER);
    let borrowed: Vec<&str> = first.iter().map(String::as_str).collect();
    let revision = register_revision(&router, REFUSAL_FAMILY, 80, &borrowed).await;
    let (group, dns_name) =
        front_door(&router, &network, case, "refused-tg", "refused-lb", 80).await;
    create_service(&router, &network, "refused", &revision, &group, 80).await;
    let registered = healthy_target(&router, case, &group, "refused").await;
    let serving = get(&router, &dns_name, "/").await;
    assert_eq!(
        (serving.status, serving.body.trim()),
        (OK, FIRST_MARKER),
        "case {case:?}: expected the first revision's own body, got {} with {:?}",
        serving.status,
        serving.body
    );

    // A Fargate-compatible revision may not be registered in another network mode.
    let (status, refused) = ecs(
        &router,
        "RegisterTaskDefinition",
        json!({
            "family": BRIDGE_FAMILY,
            "networkMode": "bridge",
            "requiresCompatibilities": ["FARGATE"],
            "cpu": "256",
            "memory": "512",
            "containerDefinitions": [{
                "name": CONTAINER,
                "image": "public.ecr.aws/docker/library/nginx:alpine",
                "memory": 128,
                "portMappings": [{ "containerPort": 80 }],
            }],
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400 for a FARGATE revision in the bridge network mode, got \
         {status} with {refused}"
    );
    assert!(
        refused["__type"]
            .as_str()
            .is_some_and(|shape| shape.ends_with("ClientException")),
        "case {case:?}: expected ClientException, got {refused}"
    );
    assert_eq!(
        refused["message"].as_str(),
        Some(FARGATE_NETWORK_MODE_MESSAGE),
        "case {case:?}: expected the fixed refusal wording, got {refused}"
    );

    // And a live Fargate service may not be moved onto a revision that does not support that
    // launch type — an EC2/bridge revision, which registers because it asks for no Fargate
    // compatibility of its own.
    let ec2_revision = ecs_ok(
        &router,
        "RegisterTaskDefinition",
        json!({
            "family": BRIDGE_FAMILY,
            "networkMode": "bridge",
            "containerDefinitions": [{
                "name": CONTAINER,
                "image": "public.ecr.aws/docker/library/nginx:alpine",
                "memory": 128,
                "portMappings": [{ "containerPort": 80 }],
            }],
        }),
    )
    .await;
    let ec2_revision = ec2_revision["taskDefinition"]["taskDefinitionArn"]
        .as_str()
        .expect("the EC2 revision must answer an ARN")
        .to_string();
    let (status, refused) = ecs(
        &router,
        "UpdateService",
        json!({
            "cluster": CLUSTER,
            "service": "refused",
            "taskDefinition": ec2_revision,
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400 for an update onto an EC2/bridge revision, got {status} \
         with {refused}"
    );
    assert!(
        refused["__type"]
            .as_str()
            .is_some_and(|shape| shape.ends_with("InvalidParameterException")),
        "case {case:?}: expected InvalidParameterException, got {refused}"
    );
    assert_eq!(
        refused["message"].as_str(),
        Some(LAUNCH_TYPE_MESSAGE),
        "case {case:?}: expected the fixed refusal wording, got {refused}"
    );

    // The refusal changed nothing: the service still runs the revision it was created with,
    // the same task is still the one registered, and the load balancer still serves that
    // revision's own bytes.
    let described = ecs_ok(
        &router,
        "DescribeServices",
        json!({ "cluster": CLUSTER, "services": ["refused"] }),
    )
    .await;
    assert_eq!(
        described["services"][0]["taskDefinition"].as_str(),
        Some(revision.as_str()),
        "case {case:?}: a refused update must leave the service's revision alone, got {described}"
    );
    let still = health(&router, &group).await;
    assert_eq!(
        still,
        vec![(registered.clone(), HEALTHY.to_string())],
        "case {case:?}: a refused update must leave the registration alone, got {still:?}"
    );
    let serving = get(&router, &dns_name, "/").await;
    assert_eq!(
        (serving.status, serving.body.trim()),
        (OK, FIRST_MARKER),
        "case {case:?}: a refused update must leave the served bytes alone, got {} with {:?}",
        serving.status,
        serving.body
    );

    // Only a repaired awsvpc revision moves the service: its primary deployment rolls to
    // COMPLETED on that revision, and the load balancer then serves ITS marker.
    let control = serving_command(CONTROL_MARKER);
    let borrowed: Vec<&str> = control.iter().map(String::as_str).collect();
    let repaired = register_revision(&router, REPAIRED_FAMILY, 80, &borrowed).await;
    ecs_ok(
        &router,
        "UpdateService",
        json!({
            "cluster": CLUSTER,
            "service": "refused",
            "taskDefinition": repaired,
        }),
    )
    .await;
    let deadline = Instant::now() + DEADLINE;
    loop {
        let described = ecs_ok(
            &router,
            "DescribeServices",
            json!({ "cluster": CLUSTER, "services": ["refused"] }),
        )
        .await;
        let rollout = primary_rollout(&described);
        let served = get(&router, &dns_name, "/").await;
        if rollout == Some(("COMPLETED".to_string(), repaired.clone()))
            && served.status == OK
            && served.body.trim() == CONTROL_MARKER
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the primary deployment COMPLETED on {repaired} and the \
             control marker served, got {rollout:?} and {} with {:?}",
            served.status,
            served.body
        );
        ::tokio::time::sleep(POLL).await;
    }
}
