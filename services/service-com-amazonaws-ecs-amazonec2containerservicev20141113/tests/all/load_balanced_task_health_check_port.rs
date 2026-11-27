// SPDX-License-Identifier: CC0-1.0

//! The HEALTH Elastic Load Balancing reports for the task of a load-balanced ECS service
//! follows whether that task's container listens on the port the target group's health check
//! connects to.
//!
//! The rule under test, over the whole class rather than one request:
//!
//! > A load balancer health-checks a registration on the target group's EFFECTIVE
//! > health-check port — `HealthCheckPort` when it names one, and the port the target
//! > receives traffic on when it names `traffic-port` or nothing — so an ECS task whose
//! > containers do not listen on that port answers no check: it is never reported `healthy`
//! > and settles on `unhealthy`, carrying one of the reason codes `TargetHealth$Reason`
//! > documents for that state and a description, while a task whose containers do listen on
//! > it converges to `healthy` with no reason code and no description. What a task listens
//! > on is what its revision declares: the `containerPort`s of its containers'
//! > `portMappings`, which for an `awsvpc` task are the ports its own interface's address is
//! > reached on.
//!
//! `TargetHealthStateEnum` names the states (`initial`, `healthy`, `unhealthy`, …) and
//! `TargetHealth$Reason` the codes each is reported with — "if the target state is `healthy`,
//! a reason code is not provided", and for `unhealthy` one of `Target.ResponseCodeMismatch`,
//! `Target.Timeout`, `Target.FailedHealthChecks`, `Elb.InternalError`
//! (`smithy_query(query="shape", shape="TargetHealth",
//! model_path="services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/smithy-model/com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10.json")`).
//! That a static mapping is what an `awsvpc` task's ports are is `PortMapping`'s own: "for
//! containers in a task with the `awsvpc` network mode, the `hostPortRange` is set to the same
//! value as the `containerPortRange`".
//!
//! The rows differ along what the rule generalises over: a health check left at the traffic
//! port, one pointed at a port the container declares no mapping for, and one pointed at a
//! SECOND port the container does declare. The configuration a recording happens to show — a
//! container port of 80 beside a health-check port of 8888 — is a single row. A second test
//! pins the bound the declared set is held to: a revision declaring more port ranges than a
//! registration carries has the first that many read, and a check against a port outside
//! them still fails.
//!
//! Nothing here reaches into a state struct: the target group, the load balancer and the
//! listener are created through the Elastic Load Balancing v2 API on the one shared engine,
//! the service through the ECS front door, and every value asserted is read back through
//! `DescribeTasks` and `DescribeTargetHealth`.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{list, ok, AWSVPC_SUBNET, AWSVPC_START};
use crate::load_balancer_target_groups::{
    attach_load_balancer, elb_call, emulator, target_health_of, VPC,
};
use crate::task_lifecycle::container;
use service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::MAX_SERVED_PORT_RANGES;

/// The cluster, the task-definition family and the container every row runs.
const CLUSTER: &str = "hcport";
const CONTAINER: &str = "web";

/// The ports the container declares in its `portMappings`: the one every binding attaches,
/// and a second one it also listens on.
const CONTAINER_PORT: i64 = 80;
const SECOND_CONTAINER_PORT: i64 = 8081;

/// A port the container declares NO mapping for, so nothing in the task listens on it.
const UNSERVED_PORT: &str = "8888";

/// How often a poll re-reads a surface it is waiting on, and how long it keeps trying: the
/// whole start of an `awsvpc` task plus the health checks the target group's own cadence runs
/// ([`HEALTH_CHECK_INTERVAL_SECONDS`]), with margin for a loaded test machine.
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const POLL_BUDGET: Duration = AWSVPC_START.saturating_add(Duration::from_secs(90));

/// The `HealthCheckIntervalSeconds` every target group below is created with: a verdict takes
/// `UnhealthyThresholdCount` consecutive failing checks, each one interval after the last, so
/// a suite that waits for one asks for the shortest interval the member admits ("the range is
/// 5-300").
const HEALTH_CHECK_INTERVAL_SECONDS: &str = "5";

