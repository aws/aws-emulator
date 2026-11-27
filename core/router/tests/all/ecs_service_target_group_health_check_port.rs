// SPDX-License-Identifier: CC0-1.0

//! An ECS service's task is reported HEALTHY by Elastic Load Balancing only while the
//! container behind it listens on the port the target group's health check connects to.
//!
//! RULE: a load balancer health-checks a registration on the target group's effective
//! health-check port — `HealthCheckPort` when it names one, and the port the target receives
//! traffic on when it names `traffic-port` or nothing — so a target whose workload does not
//! listen on that port answers no check: it is never reported `healthy` and settles on
//! `unhealthy` with the reason its check failed for and a description, while a target whose
//! workload does listen on it converges to `healthy` with no reason code. What a task
//! listens on is what its revision declares: the `containerPort`s of its containers'
//! `portMappings`, which for an `awsvpc` task are the very ports its own interface's address
//! is reached on.
//!
//! Two crates have to agree for that to hold, which is why this suite lives here: ECS
//! declares the ports of the task it registers, and Elastic Load Balancing v2 checks the
//! registration against the group's health-check port — over the account's EC2 records, which
//! hold the network the task's interface is placed in. Everything is driven through the real
//! operations on one engine, so what is asserted is what ECS persisted and what ELBv2 reports
//! for it.
//!
//! The rows differ along what the rule generalises over: a health check left at the traffic
//! port, one pointed at a port the container does not listen on, and one pointed at a second
//! port the container DOES listen on. The configuration a recording happens to show — a
//! container port of 80 beside a health-check port of 8888 — is a single row.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The ONE percent-encoder and element readers this test crate keeps for awsQuery suites
// (`all/query_wire.rs`: "Nothing new should write another one — call this module").
use crate::query_wire::{elements, form_encode, opt_element, target_health_of};

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// The API version an awsQuery request naming the ELBv2 model carries.
const ELB_VERSION: &str = "2015-12-01";

/// The ECS cluster, task-definition family and container every row runs.
const CLUSTER: &str = "hcport";
const FAMILY: &str = "hcportfamily";
const CONTAINER: &str = "app";

/// The ports the container declares in its `portMappings`: the one its service's bindings
/// attach, and a second one it also listens on.
const CONTAINER_PORT: i64 = 80;
const SECOND_CONTAINER_PORT: i64 = 8081;

/// A port the container declares NO mapping for, so nothing in the task listens on it.
const UNSERVED_PORT: &str = "8888";

/// The subnets the tasks and the load balancer are placed in, and the VPC holding them.
const SUBNET: &str = "subnet-0123456789abcdef0";
const SECOND_SUBNET: &str = "subnet-0123456789abcdef1";
const VPC: &str = "vpc-0123456789abcdef0";

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

/// One row: how the target group's health check is pointed, and the verdict that implies for
/// the task registered into it.
struct Row {
    case: &'static str,
    /// The target group's name, and the ECS service that binds it.
    group_name: &'static str,
    service_name: &'static str,
    /// The listener port that makes the group one the load balancer forwards to.
    listener_port: &'static str,
    /// `HealthCheckPort` as the create spells it, or `None` for a create that leaves it out.
    health_check_port: Option<&'static str>,
    /// The `TargetHealth$State` the registration must reach.
    state: &'static str,
    /// The `TargetHealth$Reason` values AWS gives for that state; empty for a state reported
    /// without a reason code ("if the target state is `healthy`, a reason code is not
    /// provided", `TargetHealth$Reason`).
    reasons: &'static [&'static str],
    /// The `HealthCheckPort` the description must report: the port the check connects to.
    reported_port: &'static str,
}

const ROWS: [Row; 3] = [
    Row {
        case: "a health check left at the traffic port the container listens on",
        group_name: "hcport-control",
        service_name: "control",
        listener_port: "80",
        health_check_port: None,
        state: "healthy",
        reasons: &[],
        reported_port: "80",
    },
    Row {
        case: "the recorded configuration: a health check on a port nothing listens on",
        group_name: "hcport-mismatch",
        service_name: "mismatch",
        listener_port: "8080",
        health_check_port: Some(UNSERVED_PORT),
        state: "unhealthy",
        reasons: &["Target.Timeout", "Target.FailedHealthChecks"],
        reported_port: UNSERVED_PORT,
    },
    Row {
        case: "a health check on a second port the container also listens on",
        group_name: "hcport-second",
        service_name: "second",
        listener_port: "8090",
        health_check_port: Some("8081"),
        state: "healthy",
        reasons: &[],
        reported_port: "8081",
    },
];

/// One emulator carrying Elastic Load Balancing v2 and ECS over ONE shared engine, exactly
/// as the production router builds them, with the network the placements name held in the
/// account's EC2 store — which both services resolve a subnet against.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    crate::ec2_network::register_network(
        &engine,
        VPC,
        &[SUBNET, SECOND_SUBNET],
        &format!("{REGION}a"),
    );
    // The tasks below have to RUN, and ECS reads the path to the image registry off the
    // account's network records before it starts a task.
    crate::ec2_network::register_internet_route(&engine, VPC);
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::EmulatedService::new(engine),
    );
    router
}

/// One awsQuery call to the ELBv2 model, answered with its XML body.
async fn elbv2(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> String {
    let mut body = format!("Action={action}&Version={ELB_VERSION}");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("elasticloadbalancingv2.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}\
                 /elasticloadbalancingv2/aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {action}: expected Elastic Load Balancing to answer 200, got {status} with {body}"
    );
    body
}

/// One awsJson1_1 call to ECS, answered with the HTTP status and the parsed body.
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
    let parsed = if payload.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&payload)
            .unwrap_or_else(|_| json!({ "__unparsed": String::from_utf8_lossy(&payload) }))
    };
    (status, parsed)
}

