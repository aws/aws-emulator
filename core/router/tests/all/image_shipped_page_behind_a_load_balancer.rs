// SPDX-License-Identifier: CC0-1.0

//! The page a web server IMAGE ships, and the command it declares, as the load balancer in
//! front of the Amazon ECS task running it sees them.
//!
//! RULE: **a container runs the program and over the filesystem its IMAGE provides where its
//! task definition overrides neither — so a container of a web server image runs that image's
//! own command, and a web server image provides the directory its shipped configuration serves
//! WITH a page inside it: such a container answers that directory's root with the image's page
//! and a path the image ships nothing at with the server's own `404`. A target group
//! health-checking the root therefore finds such a target serving, and one pointed at a path the
//! image ships nothing at drives the same registration to `unhealthy` with the reason the model
//! gives a rejected status. A program that writes its own document at the root REPLACES the
//! shipped one.**
//!
//! The grounds. `ContainerDefinition$command` is "the command that's passed to the container"
//! and `ContainerOverride$command` "the command to send to the container that overrides the
//! default command from the Docker image or the task definition" — so an image HAS a command of
//! its own, and a web server image's documented command is an invocation of its own server over
//! its own shipped configuration; `HealthCheckPath` is "[HTTP/HTTPS health checks] The
//! destination for health checks on the targets" and `Matcher` "the HTTP or gRPC codes to use
//! when checking for a successful response from a target"
//! (`smithy_query(query="doc",shape="CreateTargetGroupInput",member="Matcher")`), so the state
//! of a registration is what the ANSWER to that destination earns it:
//! `TargetHealth$Reason` gives "`Target.ResponseCodeMismatch` - The health checks did not return
//! an expected HTTP code", while "if the target state is healthy, a reason code is not
//! provided".
//!
//! Three crates have to agree for that to hold, which is why this suite lives here: Amazon ECS
//! runs the container and records the server it listens with at its task's own address, Amazon
//! EC2 owns the network and the security-group rule that lets a check arrive, and Elastic Load
//! Balancing v2 checks the registration, reports its health and answers the client. Everything
//! is driven through the real operations on ONE shared engine.
//!
//! The rows differ along what the rule generalises over: a revision that declares no command at
//! all (so the command the image declares is what runs, for each of the two images), which
//! command the revision writes out instead, which directory the image serves (the Apache HTTP
//! Server image's own root, and the one the nginx image shares with its package), the page each
//! of them ships, a program that READS that page and writes it somewhere else of its own, and a
//! container whose own program writes a page at that root — which is then the one served, with
//! no trace of the shipped one.
//!
//! Every clock-owed move — the task's start, the registration it owes, the verdict the group's
//! cadence reaches — is POLLED for until it holds, never slept for, and each deadline is
//! generous: a row asserts that the move HAPPENS, not how fast this host runs.

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

/// The status every control-plane call of the setup must answer.
const OK: u16 = 200;

/// The cluster every row runs in, and the container name every binding names.
const CLUSTER: &str = "shipped";
const CONTAINER: &str = "app";

/// The network every row runs in.
const VPC_CIDR: &str = "10.80.0.0/16";
const SUBNET_CIDRS: [(&str, &str); 2] = [
    ("10.80.1.0/24", "us-east-1a"),
    ("10.80.2.0/24", "us-east-1b"),
];

/// The port every container's server listens on, which is the `containerPort` its revision
/// declares, the port its binding names and its target group's own.
const CONTAINER_PORT: i64 = 80;

/// The `HealthCheckIntervalSeconds` and the `UnhealthyThresholdCount` every target group is
/// created with: a verdict takes a threshold of consecutive checks, each one interval after the
/// last, so each row asks for the shortest interval the member admits ("the range is 5-300")
/// and the fewest checks a threshold may ask for ("the range is 2-10").
const INTERVAL_SECONDS: &str = "5";
const THRESHOLD: &str = "2";

/// The health-check destination each row starts at — the root the image's own configuration
/// serves — and the one it is moved to, which the images ship nothing at.
const SERVED_PATH: &str = "/";
const MISSING_PATH: &str = "/this-path-does-not-exist";

/// The `TargetHealth$State` values this suite reads back, and the reason the model gives a
/// status the group's matcher does not accept.
const HEALTHY: &str = "healthy";
const UNHEALTHY: &str = "unhealthy";
const RESPONSE_CODE_MISMATCH: &str = "Target.ResponseCodeMismatch";

/// The status a static server answers the document it was asked for with, and the one it
/// answers a path its document root holds nothing at.
const SERVED_STATUS: u16 = 200;
const NOT_FOUND_STATUS: u16 = 404;