/// One row: how the target group's health check is pointed, and the verdict that implies for
/// the task registered into it.
struct Row {
    case: &'static str,
    /// The target group's name, and the service that binds it.
    group_name: &'static str,
    service_name: &'static str,
    /// `HealthCheckPort` as the create spells it, or `None` for a create that leaves it out.
    health_check_port: Option<&'static str>,
    /// The `TargetHealth$State` the registration must reach.
    state: &'static str,
    /// The `TargetHealth$Reason` values AWS gives for that state; empty for a state reported
    /// without a reason code.
    reasons: &'static [&'static str],
    /// The `HealthCheckPort` the description must report: the port the check connects to.
    reported_port: &'static str,
}

const ROWS: [Row; 3] = [
    Row {
        case: "a health check left at the traffic port the container listens on",
        group_name: "hcport-control",
        service_name: "control",
        health_check_port: None,
        state: "healthy",
        reasons: &[],
        reported_port: "80",
    },
    Row {
        case: "the recorded configuration: a health check on a port nothing listens on",
        group_name: "hcport-mismatch",
        service_name: "mismatch",
        health_check_port: Some(UNSERVED_PORT),
        state: "unhealthy",
        reasons: &["Target.Timeout", "Target.FailedHealthChecks"],
        reported_port: UNSERVED_PORT,
    },
    Row {
        case: "a health check on a second port the container also declares",
        group_name: "hcport-second",
        service_name: "second",
        health_check_port: Some("8081"),
        state: "healthy",
        reasons: &[],
        reported_port: "8081",
    },
];

/// Register the one revision every row's service runs: an `awsvpc`, Fargate-compatible
/// revision whose container declares two container ports.
async fn register_definition(router: &MultiServiceRouter, case: &str) {
    let mut web = container(CONTAINER, "public.ecr.aws/nginx/nginx:latest");
    web["portMappings"] = json!([
        { "containerPort": CONTAINER_PORT, "protocol": "tcp" },
        { "containerPort": SECOND_CONTAINER_PORT, "protocol": "tcp" },
    ]);
    ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": CLUSTER,
            "networkMode": "awsvpc",
            "cpu": "256",
            "memory": "512",
            "requiresCompatibilities": ["FARGATE"],
            "containerDefinitions": [web],
        }),
        case,
    )
    .await;
}

/// Create the `ip` target group one row describes, attach a load balancer that forwards to
/// it, and answer its ARN.
async fn target_group(router: &MultiServiceRouter, row: &Row) -> String {
    let mut params: Vec<(&str, &str)> = vec![
        ("Name", row.group_name),
        ("TargetType", "ip"),
        ("Protocol", "HTTP"),
        ("Port", "80"),
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
    let (status, body) = elb_call(router, "CreateTargetGroup", &params).await;
    assert_eq!(
        status, 200,
        "case {:?}: CreateTargetGroup must succeed, got {status} with {body}",
        row.case
    );
    let arn = crate::load_balancer_target_groups::first_text(&body, "TargetGroupArn")
        .unwrap_or_else(|| panic!("case {:?}: no TargetGroupArn in {body}", row.case));
    // A target group no load balancer forwards to is checked by nothing — and cannot be
    // bound to a service at all.
    attach_load_balancer(router, row.group_name, &arn, row.case).await;
    arn
}

/// Create the service one row describes: one task of the shared revision, bound to
/// `target_group` on the container port the revision declares.
async fn create_service(router: &MultiServiceRouter, row: &Row, target_group: &str) {
    let created = ok(
        router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": row.service_name,
            "taskDefinition": CLUSTER,
            "desiredCount": 1,
            "launchType": "FARGATE",
            "networkConfiguration": {
                "awsvpcConfiguration": {
                    "subnets": [AWSVPC_SUBNET],
                    "securityGroups": ["sg-0123456789abcdef0"],
                }
            },
            "loadBalancers": [{
                "targetGroupArn": target_group,
                "containerName": CONTAINER,
                "containerPort": CONTAINER_PORT,
            }],
        }),
        row.case,
    )
    .await;
    assert_eq!(
        created["service"]["loadBalancers"][0]["targetGroupArn"]
            .as_str()
            .unwrap_or_default(),
        target_group,
        "case {:?}: the stored service must carry the binding it was created with, got \
         {created}",
        row.case
    );
}

