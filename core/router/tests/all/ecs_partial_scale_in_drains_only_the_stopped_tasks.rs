// SPDX-License-Identifier: CC0-1.0

//! Scaling a load-balanced ECS service IN takes exactly the tasks it stopped out of the target
//! group, and leaves the registrations of the tasks it keeps serving.
//!
//! RULE: a task an ECS service STOPS is deregistered from every target group its placement's
//! `loadBalancers` bind, by the same state machine `DeregisterTargets` drives — the
//! registration is reported `draining` with `Target.DeregistrationInProgress` ("the target is
//! in the process of being deregistered and the deregistration delay period has not expired",
//! `TargetHealth$Reason`) and leaves the group once the group's
//! `deregistration_delay.timeout_seconds` has elapsed — while a task the service KEEPS is a
//! task it never stopped, so its own registration is untouched by that drain. Which tasks
//! those are follows from the count alone (`UpdateService$desiredCount`, "the number of
//! instantiations of the task to place and keep running in your service"): a service reduced
//! to a count above zero stops its surplus tasks and keeps the rest registered, and scaling it
//! back out registers the tasks that replace them.
//!
//! The sibling suite `ecs_scale_in_drains_its_registered_target.rs` pins the drain itself over
//! the delays the attribute admits, with every row scaled to NO tasks. This one pins the half
//! that only a PARTIAL reduction can show: that the drain is scoped to the tasks the
//! reduction stopped. The rows differ along what the rule generalises over — how many of the
//! service's tasks the reduction stops (one of two, two of three) and whether the group's
//! configured drain makes that removal observable as `draining` at all (a drain of no time
//! removes the registration at once).
//!
//! Three crates have to agree for the rule to hold, which is why this suite lives here: Amazon
//! EC2 owns the network the tasks are placed in, Amazon ECS owns the tasks and the stops its
//! scale-in asks for, and Elastic Load Balancing v2 owns the registrations, the delay
//! attribute and the health it reports. Everything is driven through the real operations over
//! ONE shared engine, and the drain itself is observed through `DescribeTargetHealth` alone —
//! the read a client that waits for a target to leave rotation polls — while the set of
//! registrations before and after it is held against the tasks ECS's own read reports running.

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
const CLUSTER: &str = "partial";
const FAMILY: &str = "partialfamily";
const CONTAINER: &str = "app";

/// The port the container declares a `portMapping` for, and the port every binding names.
const CONTAINER_PORT: i64 = 80;

/// The network every row runs in.
const VPC_CIDR: &str = "10.91.0.0/16";
const SUBNET_CIDRS: [(&str, &str); 2] = [
    ("10.91.1.0/24", "us-east-1a"),
    ("10.91.2.0/24", "us-east-1b"),
];

/// The target-group attribute each row configures its drain through: "the amount of time, in
/// seconds, for Elastic Load Balancing to wait before changing the state of a deregistering
/// target from `draining` to `unused`. The range is 0-3600 seconds."
/// (`smithy_query(query="doc",shape="TargetGroupAttribute",member="Key")`).
const DELAY_ATTRIBUTE: &str = "deregistration_delay.timeout_seconds";

/// The `TargetHealthStateEnum` value a deregistering registration is reported in, and the
/// `TargetHealthReasonEnum` reason that state carries: "`Target.DeregistrationInProgress` -
/// The target is in the process of being deregistered and the deregistration delay period has
/// not expired" (`TargetHealth$Reason`).
const DRAINING_STATE: &str = "draining";
const DRAINING_REASON: &str = "Target.DeregistrationInProgress";

/// How long a row waits for the clock-owed moves it depends on — its tasks' starts, the
/// registrations those starts owe, the stops its reduction asks for, the drain those stops
/// begin and the starts the scale-out after it owes — and how often it looks. Generous on
/// purpose: the deadline is only here so a failure is a failure rather than a hang.
///
/// The interval is what makes the drain of a row that configures one OBSERVABLE rather than a
/// race: the registration stays `draining` for the whole delay the group is configured with,
/// which is orders of magnitude longer than one interval, so every poll inside that span sees
/// it and the drain cannot slip between two looks.
const DEADLINE: Duration = Duration::from_secs(180);
const POLL: Duration = Duration::from_millis(100);

