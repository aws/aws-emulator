// SPDX-License-Identifier: CC0-1.0

//! Scaling a load-balanced ECS service IN takes its task out of the target group exactly as a
//! deregistration does: through `draining`, for as long as the group's own deregistration
//! delay says, and only then out of the group altogether.
//!
//! RULE: a task an ECS service stops is DEREGISTERED from every target group its placement's
//! `loadBalancers` bind, by the same state machine `DeregisterTargets` drives — the
//! registration is reported `draining` with `Target.DeregistrationInProgress` ("the load
//! balancer is draining connections to the target", `TargetHealth$State`) and leaves the
//! group only once the group's `deregistration_delay.timeout_seconds` has elapsed ("the
//! amount of time, in seconds, for Elastic Load Balancing to wait before changing the state
//! of a deregistering target from `draining` to `unused`", `TargetGroupAttribute$Key`). The
//! delay is the value the ATTRIBUTE carries, so a drain configured to take no time removes
//! the registration at once with no `draining` phase at all, and one configured long keeps it
//! in the group reporting `draining` until that span is over.
//!
//! Two crates have to agree for that to hold, which is why this suite lives here: ECS owns
//! the service's task and the stop that scaling it in performs, while Elastic Load Balancing
//! v2 owns the registration, the delay attribute and the health it reports. Everything is
//! driven through the real operations on one shared engine, so what is asserted is what each
//! service persisted — and the drain is observed through `DescribeTargetHealth` alone, which
//! is the read a client that waits for a target to leave rotation polls.
//!
//! The rows differ along what the rule generalises over: how many tasks the service drains at
//! once, and what the delay is configured to — a span the attribute's documented range admits
//! at its bottom (`0`), one in the middle, its published top (`3600`) and the documented
//! default a caller wrote nothing for (`300`). The recorded configuration — one task behind a
//! group with a short, finite delay — is a single row.
//!
//! The sibling suite `ecs_task_registration_precedes_its_health_verdict.rs` pins the
//! registration a RUNNING task owes; this one pins what the stop of that task owes.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The ONE percent-encoder and element readers this test crate keeps for awsQuery suites
// (`all/query_wire.rs`: "Nothing new should write another one — call this module").
use crate::query_wire::{
    element, elements, form_encode, members_of, opt_element, pairs_of, target_health_of,
};

/// The region every request below is signed for.
const REGION: &str = "us-east-1";

/// The API versions an ec2Query and an awsQuery request carry.
const EC2_API_VERSION: &str = "2016-11-15";
const ELB_VERSION: &str = "2015-12-01";

/// The ECS cluster, task-definition family and container every row runs.
const CLUSTER: &str = "drained";
const FAMILY: &str = "drainedfamily";
const CONTAINER: &str = "app";

/// The port the container declares a `portMapping` for, and the port every binding names.
const CONTAINER_PORT: i64 = 80;

/// The network every row runs in.
const VPC_CIDR: &str = "10.90.0.0/16";
const SUBNET_CIDRS: [(&str, &str); 2] = [
    ("10.90.1.0/24", "us-east-1a"),
    ("10.90.2.0/24", "us-east-1b"),
];

/// The target-group attribute each row configures its drain through, and the value the
/// attribute is carried at for a caller who wrote none: its own documentation says "the amount
/// of time, in seconds, for Elastic Load Balancing to wait before changing the state of a
/// deregistering target from `draining` to `unused`. The range is 0-3600 seconds. The default
/// value is 300 seconds."
/// (`smithy_query(query="doc",shape="TargetGroupAttribute",member="Key")`).
const DELAY_ATTRIBUTE: &str = "deregistration_delay.timeout_seconds";
const DEFAULT_DELAY_SECONDS: &str = "300";

/// The `TargetHealthStateEnum` value a deregistering registration is reported in, and the
/// `TargetHealthReasonEnum` reason that state carries: "`Target.DeregistrationInProgress` -
/// The target is in the process of being deregistered and the deregistration delay period has
/// not expired" (`TargetHealth$Reason`).
const DRAINING_STATE: &str = "draining";
const DRAINING_REASON: &str = "Target.DeregistrationInProgress";

/// How long a row waits for the clock-owed moves it depends on — its tasks' starts, the
/// registrations those starts owe, the stop its scale-in asks for and the drain that stop
/// begins — and how often it looks. Generous on purpose: the deadline is only here so a
/// failure is a failure rather than a hang.
const DEADLINE: Duration = Duration::from_secs(120);
const POLL: Duration = Duration::from_millis(100);