/// The marker the one row whose own program writes a page puts into it.
const OWN_PAGE: &str = "OWN-PAGE-NOT-THE-SHIPPED-ONE";

/// How long a clock-owed move is waited for before a row gives up, and how often it looks
/// again. Generous on purpose: the deadline is here so a failure is a failure rather than a
/// hang.
const DEADLINE: Duration = Duration::from_secs(120);
const POLL: Duration = Duration::from_millis(100);

/// One row: the container's program, what the client must then be answered with at the root,
/// and a marker that must NOT appear there.
struct Row {
    case: &'static str,
    /// The names the row's own resources take.
    family: &'static str,
    group_name: &'static str,
    load_balancer_name: &'static str,
    service_name: &'static str,
    /// The container's `image`, which is what provides its filesystem and its own command.
    image: &'static str,
    /// The container's `command`, which is what starts the server — EMPTY for a row whose
    /// revision declares none, so what runs is the command the image itself declares.
    command: &'static [&'static str],
    /// A fragment the body answered for the root must carry.
    carries: &'static str,
    /// A fragment that body must NOT carry, for the row whose own program replaced the page
    /// the image ships — empty where the row asserts nothing of the kind.
    carries_not: &'static str,
    /// A second path the same answer must be served at, for the row whose program READ the
    /// page its image ships and wrote it somewhere else — empty where the row has none.
    copied_path: &'static str,
}

/// The two official images the rows below run, as the public gallery's mirror of the Docker
/// official images publishes each: the one whose server serves the same directory its package
/// does, and the one whose own document root differs from its package's.
const NGINX_IMAGE: &str = "public.ecr.aws/docker/library/nginx:alpine";
const HTTPD_IMAGE: &str = "public.ecr.aws/docker/library/httpd:2.4";

const ROWS: &[Row] = &[
    Row {
        case: "a revision that declares no command at all runs the one its image declares",
        family: "shippeddefault",
        group_name: "shipped-default",
        load_balancer_name: "shipped-default-lb",
        service_name: "shippeddefault",
        image: NGINX_IMAGE,
        command: &[],
        // The welcome page nginx's own distribution ships at its document root, served by the
        // command the nginx image declares.
        carries: "Welcome to nginx!",
        carries_not: "",
        copied_path: "",
    },
    Row {
        case: "the same for the image whose own document root differs from its package's",
        family: "shippeddefaulthttpd",
        group_name: "shipped-default-httpd",
        load_balancer_name: "shipped-default-httpd-lb",
        service_name: "shippeddefaulthttpd",
        image: HTTPD_IMAGE,
        command: &[],
        carries: "It works!",
        carries_not: "",
        copied_path: "",
    },
    Row {
        case: "the Apache HTTP Server image's own command, over the page that image ships",
        family: "shippedhttpd",
        group_name: "shipped-httpd",
        load_balancer_name: "shipped-httpd-lb",
        service_name: "shippedhttpd",
        image: HTTPD_IMAGE,
        command: &["httpd-foreground"],
        // The one-line document the Apache HTTP Server's own distribution ships at its
        // document root.
        carries: "It works!",
        carries_not: "",
        copied_path: "",
    },
    Row {
        case: "the nginx image's own command, over the page that image ships",
        family: "shippednginx",
        group_name: "shipped-nginx",
        load_balancer_name: "shipped-nginx-lb",
        service_name: "shippednginx",
        image: NGINX_IMAGE,
        command: &["nginx", "-g", "daemon off;"],
        // The welcome page nginx's own distribution ships at its document root.
        carries: "Welcome to nginx!",
        carries_not: "",
        copied_path: "",
    },
    Row {
        case: "a program that reads the page its image ships serves it where it put it too",
        family: "shippedcopied",
        group_name: "shipped-copied",
        load_balancer_name: "shipped-copied-lb",
        service_name: "shippedcopied",
        image: NGINX_IMAGE,
        command: &[
            "/bin/sh",
            "-c",
            "mkdir -p /usr/share/nginx/html/copy && \
             cat /usr/share/nginx/html/index.html > /usr/share/nginx/html/copy/index.html && \
             nginx -g 'daemon off;'",
        ],
        carries: "Welcome to nginx!",
        carries_not: "",
        copied_path: "/copy/",
    },
    Row {
        case: "a program that writes its own page at that root replaces the shipped one",
        family: "shippedown",
        group_name: "shipped-own",
        load_balancer_name: "shipped-own-lb",
        service_name: "shippedown",
        image: NGINX_IMAGE,
        command: &[
            "/bin/sh",
            "-c",
            "echo OWN-PAGE-NOT-THE-SHIPPED-ONE > /usr/share/nginx/html/index.html && \
             nginx -g 'daemon off;'",
        ],
        carries: OWN_PAGE,
        carries_not: "Welcome to nginx!",
        copied_path: "",
    },
];

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