/// One row: the service it creates, the count its reduction leaves, and the drain its group is
/// configured with.
struct Row {
    case: &'static str,
    /// The target group's name, the listener port that makes it one the load balancer
    /// forwards to, and the ECS service that binds it.
    group_name: &'static str,
    listener_port: &'static str,
    service_name: &'static str,
    /// How many tasks the service runs before it is scaled in, and how many it is left with:
    /// `kept` is above zero in every row, which is what makes the registrations it keeps
    /// observable beside the ones that drain.
    running: i64,
    kept: i64,
    /// The value written to [`DELAY_ATTRIBUTE`] for this row's group.
    delay_seconds: &'static str,
}

const ROWS: [Row; 3] = [
    Row {
        case: "one of two tasks stopped",
        group_name: "partial-one-of-two",
        listener_port: "80",
        service_name: "oneoftwo",
        running: 2,
        kept: 1,
        delay_seconds: "5",
    },
    Row {
        case: "two of three tasks stopped at once",
        group_name: "partial-two-of-three",
        listener_port: "8080",
        service_name: "twoofthree",
        running: 3,
        kept: 1,
        delay_seconds: "5",
    },
    Row {
        case: "one of two tasks stopped behind a drain of no time",
        group_name: "partial-no-drain",
        listener_port: "8081",
        service_name: "nodrain",
        running: 2,
        kept: 1,
        delay_seconds: "0",
    },
];

impl Row {
    /// How many registrations this row's group holds while its service runs its full count.
    fn registrations(&self) -> usize {
        usize::try_from(self.running).expect("the row's running count fits a usize")
    }

    /// How many it holds once the reduction has settled.
    fn kept_registrations(&self) -> usize {
        usize::try_from(self.kept).expect("the row's kept count fits a usize")
    }