/// One row: the service it creates, the drain its target group is configured with, and what
/// scaling that service to no tasks must then be observable as.
struct Row {
    case: &'static str,
    /// The target group's name, the listener port that makes it one the load balancer
    /// forwards to, and the ECS service that binds it.
    group_name: &'static str,
    listener_port: &'static str,
    service_name: &'static str,
    /// How many tasks the service runs before it is scaled in, which is how many
    /// registrations the group holds and how many then drain.
    desired: i64,
    /// The value written to [`DELAY_ATTRIBUTE`], or `None` for a row that writes none and
    /// takes the documented default.
    delay_seconds: Option<&'static str>,
    /// Whether that delay is long enough for the drain to be OBSERVABLE: a drain of no time
    /// removes the registration at once, so it is never reported `draining`.
    drains: bool,
    /// Whether the registration must then LEAVE the group within this suite's deadline: only
    /// a delay shorter than it can elapse, so a row configured longer asserts the
    /// registration is still held instead.
    leaves: bool,
}

const ROWS: [Row; 5] = [
    Row {
        case: "the recorded configuration: one task behind a short, finite drain",
        group_name: "drained-short",
        listener_port: "80",
        service_name: "short",
        desired: 1,
        delay_seconds: Some("5"),
        drains: true,
        leaves: true,
    },
    Row {
        case: "two tasks draining out of one group together",
        group_name: "drained-two",
        listener_port: "8080",
        service_name: "two",
        desired: 2,
        delay_seconds: Some("5"),
        drains: true,
        leaves: true,
    },
    Row {
        case: "the bottom of the attribute's range: a drain of no time",
        group_name: "drained-none",
        listener_port: "8081",
        service_name: "none",
        desired: 1,
        delay_seconds: Some("0"),
        drains: false,
        leaves: true,
    },
    Row {
        case: "the top of the attribute's range",
        group_name: "drained-max",
        listener_port: "8082",
        service_name: "max",
        desired: 1,
        delay_seconds: Some("3600"),
        drains: true,
        leaves: false,
    },
    Row {
        case: "the documented default, written by no caller",
        group_name: "drained-default",
        listener_port: "8083",
        service_name: "default",
        desired: 1,
        delay_seconds: None,
        drains: true,
        leaves: false,
    },
];

impl Row {
    /// The delay the group must report for this row: the value it wrote, else the documented
    /// default the attribute is carried at.
    fn expected_delay(&self) -> &'static str {
        self.delay_seconds.unwrap_or(DEFAULT_DELAY_SECONDS)
    }

    /// How many registrations this row's group holds while its service runs.
    fn registrations(&self) -> usize {
        usize::try_from(self.desired).unwrap_or_default()
    }
}

/// One registration as `DescribeTargetHealth` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    /// `TargetDescription$Id`: the address the load balancer reaches the target at.
    id: String,
    /// `TargetDescription$Port`: the port it receives traffic on.
    port: String,
    /// `TargetHealth$State`, `TargetHealth$Reason` and `TargetHealth$Description`; a member
    /// the answer omits reads as empty.
    state: String,
    reason: String,
    description: String,
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

/// The network every row runs in: a VPC with an internet gateway, two subnets, and the one
/// security group the load balancer and the tasks are placed in.
struct Network {
    vpc: String,
    subnets: Vec<String>,
    group: String,
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
    let group = element(
        &ec2(
            router,
            "CreateSecurityGroup",
            &[
                ("GroupName", "drained-group"),
                ("GroupDescription", "the group this suites resources are in"),
                ("VpcId", vpc.as_str()),
            ],
        )
        .await,
        "groupId",
    );
    Network {
        vpc,
        subnets,
        group,
    }
}

/// Create the cluster and the revision every row's service runs: an `awsvpc`,
/// Fargate-compatible revision whose one container listens on the port every binding names.
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
                "image": "public.ecr.aws/docker/library/nginx:alpine",
                "memory": 128,
                "portMappings": [{ "containerPort": CONTAINER_PORT }],
            }],
        }),
    )
    .await;
}

