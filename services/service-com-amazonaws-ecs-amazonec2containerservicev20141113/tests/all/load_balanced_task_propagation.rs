// SPDX-License-Identifier: CC0-1.0

//! What a TASK of a load-balanced ECS service propagates outside this service: its
//! registration as an Elastic Load Balancing v2 target while it runs, the drain that
//! registration leaves through when the task stops, and the EventBridge event every status
//! it moves into is published as.
//!
//! The rules under test, each over the whole class rather than one request:
//!
//! > A service registers each of its RUNNING tasks in every target group its
//! > `loadBalancers` entries name, addressed as that group's target type requires — for a
//! > task of an `awsvpc` revision, the private IPv4 address of the elastic network
//! > interface it was allocated, on the binding's `containerPort` — and deregisters it when
//! > the task stops, however it was stopped; a deregistered target enters `draining` and
//! > leaves the group once the group's `deregistration_delay.timeout_seconds` has passed.
//!
//! > Every `lastStatus` a task MOVES INTO is published once onto the account's default
//! > EventBridge bus as `source` `aws.ecs` / `detail-type` `ECS Task State Change`,
//! > carrying that task's own `taskArn`, `clusterArn` and `lastStatus` in the event
//! > `detail`; a rule whose `EventPattern` matches delivers it to that rule's targets, and
//! > a rule that does not match receives nothing.
//!
//! `LoadBalancer$targetGroupArn`'s own doc is the address half: "If your service's task
//! definition uses the `awsvpc` network mode, you must choose `ip` as the target type, not
//! `instance` ... because tasks that use the `awsvpc` network mode are associated with an
//! elastic network interface, not an Amazon EC2 instance"; `Attachment$details` "for
//! elastic network interfaces ... includes the network interface ID, the MAC address, the
//! subnet ID, and the private IPv4 address"; the `draining` state and the delay that ends it
//! are `TargetHealthStateEnum` and `TargetGroupAttribute$Key`'s own
//! `deregistration_delay.timeout_seconds` ("the amount of time ... to wait before changing
//! the state of a deregistering target from `draining` to `unused`"). The event envelope and
//! its `detail` are recorded behaviour.
//!
//! The tables span what each rule generalises over: how many tasks the service runs, the
//! port the binding names (the group's own, and a different one), the configured drain (none
//! and a long one), and WHICH call stops the task (`StopTask`, a scale-in through
//! `UpdateService`, and `DeleteService`) — and, for the events, the kind of task (a
//! standalone `RunTask` task and a service's).
//!
//! Nothing here reaches into a state struct: the target group, the load balancer, the rule
//! and the queue are created through their own services' APIs on the one shared engine, and
//! every value asserted is read back through `DescribeTargetHealth`, `DescribeTasks` or
//! `ReceiveMessage`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{list, ok, string, AWSVPC_START, SIGNED_REGION};
use crate::load_balancer_target_groups::{
    all_texts, create_target_group, elb_call, health_texts,
};
use crate::task_lifecycle::container;

/// The `source` and `detail-type` Amazon ECS publishes a task state change under, spelled
/// out here rather than imported, so a wrong constant in the implementation cannot make a
/// test pass.
const ECS_EVENT_SOURCE: &str = "aws.ecs";
const TASK_STATE_CHANGE: &str = "ECS Task State Change";

/// The `lastStatus` values a task passes through, in the order the ECS task lifecycle takes
/// them: the statuses `Task$lastStatus` reports, as this emulator's own lifecycle moves a
/// task (`PROVISIONING` for a task whose interface is still being attached, then `PENDING`,
/// `RUNNING`, `DEACTIVATING` and `STOPPED`).
const TASK_LIFECYCLE: [&str; 5] = [
    "PROVISIONING",
    "PENDING",
    "RUNNING",
    "DEACTIVATING",
    "STOPPED",
];

