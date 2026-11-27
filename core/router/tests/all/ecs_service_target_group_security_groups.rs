// SPDX-License-Identifier: CC0-1.0

//! An ECS service's task is reported HEALTHY by Elastic Load Balancing only while the
//! security groups the placement gave it admit the load balancer's health check.
//!
//! RULE: a health check is a connection the load balancer opens to the target on the target
//! group's effective health-check port, so it only arrives if an INBOUND rule of a security
//! group governing that target names both that port and that load balancer as a source — "a
//! security group controls the traffic that is allowed to reach and leave the resources that
//! it is associated with" (Amazon VPC User Guide, "Security groups"). A target whose groups
//! carry no such rule answers no check: it is never reported `healthy` and settles on
//! `unhealthy` with the reason its check failed for and a description. A target whose groups
//! do carry one converges to `healthy` with no reason code, and a target whose groups the
//! account holds nothing for is failed on nothing: no rule was read either way.
//!
//! Three crates have to agree for that to hold, which is why this suite lives here: ECS
//! declares the security groups of the task it registers
//! (`AwsVpcConfiguration$securityGroups`), EC2 owns those groups and their rules, and Elastic
//! Load Balancing v2 reads the rules against the load balancer performing the check.
//! Everything is driven through the real operations on one engine, so what is asserted is
//! what each service persisted.
//!
//! The rows differ along what the rule generalises over: the kind of source a rule names (the
//! load balancer's own group, an address range covering it, the whole address family, another
//! group), whether the rule's port range covers the checked port, the protocol it names, a
//! group with no inbound rule at all, and a group the account does not hold. The recorded
//! configuration — a task group that admits the checked port from somewhere other than the
//! load balancer — is a single row.

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
const CLUSTER: &str = "sgcheck";
const FAMILY: &str = "sgcheckfamily";
const CONTAINER: &str = "app";

/// The port the container listens on, which is also every row's traffic and health-check
/// port: this suite's rule is about the security groups, so the port is held fixed.
const CONTAINER_PORT: i64 = 80;

/// A port the rule of one row names INSTEAD of the checked one.
const OTHER_PORT: &str = "8080";

/// The network every row runs in.
const VPC_CIDR: &str = "10.90.0.0/16";
const SUBNET_CIDRS: [(&str, &str); 2] = [("10.90.1.0/24", "us-east-1a"), ("10.90.2.0/24", "us-east-1b")];

/// How long a row waits for the clock-owed moves it depends on — a task's start, and the
/// health checks the target group's own cadence runs ([`HEALTH_CHECK_INTERVAL_SECONDS`]) —
/// and how often it looks. Generous on purpose: the deadline is only here so a failure is a
/// failure rather than a hang.
const DEADLINE: Duration = Duration::from_secs(90);
const POLL: Duration = Duration::from_millis(100);

/// The `HealthCheckIntervalSeconds` every target group below is created with: a verdict takes
/// `UnhealthyThresholdCount` consecutive failing checks, each one interval after the last, so
/// a suite that waits for one asks for the shortest interval the member admits ("the range is
/// 5-300").
const HEALTH_CHECK_INTERVAL_SECONDS: &str = "5";

/// The source an inbound rule of a row's task group names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    /// The security group the load balancer itself is associated with.
    LoadBalancerGroup,
    /// Another security group of the same VPC, which the load balancer is not in.
    OtherGroup,
    /// An address range, as a CIDR block.
    Range(&'static str),
}

/// One row: the inbound rule the task's own security group carries, and the verdict it
/// implies for the task registered into the row's target group.
struct Row {
    case: &'static str,
    /// The target group's name, and the ECS service that binds it.
    group_name: &'static str,
    service_name: &'static str,
    /// The listener port that makes the group one the load balancer forwards to.
    listener_port: &'static str,
    /// `(IpProtocol, FromPort, ToPort, source)` of the one inbound rule the task's group
    /// carries, or `None` for a group with no inbound rule at all.
    rule: Option<(&'static str, Option<&'static str>, Option<&'static str>, Source)>,
    /// The `TargetHealth$State` the registration must reach.
    state: &'static str,
    /// The `TargetHealth$Reason` values AWS gives for that state; empty for a state reported
    /// without a reason code ("if the target state is `healthy`, a reason code is not
    /// provided", `TargetHealth$Reason`).
    reasons: &'static [&'static str],
}