/// The family the over-the-bound revision of the second test is registered under, and the
/// target group and service that run it.
const OVERSIZE_FAMILY: &str = "hcportoversize";
const OVERSIZE_GROUP: &str = "hcport-oversize";
const OVERSIZE_SERVICE: &str = "oversize";

/// A revision declaring more container-port ranges than a registration carries has the
/// first that many of them read, and the ports it declares past the bound are not: a check
/// against a port OUTSIDE the ones kept still fails, so the registration keeps a claim
/// rather than falling back to claiming nothing.
///
/// The bound is the crate's own (`state::MAX_SERVED_PORT_RANGES`, "a maximum of 100 port
/// ranges per container" times the containers one revision may describe), read from the
/// crate rather than restated, and the revision here is one mapping past it. The health
/// check is pointed at a port the revision declares no mapping for at all, which is outside
/// the ranges kept either way.
#[tokio::test]
async fn a_revision_declaring_more_port_ranges_than_a_registration_carries_keeps_the_first() {
    let case = "a revision declaring one port mapping more than a registration carries";
    let router = emulator();
    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": CLUSTER }),
        case,
    )
    .await;

    let mappings: Vec<Value> = (1..=MAX_SERVED_PORT_RANGES + 1)
        .map(|port| json!({ "containerPort": port, "protocol": "tcp" }))
        .collect();
    let mut web = container(CONTAINER, "public.ecr.aws/nginx/nginx:latest");
    web["portMappings"] = Value::Array(mappings);
    ok(
        &router,
        "RegisterTaskDefinition",
        json!({
            "family": OVERSIZE_FAMILY,
            "networkMode": "awsvpc",
            "cpu": "256",
            "memory": "512",
            "requiresCompatibilities": ["FARGATE"],
            "containerDefinitions": [web],
        }),
        case,
    )
    .await;

    // The health check is pointed at a port the revision declares no mapping for at all,
    // which is the port a registration with declared ports would fail on.
    let (status, body) = elb_call(
        &router,
        "CreateTargetGroup",
        &[
            ("Name", OVERSIZE_GROUP),
            ("TargetType", "ip"),
            ("Protocol", "HTTP"),
            ("Port", "80"),
            ("VpcId", VPC),
            ("HealthCheckProtocol", "HTTP"),
            ("HealthCheckPath", "/"),
            ("HealthCheckPort", UNSERVED_PORT),
            (
                "HealthCheckIntervalSeconds",
                HEALTH_CHECK_INTERVAL_SECONDS,
            ),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateTargetGroup must succeed, got {status} with {body}"
    );
    let group = crate::load_balancer_target_groups::first_text(&body, "TargetGroupArn")
        .unwrap_or_else(|| panic!("case {case:?}: no TargetGroupArn in {body}"));
    attach_load_balancer(&router, OVERSIZE_GROUP, &group, case).await;

    ok(
        &router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": OVERSIZE_SERVICE,
            "taskDefinition": OVERSIZE_FAMILY,
            "desiredCount": 1,
            "launchType": "FARGATE",
            "networkConfiguration": {
                "awsvpcConfiguration": {
                    "subnets": [AWSVPC_SUBNET],
                    "securityGroups": ["sg-0123456789abcdef0"],
                }
            },
            "loadBalancers": [{
                "targetGroupArn": &group,
                "containerName": CONTAINER,
                "containerPort": CONTAINER_PORT,
            }],
        }),
        case,
    )
    .await;

    let address = running_task_address(&router, case, OVERSIZE_SERVICE).await;
    let (reported, seen) = health_reaching(&router, case, &group, "unhealthy").await;
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: expected the group to hold the service's one task, got {reported:?}"
    );
    for (id, state, reason, description, port) in &reported {
        assert_eq!(
            id, &address,
            "case {case:?}: the registration is the task's own interface address {address}, \
             got {id:?}"
        );
        assert!(
            ["Target.Timeout", "Target.FailedHealthChecks"].contains(&reason.as_str())
                && !description.is_empty(),
            "case {case:?}: expected a reason code for the failing check and a description, \
             got {state}/{reason}/{description:?}"
        );
        assert_eq!(
            port, UNSERVED_PORT,
            "case {case:?}: expected the health check on port {UNSERVED_PORT}, got {port:?}"
        );
    }
    assert!(
        !seen.iter().any(|state| state == "healthy"),
        "case {case:?}: a target whose health check reaches nothing must never be reported \
         healthy, got the states {seen:?}"
    );
}