    /// Whether this row's configured delay makes the removal observable as `draining`: a drain
    /// of no time removes the registration at once, so it is never reported in that state.
    ///
    /// Read off [`Self::delay_seconds`] rather than carried beside it, so a row cannot state a
    /// delay and an expectation that disagree, and read as the NUMBER the attribute carries, so
    /// any spelling of a value the attribute admits is derived from the same way.
    fn drains(&self) -> bool {
        self.delay_seconds
            .trim()
            .parse::<u32>()
            .expect("the row's delay is a number of seconds")
            > 0
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

/// What one poll loop saw on its way to the state it was waiting for.
#[derive(Debug, Default)]
struct Observed {
    /// The registrations of the final poll.
    reported: Vec<Reported>,
    /// Every `TargetHealth$State` seen on any poll.
    states: BTreeSet<String>,
    /// The address of every registration seen `draining` on any poll — what makes the claim
    /// "the tasks it kept never drained" an assertion rather than an absence.
    drained: BTreeSet<String>,
    /// The reasons the draining registrations carried, and their descriptions.
    draining_reasons: BTreeSet<String>,
    draining_descriptions: BTreeSet<String>,
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
                ("GroupName", "partial-group"),
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
/// group then REPORTS that delay — the value the deregistrations below are held to.
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
    elbv2(
        router,
        "ModifyTargetGroupAttributes",
        &[
            ("TargetGroupArn", arn.as_str()),
            ("Attributes.member.1.Key", DELAY_ATTRIBUTE),
            ("Attributes.member.1.Value", row.delay_seconds),
        ],
    )
    .await;
    let described = elbv2(
        router,
        "DescribeTargetGroupAttributes",
        &[("TargetGroupArn", arn.as_str())],
    )
    .await;
    let attributes = pairs_of(&described, "Key", "Value");
    assert_eq!(
        attributes.get(DELAY_ATTRIBUTE).map(String::as_str),
        Some(row.delay_seconds),
        "case {:?}: expected the group to report a {} second drain, got {attributes:?}",
        row.case,
        row.delay_seconds
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
            "desiredCount": row.running,
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
        Some(row.running),
        "case {:?}: the stored service must carry the count the request asked for, got {created}",
        row.case
    );
}

/// Ask one row's service for `count` tasks, and assert the stored service carries that count.
async fn scale_to(router: &MultiServiceRouter, row: &Row, count: i64) {
    let updated = ecs_ok(
        router,
        "UpdateService",
        json!({
            "cluster": CLUSTER,
            "service": row.service_name,
            "desiredCount": count,
        }),
    )
    .await;
    assert_eq!(
        updated["service"]["desiredCount"].as_i64(),
        Some(count),
        "case {:?}: the scaled service must want {count} task(s), got {updated}",
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

/// Poll until the group at `arn` holds `wanted` registrations, none of them draining, AND ECS
/// itself reports its running tasks at exactly those addresses — answering those registrations.
///
/// Both halves are read inside ONE loop, so the agreement is a condition this waits for rather
/// than a comparison of two reads taken at different instants: a task the scheduler moved
/// between them would otherwise fail the comparison instead of being waited out.
async fn agreement_until(
    router: &MultiServiceRouter,
    row: &Row,
    arn: &str,
    wanted: usize,
) -> Vec<Reported> {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let reported = health(router, arn).await;
        let held = registered_addresses(&reported);
        let settled = reported.len() == wanted
            && reported
                .iter()
                .all(|description| description.state != DRAINING_STATE);
        let running = running_task_addresses(router, row).await;
        if settled && running == held {
            return reported;
        }
        assert!(
            Instant::now() < deadline,
            "case {:?}: expected ECS and the group to agree on {wanted} registration(s), got \
             {reported:?} in the group against the {running:?} ECS reports running",
            row.case
        );
        ::tokio::time::sleep(POLL).await;
    }
}

/// Poll `arn` until `settled` holds of its registrations, answering what was seen on the way.
///
/// Polls the CONDITION rather than waiting a fixed time, and the deadline is generous: a row
/// is waiting for tasks to start or stop, and for the drain a stop begins.
async fn observe_until<F>(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
    expected: &str,
    settled: F,
) -> Observed
where
    F: Fn(&[Reported]) -> bool,
{
    let deadline = Instant::now() + DEADLINE;
    let mut observed = Observed::default();
    loop {
        let reported = health(router, arn).await;
        for description in &reported {
            observed.states.insert(description.state.clone());
            if description.state == DRAINING_STATE {
                observed.drained.insert(description.id.clone());
                observed
                    .draining_reasons
                    .insert(description.reason.clone());
                observed
                    .draining_descriptions
                    .insert(description.description.clone());
            }
        }
        let done = settled(&reported);
        if done || Instant::now() >= deadline {
            observed.reported = reported;
            assert!(
                done,
                "case {case:?}: expected {expected}, got {:?} after the states {:?}",
                observed.reported, observed.states
            );
            return observed;
        }
        ::tokio::time::sleep(POLL).await;
    }
}

/// The addresses of the RUNNING tasks of one row's service, read back out of ECS itself, which
/// is what the group's registrations are compared against.
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
    if arns.is_empty() {
        return BTreeSet::new();
    }
    let described = ecs_ok(
        router,
        "DescribeTasks",
        json!({ "cluster": CLUSTER, "tasks": arns }),
    )
    .await;
    described["tasks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|task| {
            task["lastStatus"].as_str() == Some("RUNNING")
                && task["desiredStatus"].as_str() == Some("RUNNING")
        })
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
        .collect()
}

/// The addresses of the registrations `reported` describes.
fn registered_addresses(reported: &[Reported]) -> BTreeSet<String> {
    reported
        .iter()
        .map(|description| description.id.clone())
        .collect()
}

/// Every registration of `reported` is on the port the binding named, whatever its state.
fn assert_bound_port(case: &str, reported: &[Reported]) {
    for description in reported {
        assert_eq!(
            description.port,
            CONTAINER_PORT.to_string(),
            "case {case:?}: expected the registration on the bound port {CONTAINER_PORT}, got \
             {description:?}"
        );
    }
}

#[tokio::test]
async fn scaling_a_service_part_of_the_way_in_drains_only_the_tasks_it_stopped() {
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
                ("Name", "partial-alb"),
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

    // Every row's full count is registered first: the reduction below is about registrations
    // that were there, so each row's precondition is asserted before it is scaled in.
    let mut addresses: Vec<BTreeSet<String>> = Vec::new();
    for (group, row) in groups.iter().zip(&ROWS) {
        // The group holds the full count and ECS reports its tasks at exactly those addresses:
        // both halves are waited for together ([`agreement_until`]).
        let reported = agreement_until(&router, row, group, row.registrations()).await;
        assert_bound_port(row.case, &reported);
        addresses.push(registered_addresses(&reported));
    }

    // Each row is then scaled in and observed on its own, so no row's drain runs while another
    // row is being watched.
    for ((group, row), running) in groups.iter().zip(&ROWS).zip(&addresses) {
        scale_to(&router, row, row.kept).await;

        // The terminal observation: the group holds exactly the registrations of the tasks the
        // service kept, and none of them is draining. Everything the drain itself did is read
        // off what the polls on the way there saw, so a stop the scheduler staggered is
        // observed the same way as one it made in a single pass.
        let kept = row.kept_registrations();
        let observed = observe_until(
            &router,
            row.case,
            group,
            &format!(
                "{kept} registration(s) left, none draining, after a {} second drain",
                row.delay_seconds
            ),
            |reported| {
                reported.len() == kept
                    && reported
                        .iter()
                        .all(|description| description.state != DRAINING_STATE)
            },
        )
        .await;
        // ECS's own read agrees about which tasks are left, so the group holds the tasks that
        // service reports running rather than a set only ELBv2 believes in — read in the one
        // loop that waits for the two to agree, which the poll above has already settled.
        let kept_registrations = agreement_until(&router, row, group, kept).await;
        let left = registered_addresses(&kept_registrations);
        assert!(
            left.is_subset(running),
            "case {:?}: the registrations left must be ones the running tasks held, got {left:?} \
             out of {running:?}",
            row.case
        );
        assert_bound_port(row.case, &kept_registrations);

        // The addresses the reduction took out: everything that was registered and is not any
        // more. The drain is scoped to exactly those — so a registration the service KEPT was
        // never reported draining on any poll, which is what this comparison asserts.
        let removed: BTreeSet<String> = running.difference(&left).cloned().collect();
        if row.drains() {
            assert_eq!(
                observed.drained, removed,
                "case {:?}: expected exactly the stopped tasks' registrations to drain, got \
                 {:?} draining out of the {running:?} registered and the {left:?} left",
                row.case, observed.drained
            );
            assert_eq!(
                observed.draining_reasons,
                BTreeSet::from([DRAINING_REASON.to_string()]),
                "case {:?}: expected every draining registration to carry {DRAINING_REASON}, got \
                 {:?}",
                row.case,
                observed.draining_reasons
            );
            assert!(
                observed
                    .draining_descriptions
                    .iter()
                    .all(|description| !description.is_empty()),
                "case {:?}: a draining registration carries a description, got {:?}",
                row.case,
                observed.draining_descriptions
            );
        } else {
            // A drain of no time has no connections to drain, so the registration is removed
            // by the deregistration itself and is never reported draining.
            assert!(
                observed.drained.is_empty() && !observed.states.contains(DRAINING_STATE),
                "case {:?}: a drain of no time must never be observable as {DRAINING_STATE}, got \
                 {:?} draining out of the states {:?}",
                row.case,
                observed.drained,
                observed.states
            );
        }

        // And scaling the same service back OUT registers the tasks that replace the drained
        // ones, with the kept registrations still in place: the reduction took out the tasks
        // it stopped and nothing else. The group's full count and ECS's own running tasks are
        // again waited for together.
        scale_to(&router, row, row.running).await;
        let restored = agreement_until(&router, row, group, row.registrations()).await;
        let registered = registered_addresses(&restored);
        assert!(
            left.is_subset(&registered),
            "case {:?}: the kept registrations must still be held after the scale-out, got \
             {registered:?} without {left:?}",
            row.case
        );
        assert_bound_port(row.case, &restored);
    }
}