/// Create one row's `ip` target group, configure the drain the row asks for, and assert the
/// group then REPORTS that delay — the value the deregistration below is held to.
async fn target_group(router: &MultiServiceRouter, network: &Network, row: &Row) -> String {
    let created = elbv2(
        router,
        "CreateTargetGroup",
        &[
            ("Name", row.group_name),
            ("Protocol", "HTTP"),
            ("Port", &CONTAINER_PORT.to_string()),
            ("TargetType", "ip"),
            ("VpcId", network.vpc.as_str()),
        ],
    )
    .await;
    let arn = opt_element(&created, "TargetGroupArn")
        .unwrap_or_else(|| panic!("case {:?}: no TargetGroupArn in {created}", row.case));
    if let Some(configured) = row.delay_seconds {
        elbv2(
            router,
            "ModifyTargetGroupAttributes",
            &[
                ("TargetGroupArn", arn.as_str()),
                ("Attributes.member.1.Key", DELAY_ATTRIBUTE),
                ("Attributes.member.1.Value", configured),
            ],
        )
        .await;
    }
    let described = elbv2(
        router,
        "DescribeTargetGroupAttributes",
        &[("TargetGroupArn", arn.as_str())],
    )
    .await;
    let attributes = pairs_of(&described, "Key", "Value");
    assert_eq!(
        attributes.get(DELAY_ATTRIBUTE).map(String::as_str),
        Some(row.expected_delay()),
        "case {:?}: expected the group to report a {} second drain, got {attributes:?}",
        row.case,
        row.expected_delay()
    );
    arn
}

/// Create the ECS service one row describes: its tasks in the first subnet, bound to `group`
/// on the one port the container listens on.
async fn create_service(router: &MultiServiceRouter, network: &Network, row: &Row, group: &str) {
    let created = ecs_ok(
        router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": row.service_name,
            "taskDefinition": FAMILY,
            "desiredCount": row.desired,
            "launchType": "FARGATE",
            "networkConfiguration": {
                "awsvpcConfiguration": {
                    "subnets": [network.subnets[0]],
                    "securityGroups": [network.group],
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
        created["service"]["desiredCount"].as_i64(),
        Some(row.desired),
        "case {:?}: the stored service must carry the count the request asked for, got {created}",
        row.case
    );
}

/// Scale one row's service to NO tasks, and assert the stored service carries that count.
async fn scale_in(router: &MultiServiceRouter, row: &Row) {
    let updated = ecs_ok(
        router,
        "UpdateService",
        json!({
            "cluster": CLUSTER,
            "service": row.service_name,
            "desiredCount": 0,
        }),
    )
    .await;
    assert_eq!(
        updated["service"]["desiredCount"].as_i64(),
        Some(0),
        "case {:?}: the scaled service must want no tasks, got {updated}",
        row.case
    );
}

/// Every registration `arn` reports.
async fn health(router: &MultiServiceRouter, arn: &str) -> Vec<Reported> {
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
            Reported {
                id: text(description, "Id"),
                port: text(description, "Port"),
                state: text(health, "State"),
                reason: text(health, "Reason"),
                description: text(health, "Description"),
            }
        })
        .collect()
}

/// The registrations of `arn` once `settled` holds of them, plus every `TargetHealth$State`
/// seen on the way there.
///
/// Polls the CONDITION rather than waiting a fixed time, and the deadline is generous: a row
/// is waiting for a task to start or stop, and for the drain the stop begins.
async fn health_until<F>(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
    expected: &str,
    settled: F,
) -> (Vec<Reported>, BTreeSet<String>)
where
    F: Fn(&[Reported]) -> bool,
{
    let deadline = Instant::now() + DEADLINE;
    let mut seen: BTreeSet<String> = BTreeSet::new();
    loop {
        let reported = health(router, arn).await;
        for description in &reported {
            seen.insert(description.state.clone());
        }
        let done = settled(&reported);
        if done || Instant::now() >= deadline {
            assert!(
                done,
                "case {case:?}: expected {expected}, got {reported:?} after the states {seen:?}"
            );
            return (reported, seen);
        }
        ::tokio::time::sleep(POLL).await;
    }
}

/// The addresses of the RUNNING tasks of one row's service, read back out of ECS itself.
async fn running_task_addresses(router: &MultiServiceRouter, row: &Row) -> BTreeSet<String> {
    let listed = ecs_ok(
        router,
        "ListTasks",
        json!({ "cluster": CLUSTER, "serviceName": row.service_name }),
    )
    .await;
    let arns: Vec<String> = listed["taskArns"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|arn| arn.as_str().map(str::to_string))
        .collect();
    let described = ecs_ok(
        router,
        "DescribeTasks",
        json!({ "cluster": CLUSTER, "tasks": arns }),
    )
    .await;
    let addresses: BTreeSet<String> = described["tasks"]
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
        row.registrations(),
        "case {:?}: expected {} task(s), each at its own interface's address, got {described}",
        row.case,
        row.desired
    );
    addresses
}