/// How often a poll re-reads a surface it is waiting on.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How long a poll keeps trying before it gives up: the whole start of an `awsvpc` task,
/// several further transitions and the release of the interface such a task was allocated,
/// which its stop waits for (one `state::INTERFACE_RELEASE_STEP_SPAN` per step of
/// `state::INTERFACE_DETACH_LIFECYCLE`), with margin for a loaded test machine.
const POLL_BUDGET: Duration = AWSVPC_START
    .saturating_add(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::INTERFACE_RELEASE_STEP_SPAN
            .saturating_mul(3),
    )
    .saturating_add(Duration::from_secs(6));

/// An emulator carrying ECS, Elastic Load Balancing v2, EventBridge and SQS over ONE shared
/// engine, the way the production router mounts them.
///
/// All four are needed because the propagation under test spans them: the target group a
/// service binds lives in the ELBv2 store, a task state change is published onto the
/// EventBridge bus through the in-process callback, and a rule on that bus delivers the
/// event to an SQS queue. Each service claims only requests scoped to its own signing name,
/// so no ECS request below is routed any differently for their presence.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    // The services and tasks below are placed in an `awsvpc` subnet, which Amazon ECS
    // resolves against Amazon VPC's records ([`crate::ecs_wire::seed_awsvpc_subnets`]).
    crate::ecs_wire::seed_awsvpc_subnets(&engine);
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        engine,
    ));
    router
}

/// One `awsJson` RPC against the service whose signing name is `signing_name`, as a client
/// makes it.
async fn rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: Value,
    case: &str,
) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{SIGNED_REGION}.amazonaws.com"))
        .header(
            "content-type",
            format!("application/x-amz-json-{json_version}"),
        )
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDECSTEST/20260101/{SIGNED_REGION}\
                 /{signing_name}/aws4_request, SignedHeaders=host;x-amz-target, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
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
        "case {case:?}: expected {target_prefix}.{operation} to answer 200, got {status} with \
         {parsed}"
    );
    parsed
}

/// One EventBridge call: `awsJson1_1`, `X-Amz-Target: AWSEvents.<Operation>`.
async fn events(router: &MultiServiceRouter, operation: &str, body: Value, case: &str) -> Value {
    rpc(router, "events", "AWSEvents", "1.1", operation, body, case).await
}

/// One SQS call: `awsJson1_0`, `X-Amz-Target: AmazonSQS.<Operation>`.
async fn sqs(router: &MultiServiceRouter, operation: &str, body: Value, case: &str) -> Value {
    rpc(router, "sqs", "AmazonSQS", "1.0", operation, body, case).await
}

/// An SQS queue called `name`, carrying the grant a rule's target delivery needs —
/// EventBridge makes the target's `sqs:SendMessage` as its own service principal and relies
/// on the queue's resource policy — and answers its url and the ARN a rule names it by.
async fn given_queue(router: &MultiServiceRouter, name: &str, case: &str) -> (String, String) {
    let created = sqs(router, "CreateQueue", json!({ "QueueName": name }), case).await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no QueueUrl in {created}"))
        .to_string();
    let attributes = sqs(
        router,
        "GetQueueAttributes",
        json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
        case,
    )
    .await;
    let arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no QueueArn in {attributes}"))
        .to_string();
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "events.amazonaws.com" },
            "Action": "sqs:SendMessage",
            "Resource": &arn,
        }],
    })
    .to_string();
    sqs(
        router,
        "SetQueueAttributes",
        json!({ "QueueUrl": url, "Attributes": { "Policy": policy } }),
        case,
    )
    .await;
    (url, arn)
}

/// An ENABLED rule called `name` whose `EventPattern` is `pattern`, delivering to a queue of
/// its own; answers that queue's url.
async fn given_rule(
    router: &MultiServiceRouter,
    name: &str,
    pattern: Value,
    case: &str,
) -> String {
    let (url, arn) = given_queue(router, &format!("{name}-queue"), case).await;
    events(
        router,
        "PutRule",
        json!({ "Name": name, "EventPattern": pattern.to_string(), "State": "ENABLED" }),
        case,
    )
    .await;
    events(
        router,
        "PutTargets",
        json!({ "Rule": name, "Targets": [{ "Id": "t1", "Arn": arn }] }),
        case,
    )
    .await;
    url
}