/// Poll until the service `service_name` has its one RUNNING task, and answer the private
/// IPv4 address of the elastic network interface that task was allocated.
async fn running_task_address(router: &MultiServiceRouter, case: &str, service_name: &str) -> String {
    let deadline = Instant::now() + POLL_BUDGET;
    loop {
        let listed = ok(
            router,
            "ListTasks",
            json!({ "cluster": CLUSTER, "serviceName": service_name }),
            case,
        )
        .await;
        let arns: Vec<Value> = list(&listed, "taskArns", case);
        if !arns.is_empty() {
            let described = ok(
                router,
                "DescribeTasks",
                json!({ "cluster": CLUSTER, "tasks": arns }),
                case,
            )
            .await;
            let address = list(&described, "tasks", case)
                .into_iter()
                .filter(|task| task["lastStatus"] == json!("RUNNING"))
                .flat_map(|task| list(&task, "attachments", case))
                .flat_map(|attachment| list(&attachment, "details", case))
                .find(|detail| detail["name"] == json!("privateIPv4Address"))
                .and_then(|detail| detail["value"].as_str().map(str::to_string));
            if let Some(address) = address {
                return address;
            }
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected {service_name} to run its one task with an interface \
             address within the poll budget, got {listed}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// `(target id, state, reason, description, health-check port)` for every registration the
/// group reports.
///
/// One `TargetHealthDescription` carries one of each leaf, in that order, so the leaf
/// sequences pair up — which the length check asserts rather than assumes. A state reported
/// with no reason code leaves both the reason and the description out, so those two are read
/// per description block instead of as whole-document sequences.
async fn health(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
) -> Vec<(String, String, String, String, String)> {
    let (status, body) =
        elb_call(router, "DescribeTargetHealth", &[("TargetGroupArn", arn)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeTargetHealth must succeed, got {status} with {body}"
    );
    let text = |block: &str, tag: &str| {
        crate::load_balancer_target_groups::first_text(block, tag).unwrap_or_default()
    };
    body.split("<member")
        .skip(1)
        .filter_map(|block| block.split_once('>'))
        .filter_map(|(_, rest)| rest.split("</member>").next())
        .map(|block| {
            // The health's own element, because the override reported beside it carries
            // members of the same names ([`target_health_of`]).
            let health = target_health_of(block);
            (
                text(block, "Id"),
                text(health, "State"),
                text(health, "Reason"),
                text(health, "Description"),
                text(block, "HealthCheckPort"),
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
    let deadline = Instant::now() + POLL_BUDGET;
    let mut seen: Vec<String> = Vec::new();
    loop {
        let reported = health(router, case, arn).await;
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
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[tokio::test]
async fn a_tasks_target_is_healthy_only_while_its_container_listens_on_the_checked_port() {
    let router = emulator();
    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": CLUSTER }),
        "setup",
    )
    .await;
    register_definition(&router, "setup").await;

    let mut groups: Vec<String> = Vec::new();
    for row in &ROWS {
        let group = target_group(&router, row).await;
        create_service(&router, row, &group).await;
        groups.push(group);
    }

    for (group, row) in groups.iter().zip(&ROWS) {
        // The task ECS placed, by the address its own interface was allocated: the target the
        // group holds must be that task and not a value this suite made up.
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
                "case {:?}: the registration is the task's own interface address {address}, \
                 got {id:?}",
                row.case
            );
            assert_eq!(
                state, row.state,
                "case {:?}: expected {}, got {state}/{reason}/{description:?}",
                row.case, row.state
            );
            if row.reasons.is_empty() {
                assert!(
                    reason.is_empty() && description.is_empty(),
                    "case {:?}: a healthy target is reported with no reason code and no \
                     description, got {reason:?}/{description:?}",
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
                    "case {:?}: a non-healthy state carries the detail its reason is \
                     described by, got {state}/{reason}",
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