/// The addresses of the registrations `reported` describes.
fn registered_addresses(reported: &[Reported]) -> BTreeSet<String> {
    reported
        .iter()
        .map(|description| description.id.clone())
        .collect()
}

#[tokio::test]
async fn scaling_a_load_balanced_service_in_drains_then_deregisters_its_target() {
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
                ("Name", "drained-alb"),
                ("Subnets.member.1", network.subnets[0].as_str()),
                ("Subnets.member.2", network.subnets[1].as_str()),
                ("SecurityGroups.member.1", network.group.as_str()),
            ],
        )
        .await,
        "LoadBalancerArn",
    );

    let mut groups: Vec<String> = Vec::new();
    for row in &ROWS {
        let group = target_group(&router, &network, row).await;
        // A listener forwarding to the group is what makes it one the load balancer sends
        // traffic to — and so one whose deregistrations have connections to drain.
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
        create_service(&router, &network, row, &group).await;
        groups.push(group);
    }

    // Every row's tasks are registered first: the drain below is about a registration that
    // was there, so each row's precondition is asserted before it is scaled in.
    let mut addresses: Vec<BTreeSet<String>> = Vec::new();
    for (group, row) in groups.iter().zip(&ROWS) {
        let wanted = row.registrations();
        let (reported, _) = health_until(
            &router,
            row.case,
            group,
            &format!("{wanted} registration(s), none of them draining"),
            |reported| {
                reported.len() == wanted
                    && reported
                        .iter()
                        .all(|description| description.state != DRAINING_STATE)
            },
        )
        .await;
        let running = running_task_addresses(&router, row).await;
        assert_eq!(
            registered_addresses(&reported),
            running,
            "case {:?}: expected the group to hold exactly the tasks' own addresses, got \
             {reported:?}",
            row.case
        );
        for description in &reported {
            assert_eq!(
                description.port,
                CONTAINER_PORT.to_string(),
                "case {:?}: expected the registration on the bound port {CONTAINER_PORT}, got \
                 {description:?}",
                row.case
            );
        }
        addresses.push(running);
    }

    // Each row is then scaled in and observed on its own, so no row's drain runs while
    // another row is being watched.
    for ((group, row), running) in groups.iter().zip(&ROWS).zip(&addresses) {
        scale_in(&router, row).await;

        if row.drains {
            let (reported, _) = health_until(
                &router,
                row.case,
                group,
                &format!(
                    "every registration {DRAINING_STATE} with {DRAINING_REASON}, over a {} \
                     second drain",
                    row.expected_delay()
                ),
                |reported| {
                    reported.len() == row.registrations()
                        && reported
                            .iter()
                            .all(|description| description.state == DRAINING_STATE)
                },
            )
            .await;
            assert_eq!(
                &registered_addresses(&reported),
                running,
                "case {:?}: the draining registrations must be the very ones the tasks held, \
                 got {reported:?}",
                row.case
            );
            for description in &reported {
                assert_eq!(
                    description.reason, DRAINING_REASON,
                    "case {:?}: expected the reason {DRAINING_REASON}, got {description:?}",
                    row.case
                );
                assert!(
                    !description.description.is_empty(),
                    "case {:?}: a draining registration carries a description, got \
                     {description:?}",
                    row.case
                );
            }
        }

        if row.leaves {
            let (_, seen) = health_until(
                &router,
                row.case,
                group,
                &format!("the group to hold no registration after its {} second drain", row.expected_delay()),
                <[Reported]>::is_empty,
            )
            .await;
            if !row.drains {
                // A drain of no time has no connections to drain, so the registration is
                // removed by the deregistration itself and is never reported draining.
                assert!(
                    !seen.contains(DRAINING_STATE),
                    "case {:?}: a drain of no time must never be observable as \
                     {DRAINING_STATE}, got the states {seen:?}",
                    row.case
                );
            }
        } else {
            // The configured drain is longer than this whole suite's deadline, so the
            // registration cannot have left the group: it is still held, still draining.
            let reported = health(&router, group).await;
            assert_eq!(
                &registered_addresses(&reported),
                running,
                "case {:?}: a registration whose {} second drain has not elapsed is still held, \
                 got {reported:?}",
                row.case,
                row.expected_delay()
            );
            assert!(
                reported
                    .iter()
                    .all(|description| description.state == DRAINING_STATE),
                "case {:?}: a registration whose {} second drain has not elapsed is still \
                 {DRAINING_STATE}, got {reported:?}",
                row.case,
                row.expected_delay()
            );
        }
    }
}