const ROWS: [Row; 7] = [
    Row {
        case: "a rule admitting the load balancer's own group on the checked port",
        group_name: "from-alb",
        service_name: "fromalb",
        listener_port: "80",
        rule: Some(("tcp", Some("80"), Some("80"), Source::LoadBalancerGroup)),
        state: "healthy",
        reasons: &[],
    },
    Row {
        case: "the recorded configuration: the checked port admitted from another group",
        group_name: "from-other",
        service_name: "fromother",
        listener_port: "8081",
        rule: Some(("tcp", Some("80"), Some("80"), Source::OtherGroup)),
        state: "unhealthy",
        reasons: &["Target.Timeout", "Target.FailedHealthChecks"],
    },
    Row {
        case: "a rule admitting the load balancer's group on another port",
        group_name: "other-port",
        service_name: "otherport",
        listener_port: "8082",
        rule: Some((
            "tcp",
            Some(OTHER_PORT),
            Some(OTHER_PORT),
            Source::LoadBalancerGroup,
        )),
        state: "unhealthy",
        reasons: &["Target.Timeout", "Target.FailedHealthChecks"],
    },
    Row {
        case: "a group with no inbound rule at all",
        group_name: "no-rule",
        service_name: "norule",
        listener_port: "8083",
        rule: None,
        state: "unhealthy",
        reasons: &["Target.Timeout", "Target.FailedHealthChecks"],
    },
    Row {
        case: "a rule admitting the VPC's own address range, which the load balancer is in",
        group_name: "vpc-range",
        service_name: "vpcrange",
        listener_port: "8084",
        rule: Some((
            "tcp",
            Some("80"),
            Some("80"),
            Source::Range(VPC_CIDR),
        )),
        state: "healthy",
        reasons: &[],
    },
    Row {
        case: "a rule admitting the whole address family",
        group_name: "anywhere",
        service_name: "anywhere",
        listener_port: "8085",
        rule: Some((
            "tcp",
            Some("80"),
            Some("80"),
            Source::Range("0.0.0.0/0"),
        )),
        state: "healthy",
        reasons: &[],
    },
    Row {
        case: "a rule admitting every protocol, so every port, from the load balancer",
        group_name: "all-protocols",
        service_name: "allprotocols",
        listener_port: "8086",
        rule: Some(("-1", None, None, Source::LoadBalancerGroup)),
        state: "healthy",
        reasons: &[],
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

/// The network every row runs in: a VPC, two subnets, the load balancer's own security group,
/// and one more group the load balancer is NOT in.
struct Network {
    vpc: String,
    subnets: Vec<String>,
    load_balancer_group: String,
    other_group: String,
}

/// Create that network through EC2 itself, so every id asserted against is one EC2 minted.
///
/// The VPC is given the internet gateway AWS requires of the internet-facing load balancer
/// below — the scheme `CreateLoadBalancer` documents as its default — attached through EC2's
/// own `AttachInternetGateway`.
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
    let load_balancer_group = group("sgcheck-alb", "the load balancers own group").await;
    let other_group = group("sgcheck-other", "a group the load balancer is not in").await;
    Network {
        vpc,
        subnets,
        load_balancer_group,
        other_group,
    }
}

/// Create the security group of one row's task, carrying exactly the inbound rule that row
/// describes, and answer its id.
async fn task_security_group(
    router: &MultiServiceRouter,
    network: &Network,
    row: &Row,
) -> String {
    let created = ec2(
        router,
        "CreateSecurityGroup",
        &[
            ("GroupName", row.group_name),
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
        match source {
            Source::LoadBalancerGroup => params.push((
                "IpPermissions.1.Groups.1.GroupId",
                network.load_balancer_group.as_str(),
            )),
            Source::OtherGroup => params.push((
                "IpPermissions.1.Groups.1.GroupId",
                network.other_group.as_str(),
            )),
            Source::Range(cidr) => params.push(("IpPermissions.1.IpRanges.1.CidrIp", cidr)),
        }
        ec2(router, "AuthorizeSecurityGroupIngress", &params).await;
    }
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

/// Create the `ip` target group one row describes, health-checked on the port the container
/// listens on, and answer its ARN.
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
            (
                "HealthCheckIntervalSeconds",
                HEALTH_CHECK_INTERVAL_SECONDS,
            ),
        ],
    )
    .await;
    opt_element(&created, "TargetGroupArn")
        .unwrap_or_else(|| panic!("case {:?}: no TargetGroupArn in {created}", row.case))
}

/// Create the ECS service one row describes: one task in the first subnet, in the security
/// group that row created (or the one the account holds nothing for), bound to `group`.
async fn create_service(
    router: &MultiServiceRouter,
    network: &Network,
    row: &Row,
    group: &str,
    task_group: &str,
) {
    let declared = task_group;
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
                    "subnets": [network.subnets[0]],
                    "securityGroups": [declared],
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
        created["service"]["networkConfiguration"]["awsvpcConfiguration"]["securityGroups"][0]
            .as_str(),
        Some(declared),
        "case {:?}: the stored service must carry the group the placement named, got {created}",
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

#[tokio::test]
async fn an_ecs_task_is_healthy_only_while_its_security_groups_admit_the_health_check() {
    let (router, engine) = emulator();
    let network = seed_network(&router).await;
    // The tasks below have to RUN, and ECS reads the path to the image registry off the
    // account's network records before it starts one.
    crate::ec2_network::register_internet_route(&engine, &network.vpc);
    seed_ecs(&router).await;

    let load_balancer = element(
        &elbv2(
            &router,
            "CreateLoadBalancer",
            &[
                ("Name", "sgcheck-alb"),
                ("Subnets.member.1", network.subnets[0].as_str()),
                ("Subnets.member.2", network.subnets[1].as_str()),
                (
                    "SecurityGroups.member.1",
                    network.load_balancer_group.as_str(),
                ),
            ],
        )
        .await,
        "LoadBalancerArn",
    );

    let mut groups: Vec<String> = Vec::new();
    for row in &ROWS {
        let task_group = task_security_group(&router, &network, row).await;
        let group = target_group(&router, &network, row).await;
        // A listener forwarding to the group is what makes it one the load balancer checks —
        // and what lets an ECS service bind it at all.
        elbv2(
            &router,
            "CreateListener",
            &[
                ("LoadBalancerArn", load_balancer.as_str()),
                ("Protocol", "HTTP"),
                ("Port", row.listener_port),
                ("DefaultActions.member.1.Type", "forward"),
                ("DefaultActions.member.1.TargetGroupArn", group.as_str()),
            ],
        )
        .await;
        create_service(&router, &network, row, &group, &task_group).await;
        groups.push(group);
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