/// Every message waiting on the queue at `url`, as parsed JSON bodies.
async fn drain(router: &MultiServiceRouter, url: &str, case: &str) -> Vec<Value> {
    let received = sqs(
        router,
        "ReceiveMessage",
        json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
        case,
    )
    .await;
    received["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| {
            serde_json::from_str::<Value>(message["Body"].as_str().unwrap_or_default())
                .unwrap_or(Value::Null)
        })
        .collect()
}

/// Write one target group attribute through the ELBv2 API.
async fn set_target_group_attribute(
    router: &MultiServiceRouter,
    arn: &str,
    key: &str,
    value: &str,
    case: &str,
) {
    let (status, body) = elb_call(
        router,
        "ModifyTargetGroupAttributes",
        &[
            ("TargetGroupArn", arn),
            ("Attributes.member.1.Key", key),
            ("Attributes.member.1.Value", value),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ModifyTargetGroupAttributes must succeed, got {status} with {body}"
    );
}

/// `(target id, state)` for every registration the group reports, in the order reported.
///
/// One `TargetHealthDescription` carries exactly one `Target.Id` and one
/// `TargetHealth.State`, in that order, so the two leaf sequences pair up — which the length
/// check asserts rather than assumes.
async fn target_health(
    router: &MultiServiceRouter,
    group: &str,
    case: &str,
) -> Vec<(String, String)> {
    let (status, body) =
        elb_call(router, "DescribeTargetHealth", &[("TargetGroupArn", group)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeTargetHealth must succeed, got {status} with {body}"
    );
    let ids = all_texts(&body, "Id");
    // One state per DESCRIPTION, read inside that description's own health block: the
    // administrative override reported beside it carries a `State` of its own
    // ([`health_texts`]).
    let states = health_texts(&body, "State");
    assert_eq!(
        ids.len(),
        states.len(),
        "case {case:?}: expected one state per described target, got {ids:?} and {states:?} \
         from {body}"
    );
    ids.into_iter().zip(states).collect()
}

/// The state the group reports for `id`, or `None` when the group holds no registration of
/// it at all.
async fn state_of(
    router: &MultiServiceRouter,
    group: &str,
    id: &str,
    case: &str,
) -> Option<String> {
    target_health(router, group, case)
        .await
        .into_iter()
        .find(|(target, _)| target == id)
        .map(|(_, state)| state)
}

/// Poll the group until its registration of `id` reports one of `wanted`, and answer the
/// state it was last seen in — which the CALLER asserts, so a row that never reaches its
/// state fails with its own case and context rather than inside this helper. `None` in
/// `wanted` means "no registration at all", which is what a target that has left the group
/// reports.
async fn await_state(
    router: &MultiServiceRouter,
    group: &str,
    id: &str,
    wanted: &[Option<&str>],
    case: &str,
) -> Option<String> {
    let deadline = Instant::now() + POLL_BUDGET;
    loop {
        let seen = state_of(router, group, id, case).await;
        if wanted.contains(&seen.as_deref()) || Instant::now() >= deadline {
            return seen;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Register one Fargate-compatible `awsvpc` revision of `family` whose container `web`
/// exposes `container_port`.
async fn register_definition(
    router: &MultiServiceRouter,
    family: &str,
    container_port: i64,
    case: &str,
) {
    let mut web = container("web", "public.ecr.aws/nginx/nginx:latest");
    web["portMappings"] = json!([{ "containerPort": container_port, "protocol": "tcp" }]);
    ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
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

/// The `networkConfiguration` every service and task below launches with: the subnet the
/// harness holds in the account's Amazon VPC records, since a placement into the `awsvpc`
/// mode is resolved against them, and an EC2-shaped security group.
fn network_configuration() -> Value {
    json!({
        "awsvpcConfiguration": {
            "subnets": [crate::ecs_wire::AWSVPC_SUBNET],
            "securityGroups": ["sg-0123456789abcdef0"],
        }
    })
}

/// The service name every row creates, and the container the revision declares: one value
/// each, because neither is what any rule here generalises over.
const SERVICE: &str = "web";
const CONTAINER: &str = "web";

/// Create the service [`SERVICE`] in `cluster`, from the revision of the family named after
/// that cluster, bound to `group` on `container_port`, with `desired` tasks.
async fn create_service(
    router: &MultiServiceRouter,
    cluster: &str,
    group: &str,
    container_port: i64,
    desired: i64,
    case: &str,
) {
    ok(
        router,
        "CreateService",
        json!({
            "cluster": cluster,
            "serviceName": SERVICE,
            "taskDefinition": cluster,
            "desiredCount": desired,
            "launchType": "FARGATE",
            "networkConfiguration": network_configuration(),
            "loadBalancers": [{
                "targetGroupArn": group,
                "containerName": CONTAINER,
                "containerPort": container_port,
            }],
        }),
        case,
    )
    .await;
}

/// Poll until `cluster`'s service [`SERVICE`] has `expected` RUNNING tasks, and answer
/// `(taskArn, private IPv4 address)` for each, ordered by task ARN.
async fn running_tasks(
    router: &MultiServiceRouter,
    cluster: &str,
    expected: usize,
    case: &str,
) -> Vec<(String, String)> {
    let name = SERVICE;
    let deadline = Instant::now() + POLL_BUDGET;
    loop {
        let listed = ok(
            router,
            "ListTasks",
            json!({ "cluster": cluster, "serviceName": name }),
            case,
        )
        .await;
        let arns: Vec<Value> = list(&listed, "taskArns", case);
        let mut running: Vec<(String, String)> = Vec::new();
        if !arns.is_empty() {
            let described = ok(
                router,
                "DescribeTasks",
                json!({ "cluster": cluster, "tasks": arns }),
                case,
            )
            .await;
            for task in list(&described, "tasks", case) {
                if task["lastStatus"] != json!("RUNNING") {
                    continue;
                }
                let address = list(&task, "attachments", case)
                    .into_iter()
                    .flat_map(|attachment| list(&attachment, "details", case))
                    .find(|detail| detail["name"] == json!("privateIPv4Address"))
                    .and_then(|detail| detail["value"].as_str().map(str::to_string));
                if let Some(address) = address {
                    running.push((string(&task, "taskArn", case), address));
                }
            }
        }
        if running.len() >= expected {
            running.sort();
            return running;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected {expected} running task(s) of {name} with an interface \
             address within the poll budget, got {running:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// How a row stops the task it registered.
#[derive(Debug, Clone, Copy)]
enum HowItStops {
    /// `StopTask` on the task itself.
    StopTask,
    /// `UpdateService` taking the service's `desiredCount` to zero.
    ScaleToZero,
    /// `DeleteService`, which drains the whole service away.
    DeleteService,
}

/// A load-balanced service's running tasks are `ip` targets of the group its binding names,
/// and a task that stops drains out of it.
///
/// Each row stands up an emulator of its own and shares nothing with the others, so the rows
/// run CONCURRENTLY: every wait here is the clock settling a transition, and waiting them out
/// one after another would hold this binary — and the machine the whole suite runs on — for
/// four times as long as the rule needs.
#[tokio::test]
async fn a_load_balanced_services_tasks_are_ip_targets_that_drain_when_they_stop() {
    // The rows differ along what the rule generalises over: how many tasks the service
    // runs, the port the binding names against the group's own, the drain the group is
    // configured with, and WHICH call stops the task.
    tokio::join!(
        one_target_row(
            "one of two tasks stopped, a long drain",
            2,
            80,
            "60",
            HowItStops::StopTask,
        ),
        one_target_row(
            "a binding on a port other than the group's",
            1,
            8080,
            "60",
            HowItStops::StopTask,
        ),
        one_target_row(
            "scaled to zero with no drain configured",
            1,
            80,
            "0",
            HowItStops::ScaleToZero,
        ),
        one_target_row(
            "the whole service deleted, a long drain",
            1,
            80,
            "60",
            HowItStops::DeleteService,
        ),
    );
}

/// One row of the target table: a service of `desired` tasks bound to an `ip` group on
/// `container_port` with `delay` as its drain, stopped the way `stop` says.
async fn one_target_row(
    case: &str,
    desired: i64,
    container_port: i64,
    delay: &str,
    stop: HowItStops,
) {
    {
        let router = emulator();
        ok(
            &router,
            "CreateCluster",
            json!({ "clusterName": "lbtargets" }),
            case,
        )
        .await;
        register_definition(&router, "lbtargets", container_port, case).await;
        let group = create_target_group(&router, "lbtargets-tg", "ip", case).await;
        set_target_group_attribute(
            &router,
            &group,
            "deregistration_delay.timeout_seconds",
            delay,
            case,
        )
        .await;
        create_service(&router, "lbtargets", &group, container_port, desired, case).await;

        // Every running task is registered, at its own interface address, and comes up
        // `initial` before it is `healthy` — the registration's own settling, not something
        // this service invents.
        let wanted_tasks = usize::try_from(desired).unwrap_or(1);
        let tasks = running_tasks(&router, "lbtargets", wanted_tasks, case).await;
        for (task_arn, address) in &tasks {
            let state = await_state(
                &router,
                &group,
                address,
                &[Some("initial"), Some("healthy")],
                case,
            )
            .await;
            assert!(
                state.is_some(),
                "case {case:?}: expected the running task {task_arn} to be registered at \
                 {address}, got {:?}",
                target_health(&router, &group, case).await
            );
            let healthy = await_state(&router, &group, address, &[Some("healthy")], case).await;
            assert_eq!(
                healthy.as_deref(),
                Some("healthy"),
                "case {case:?}: expected the registration of {address} to become healthy"
            );
        }
        let registered = target_health(&router, &group, case).await;
        assert_eq!(
            registered.len(),
            tasks.len(),
            "case {case:?}: expected one registration per running task, got {registered:?}"
        );

        // A registration a CALLER made is this group's own and not the service's, so the
        // reconcile below must leave it exactly as it is.
        let (status, body) = elb_call(
            &router,
            "RegisterTargets",
            &[
                ("TargetGroupArn", &group),
                ("Targets.member.1.Id", "10.0.0.9"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: RegisterTargets must succeed, got {status} with {body}"
        );

        let (victim_arn, victim_address) = tasks[0].clone();
        match stop {
            HowItStops::StopTask => {
                ok(
                    &router,
                    "StopTask",
                    json!({ "cluster": "lbtargets", "task": &victim_arn, "reason": "drain it" }),
                    case,
                )
                .await;
            }
            HowItStops::ScaleToZero => {
                ok(
                    &router,
                    "UpdateService",
                    json!({ "cluster": "lbtargets", "service": SERVICE, "desiredCount": 0 }),
                    case,
                )
                .await;
            }
            HowItStops::DeleteService => {
                ok(
                    &router,
                    "DeleteService",
                    json!({ "cluster": "lbtargets", "service": SERVICE, "force": true }),
                    case,
                )
                .await;
            }
        }

        // A drain the group is configured for is observable; one it is not configured for
        // takes the target out at once. Either way the stopped task is no longer a target
        // the load balancer sends new requests to.
        let drains = delay != "0";
        let reached = await_state(
            &router,
            &group,
            &victim_address,
            if drains {
                &[Some("draining")]
            } else {
                &[None]
            },
            case,
        )
        .await;
        assert_eq!(
            reached.as_deref(),
            if drains { Some("draining") } else { None },
            "case {case:?}: unexpected state for the stopped task {victim_arn} at \
             {victim_address}"
        );

        // The tasks that did NOT stop keep serving, so a stop is per task and not per
        // service.
        for (task_arn, address) in tasks.iter().skip(1) {
            let state = state_of(&router, &group, address, case).await;
            assert_eq!(
                state.as_deref(),
                Some("healthy"),
                "case {case:?}: expected the surviving task {task_arn} to stay healthy at \
                 {address}"
            );
        }

        // And the caller's own registration is untouched by any of it: still registered and
        // never put in `draining`, whichever way the service's own task went.
        let callers = state_of(&router, &group, "10.0.0.9", case).await;
        assert!(
            matches!(callers.as_deref(), Some("initial") | Some("healthy")),
            "case {case:?}: a registration a caller made must survive the service's \
             reconcile, got {callers:?}"
        );
    }
}

/// Every status a task moves into is published as an `ECS Task State Change` event, reaches
/// the rule whose pattern matches it, and reaches no other rule — with no further ECS call
/// once the task has been stopped.
///
/// The two rows share nothing, so they run CONCURRENTLY for the reason the target table's do.
#[tokio::test]
async fn every_task_state_change_is_published_to_the_matching_rule_only() {
    // The rows differ along the kind of task and the call that stops it: a standalone task
    // of a `RunTask`, and a task of a load-balanced service scaled to zero.
    tokio::join!(
        one_event_row("a standalone task stopped by StopTask", false),
        one_event_row("a load-balanced service's task scaled to zero", true),
    );
}

/// One row of the event table: the whole lifecycle of a task, published and delivered.
async fn one_event_row(case: &str, through_a_service: bool) {
    {
        let router = emulator();
        let cluster_arn = string(
            &ok(
                &router,
                "CreateCluster",
                json!({ "clusterName": "taskevents" }),
                case,
            )
            .await["cluster"],
            "clusterArn",
            case,
        );
        // Three rules: every ECS task state change, only the `STOPPED` ones of THIS cluster
        // (the pattern a client that reacts to a task stopping writes), and one that cannot
        // match an ECS event at all.
        let every = given_rule(
            &router,
            "ecs-task-changes",
            json!({ "source": [ECS_EVENT_SOURCE], "detail-type": [TASK_STATE_CHANGE] }),
            case,
        )
        .await;
        let stopped_only = given_rule(
            &router,
            "ecs-task-stopped",
            json!({
                "source": [ECS_EVENT_SOURCE],
                "detail-type": [TASK_STATE_CHANGE],
                "detail": { "lastStatus": ["STOPPED"], "clusterArn": [&cluster_arn] },
            }),
            case,
        )
        .await;
        let unrelated = given_rule(
            &router,
            "other-service-events",
            json!({ "source": ["aws.batch"], "detail-type": ["Batch Job State Change"] }),
            case,
        )
        .await;

        register_definition(&router, "taskevents", 80, case).await;
        let task_arn = if through_a_service {
            let group = create_target_group(&router, "taskevents-tg", "ip", case).await;
            create_service(&router, "taskevents", &group, 80, 1, case).await;
            let tasks = running_tasks(&router, "taskevents", 1, case).await;
            ok(
                &router,
                "UpdateService",
                json!({ "cluster": "taskevents", "service": SERVICE, "desiredCount": 0 }),
                case,
            )
            .await;
            tasks[0].0.clone()
        } else {
            let run = ok(
                &router,
                "RunTask",
                json!({
                    "cluster": "taskevents",
                    "taskDefinition": "taskevents",
                    "launchType": "FARGATE",
                    "networkConfiguration": network_configuration(),
                }),
                case,
            )
            .await;
            let arn = string(&list(&run, "tasks", case)[0], "taskArn", case);
            // The task has to be RUNNING before it is stopped, so the row covers the whole
            // lifecycle rather than a task that never started.
            let deadline = Instant::now() + POLL_BUDGET;
            loop {
                let described = ok(
                    &router,
                    "DescribeTasks",
                    json!({ "cluster": "taskevents", "tasks": [&arn] }),
                    case,
                )
                .await;
                if list(&described, "tasks", case)[0]["lastStatus"] == json!("RUNNING") {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "case {case:?}: the task never reached RUNNING"
                );
                tokio::time::sleep(POLL_INTERVAL).await;
            }
            ok(
                &router,
                "StopTask",
                json!({ "cluster": "taskevents", "task": &arn, "reason": "end it" }),
                case,
            )
            .await;
            arn
        };

        // From here NOTHING calls ECS again: every message below is one the clock produced
        // and the pump published, which is what a client watching the event bus alone sees.
        let mut statuses: Vec<String> = Vec::new();
        let mut envelopes: Vec<Value> = Vec::new();
        let deadline = Instant::now() + POLL_BUDGET;
        let mut narrow: Vec<Value> = Vec::new();
        loop {
            for message in drain(&router, &every, case).await {
                if message["detail"]["taskArn"] == json!(&task_arn) {
                    statuses.push(
                        message["detail"]["lastStatus"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                    );
                    envelopes.push(message);
                }
            }
            // The narrow rule's queue is drained in the SAME loop, because a delivery is
            // observed when it arrives rather than at the instant the other rule's did.
            narrow.extend(drain(&router, &stopped_only, case).await);
            if statuses.iter().any(|status| status == "STOPPED") && !narrow.is_empty() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: expected the STOPPED change to be delivered to both rules, got \
                 {statuses:?} and {narrow:?}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }

        // Every status delivered is one the lifecycle passes through, each at most once and
        // in lifecycle order: the published stream is the task's own history, not a status
        // invented or repeated.
        let ranks: Vec<usize> = statuses
            .iter()
            .map(|status| {
                TASK_LIFECYCLE
                    .iter()
                    .position(|known| known == status)
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: {status:?} is no status of the task lifecycle")
                    })
            })
            .collect();
        assert!(
            ranks.windows(2).all(|pair| pair[0] < pair[1]),
            "case {case:?}: expected the statuses in lifecycle order with no repeats, got \
             {statuses:?}"
        );
        for required in ["RUNNING", "STOPPED"] {
            assert!(
                statuses.iter().any(|status| status == required),
                "case {case:?}: expected {required} among the delivered statuses, got \
                 {statuses:?}"
            );
        }

        // The envelope and the detail are what a rule filters on, and the detail's task is
        // this task in this cluster.
        for envelope in &envelopes {
            assert_eq!(
                envelope["source"],
                json!(ECS_EVENT_SOURCE),
                "case {case:?}: unexpected source in {envelope}"
            );
            assert_eq!(
                envelope["detail-type"],
                json!(TASK_STATE_CHANGE),
                "case {case:?}: unexpected detail-type in {envelope}"
            );
            assert_eq!(
                envelope["detail"]["clusterArn"],
                json!(&cluster_arn),
                "case {case:?}: unexpected clusterArn in {envelope}"
            );
            assert!(
                envelope["resources"]
                    .as_array()
                    .is_some_and(|resources| resources.contains(&json!(&task_arn))),
                "case {case:?}: expected the task's ARN in the envelope's resources, got \
                 {envelope}"
            );
        }

        // The narrow rule receives the STOPPED change and nothing else; the rule that
        // matches another service's events receives nothing at all.
        let narrow_statuses: Vec<String> = narrow
            .iter()
            .filter(|message| message["detail"]["taskArn"] == json!(&task_arn))
            .map(|message| {
                message["detail"]["lastStatus"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            narrow_statuses,
            vec!["STOPPED".to_string()],
            "case {case:?}: expected the lastStatus-filtered rule to receive the STOPPED \
             change alone, got {narrow:?}"
        );
        let withheld = drain(&router, &unrelated, case).await;
        assert!(
            withheld.is_empty(),
            "case {case:?}: a rule matching another service's events must receive nothing, \
             got {withheld:?}"
        );
    }
}