/// One awsJson1_1 call to ECS, answered with its status and parsed body.
async fn ecs(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
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
async fn ecs_ok(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let (status, parsed) = ecs(router, operation, body).await;
    assert_eq!(
        status, OK,
        "setup: ECS {operation} answered {status} with {parsed}"
    );
    parsed
}

/// The network every row runs in: a VPC with two subnets and an internet gateway, the load
/// balancers' own security group, and the group the tasks are placed in — which admits the load
/// balancers' checks and traffic on the port the containers serve on.
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
    // The path off the VPC a task needs to fetch the image its container runs: Amazon ECS reads
    // the route table that applies to the placement's subnet before it starts a task, and a
    // placement with no route off the VPC cannot reach a public registry.
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
    let load_balancer_group = group("shipped-alb", "the load balancers own group").await;
    let task_group = group("shipped-tasks", "the group the tasks are placed in").await;
    // The ONE rule this suite authorizes: the tasks' group takes the load balancers' own group
    // as a source on the port the containers serve on, which is what lets a check of a target
    // in it arrive.
    let port = CONTAINER_PORT.to_string();
    ec2(
        router,
        "AuthorizeSecurityGroupIngress",
        &[
            ("GroupId", task_group.as_str()),
            ("IpPermissions.1.IpProtocol", "tcp"),
            ("IpPermissions.1.FromPort", port.as_str()),
            ("IpPermissions.1.ToPort", port.as_str()),
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

/// Register one `awsvpc`, Fargate-compatible revision of `family` from `image` whose single
/// container runs `command` — and, where that is empty, declares none at all, so what runs is
/// the command its image declares.
async fn register_revision(
    router: &MultiServiceRouter,
    family: &str,
    image: &str,
    command: &[&str],
) -> String {
    let mut container = json!({
        "name": CONTAINER,
        "image": image,
        "memory": 128,
        "portMappings": [{ "containerPort": CONTAINER_PORT }],
    });
    if !command.is_empty() {
        container["command"] = json!(command);
    }
    let registered = ecs_ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
            "networkMode": "awsvpc",
            "requiresCompatibilities": ["FARGATE"],
            "cpu": "256",
            "memory": "512",
            "containerDefinitions": [container],
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
async fn front_door(router: &MultiServiceRouter, network: &Network, row: &Row) -> (String, String) {
    let created = elbv2(
        router,
        "CreateTargetGroup",
        &[
            ("Name", row.group_name),
            ("Protocol", "HTTP"),
            ("Port", &CONTAINER_PORT.to_string()),
            ("TargetType", "ip"),
            ("VpcId", network.vpc.as_str()),
            ("HealthCheckProtocol", "HTTP"),
            ("HealthCheckPath", SERVED_PATH),
            ("HealthCheckIntervalSeconds", INTERVAL_SECONDS),
            ("HealthyThresholdCount", THRESHOLD),
            ("UnhealthyThresholdCount", THRESHOLD),
        ],
    )
    .await;
    let group = opt_element(&created, "TargetGroupArn")
        .unwrap_or_else(|| panic!("case {:?}: no TargetGroupArn in {created}", row.case));
    let balanced = elbv2(
        router,
        "CreateLoadBalancer",
        &[
            ("Name", row.load_balancer_name),
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

/// Create one ECS service of `revision`, bound to `group`, with one task.
async fn create_service(
    router: &MultiServiceRouter,
    network: &Network,
    service_name: &str,
    revision: &str,
    group: &str,
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
                "containerPort": CONTAINER_PORT,
            }],
            "healthCheckGracePeriodSeconds": 120,
        }),
    )
    .await;
}

/// The `(Id, State, Reason)` of every registration `group` reports — the reason empty where
/// none is reported, which is what a healthy registration carries.
async fn health(router: &MultiServiceRouter, group: &str) -> Vec<(String, String, String)> {
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
            (
                text(description, "Id"),
                text(health, "State"),
                text(health, "Reason"),
            )
        })
        .collect()
}