/// One ECS call that must succeed, answered with its body.
async fn ecs_ok(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let (status, parsed) = ecs(router, operation, body).await;
    assert_eq!(
        status, 200,
        "setup: ECS {operation} answered {status} with {parsed}"
    );
    parsed
}

/// Create the cluster and the revision every row's service runs: an `awsvpc`,
/// Fargate-compatible revision whose one container declares two container ports.
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
                "portMappings": [
                    { "containerPort": CONTAINER_PORT },
                    { "containerPort": SECOND_CONTAINER_PORT },
                ],
            }],
        }),
    )
    .await;
}

/// Create the `ip` target group one row describes and answer its ARN.
async fn target_group(router: &MultiServiceRouter, row: &Row) -> String {
    let mut params: Vec<(&str, &str)> = vec![
        ("Name", row.group_name),
        ("Protocol", "HTTP"),
        ("Port", "80"),
        ("TargetType", "ip"),
        ("VpcId", VPC),
        ("HealthCheckProtocol", "HTTP"),
        ("HealthCheckPath", "/"),
        (
            "HealthCheckIntervalSeconds",
            HEALTH_CHECK_INTERVAL_SECONDS,
        ),
    ];
    if let Some(port) = row.health_check_port {
        params.push(("HealthCheckPort", port));
    }
    let created = elbv2(router, "CreateTargetGroup", &params).await;
    opt_element(&created, "TargetGroupArn")
        .unwrap_or_else(|| panic!("case {:?}: no TargetGroupArn in {created}", row.case))
}

/// Create the ECS service one row describes, binding `target_group` on the container port
/// the revision declares, and answer nothing: what it placed is read back below.
async fn create_service(router: &MultiServiceRouter, row: &Row, target_group: &str) {
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
                    "subnets": [SUBNET],
                    "securityGroups": ["sg-0123456789abcdef0"],
                    // The task's own public address, which is what makes the internet
                    // gateway its subnet routes through a path to the image's registry.
                    "assignPublicIp": "ENABLED",
                }
            },
            "loadBalancers": [{
                "targetGroupArn": target_group,
                "containerName": CONTAINER,
                "containerPort": CONTAINER_PORT,
            }],
        }),
    )
    .await;
    assert_eq!(
        created["service"]["loadBalancers"][0]["targetGroupArn"].as_str(),
        Some(target_group),
        "case {:?}: the stored service must carry the binding, got {created}",
        row.case
    );
}

/// The private IPv4 address of the one RUNNING task of `service_name`, once its scheduler has
/// placed it and the clock has started it.
async fn running_task_address(router: &MultiServiceRouter, case: &str, service_name: &str) -> String {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let described = ecs_ok(
            router,
            "DescribeServices",
            json!({ "cluster": CLUSTER, "services": [service_name] }),
        )
        .await;
        let running = described["services"][0]["runningCount"]
            .as_i64()
            .unwrap_or_default();
        if running == 1 {
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
    let address = task["attachments"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|attachment| attachment["details"].as_array())
        .flatten()
        .find(|detail| detail["name"].as_str() == Some("privateIPv4Address"))
        .and_then(|detail| detail["value"].as_str())
        .unwrap_or_else(|| {
            panic!("case {case:?}: the task carries its interface's address, got {task}")
        });
    address.to_string()
}

/// `(target id, state, reason, description, health-check port)` for every registration the
/// group reports.
async fn health(
    router: &MultiServiceRouter,
    arn: &str,
) -> Vec<(String, String, String, String, String)> {
    let body = elbv2(
        router,
        "DescribeTargetHealth",
        &[("TargetGroupArn", arn)],
    )
    .await;
    crate::query_wire::members_of(&body)
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
                text(description, "HealthCheckPort"),
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
) -> (Vec<(String, String, String, String, String)>, Vec<String>) {
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
async fn an_ecs_task_is_healthy_only_while_its_container_listens_on_the_checked_port() {
    let router = emulator();
    seed_ecs(&router).await;

    let load_balancer = opt_element(
        &elbv2(
            &router,
            "CreateLoadBalancer",
            &[
                ("Name", "hcport-alb"),
                ("Subnets.member.1", SUBNET),
                ("Subnets.member.2", SECOND_SUBNET),
            ],
        )
        .await,
        "LoadBalancerArn",
    )
    .expect("the create answers the load balancer's ARN");

    let mut groups: Vec<String> = Vec::new();
    for row in &ROWS {
        let group = target_group(&router, row).await;
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
        create_service(&router, row, &group).await;
        groups.push(group);
    }

    for (group, row) in groups.iter().zip(&ROWS) {
        // The task ECS placed, by the address its own interface was allocated: the target
        // the group must hold is that task and not a value this suite made up.
        let address = running_task_address(&router, row.case, row.service_name).await;
        let (reported, seen) = health_reaching(&router, row.case, group, row.state).await;
        assert_eq!(
            reported.len(),
            1,
            "case {:?}: expected the group to hold the service's one task, got {reported:?}",
            row.case
        );
        for (id, state, reason, description, port) in &reported {
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
                    "case {:?}: a non-healthy state carries a description, got \
                     {state}/{reason}",
                    row.case
                );
            }
            // The port the check connects to is the one the description reports.
            assert_eq!(
                port, row.reported_port,
                "case {:?}: expected the health check on port {}, got {port:?}",
                row.case, row.reported_port
            );
        }
        if row.state != "healthy" {
            assert!(
                !seen.iter().any(|state| state == "healthy"),
                "case {:?}: a target whose health check reaches nothing must never be \
                 reported healthy, got the states {seen:?}",
                row.case
            );
        }
    }
}