/// Wait until `group` reports exactly one registration in `state` whose reason is `reason` (the
/// empty string for a state that carries none), and answer the address it is registered at.
///
/// Polls the CONDITION — the task's start, the registration that start owes and the verdict its
/// group's cadence reaches are all clock-owed — rather than waiting a fixed time.
async fn settled(
    router: &MultiServiceRouter,
    row: &Row,
    group: &str,
    state: &str,
    reason: &str,
) -> String {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let reported = health(router, group).await;
        if reported.len() == 1 && reported[0].1 == state && reported[0].2 == reason {
            return reported[0].0.clone();
        }
        if Instant::now() >= deadline {
            // What the service itself says about the tasks it placed, so a row that never
            // converges names the reason rather than only the reads that did not settle.
            let described = ecs_ok(
                router,
                "DescribeServices",
                json!({ "cluster": CLUSTER, "services": [row.service_name] }),
            )
            .await;
            panic!(
                "case {:?}: expected one registration reported {state:?} with reason \
                 {reason:?}, got {reported:?}; the service reports {described}",
                row.case
            );
        }
        ::tokio::time::sleep(POLL).await;
    }
}

/// One answered data-plane request: the status and the body, as the wire carries them.
struct Answered {
    status: u16,
    body: String,
}

/// Send one request to the address the load balancer published, carrying no AWS credential —
/// exactly as a client handed that name sends it.
async fn get(router: &MultiServiceRouter, dns_name: &str, path: &str) -> Answered {
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_page_the_image_ships_is_served_and_health_checked() {
    let router = emulator();
    let network = seed_network(&router).await;
    ecs_ok(&router, "CreateCluster", json!({ "clusterName": CLUSTER })).await;
    for row in ROWS {
        let revision = register_revision(&router, row.family, row.image, row.command).await;
        let (group, dns_name) = front_door(&router, &network, row).await;
        create_service(&router, &network, row.service_name, &revision, &group).await;

        // The group's checks are answered by the page the image ships at the destination they
        // ask for, so the registration reaches `healthy` — and a healthy one carries no reason.
        let address = settled(&router, row, &group, HEALTHY, "").await;
        assert!(
            address.starts_with("10.80."),
            "case {:?}: the registration must name the task's own address in the VPC, got \
             {address:?}",
            row.case
        );

        // The client is answered that same page, and the server's own 404 for a path nothing
        // of the container — neither its image nor its program — holds a document at.
        let served = get(&router, &dns_name, SERVED_PATH).await;
        assert_eq!(
            served.status, SERVED_STATUS,
            "case {:?}: expected HTTP {SERVED_STATUS} for {SERVED_PATH}, got {} with body {:?}",
            row.case, served.status, served.body
        );
        assert!(
            served.body.contains(row.carries),
            "case {:?}: expected the body for {SERVED_PATH} to carry {:?}, got {:?}",
            row.case,
            row.carries,
            served.body
        );
        if !row.carries_not.is_empty() {
            assert!(
                !served.body.contains(row.carries_not),
                "case {:?}: the program's own page must replace the shipped one, but the body \
                 for {SERVED_PATH} carries {:?}: {:?}",
                row.case,
                row.carries_not,
                served.body
            );
        }
        // A path the program READ that page into carries it too: what the image ships is in the
        // container's own filesystem, not only in what its server answers.
        if !row.copied_path.is_empty() {
            let copied = get(&router, &dns_name, row.copied_path).await;
            assert_eq!(
                copied.status, SERVED_STATUS,
                "case {:?}: expected HTTP {SERVED_STATUS} for {}, got {} with body {:?}",
                row.case, row.copied_path, copied.status, copied.body
            );
            assert!(
                copied.body.contains(row.carries),
                "case {:?}: expected the body for {} to carry {:?}, got {:?}",
                row.case,
                row.copied_path,
                row.carries,
                copied.body
            );
        }
        let missing = get(&router, &dns_name, MISSING_PATH).await;
        assert_eq!(
            missing.status, NOT_FOUND_STATUS,
            "case {:?}: expected HTTP {NOT_FOUND_STATUS} for {MISSING_PATH}, got {} with body \
             {:?}",
            row.case, missing.status, missing.body
        );

        // Pointing the group's health check at that path is what the checks then ask for, so
        // the SAME registration reaches `unhealthy` with the reason the model gives a status
        // its matcher does not accept.
        elbv2(
            &router,
            "ModifyTargetGroup",
            &[
                ("TargetGroupArn", group.as_str()),
                ("HealthCheckPath", MISSING_PATH),
            ],
        )
        .await;
        let still = settled(&router, row, &group, UNHEALTHY, RESPONSE_CODE_MISMATCH).await;
        assert_eq!(
            still, address,
            "case {:?}: the verdict must be about the same registration, got {still:?} after \
             {address:?}",
            row.case
        );
    }
}
