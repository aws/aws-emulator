// SPDX-License-Identifier: CC0-1.0

//! Amazon ECS publishes an `ECS Task State Change` event every time a task moves.
//!
//! The RULE under test: every status a task moves into — the `lastStatus`/`desiredStatus`
//! pair it reports itself by — is published onto the account's DEFAULT EventBridge bus as
//! `source` `aws.ecs` / `detail-type` `ECS Task State Change`, whose `detail` carries the
//! task Amazon ECS holds at that instant (its `taskArn`, the `clusterArn` of the cluster it
//! runs in, its `taskDefinitionArn` and the status it moved into) and whose `resources`
//! names that task's ARN; EventBridge then routes it, so a rule whose `EventPattern`
//! matches delivers the envelope to that rule's targets and a rule that cannot match
//! receives nothing. Target-side authorization is evaluated at DELIVERY time: a queue
//! target whose own resource policy grants the EventBridge service principal
//! `sqs:SendMessage` receives the event, and one that grants nothing receives nothing while
//! `PutTargets` still succeeds. The transitions the clock owes a task are published as they
//! fall due, with no further Amazon ECS call: a client watching a task through EventBridge
//! makes none.
//!
//! The rows span what the rule generalises over: WHICH door moved the task (`RunTask`,
//! `StopTask`, a service's own scheduler), the network mode and launch type it was placed
//! with — and so WHICH statuses it passes through, an `awsvpc` task starting one step
//! earlier in the lifecycle than a task that shares its host's networking — and whether the
//! target queue has granted the delivery at all.
//!
//! EventBridge and Amazon SQS are mounted BESIDE Amazon ECS on the SAME engine, so a
//! publication travels the real in-process callback, through the real bus, to a real SQS
//! target: nothing here reads the publisher's own queue.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    awsvpc_network_configuration, list, ok, register_capacity, string, ACCOUNT, AWSVPC_START,
    SIGNED_REGION,
};
use crate::task_lifecycle::container;

/// The `source` and `detail-type` Amazon ECS publishes a task's state changes under, spelled
/// out here rather than imported, so a wrong constant in the implementation cannot make a
/// test pass. Shared with the suites that read those same publications, so the binary spells
/// each of them out once.
pub(crate) const EVENT_SOURCE: &str = "aws.ecs";
pub(crate) const TASK_STATE_CHANGE: &str = "ECS Task State Change";

/// The two task statuses those suites select the published envelopes by.
pub(crate) const RUNNING: &str = "RUNNING";
pub(crate) const STOPPED: &str = "STOPPED";

/// The rule that matches an ECS task event, and the rule that cannot.
const MATCHING_RULE: &str = "ecs-task-state-change";
const OTHER_RULE: &str = "unrelated-events";

/// The cluster every task below runs in.
const CLUSTER: &str = "eventing";

/// The image the containers start from; irrelevant to these rules, so one value.
const IMAGE: &str = "public.ecr.aws/nginx/nginx:latest";

/// How often a poll re-reads the target queue.
pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// How long a poll waiting on a STANDALONE task's whole run keeps trying before it gives up:
/// the start of an `awsvpc` task ([`AWSVPC_START`]), the work such a task was placed to do
/// (`state::CONTAINER_WORK_SPAN`), the stop that follows it and the delivery of the events it
/// earned, with generous margin for a loaded test machine.
pub(crate) const STANDALONE_RUN_POLL_BUDGET: Duration = AWSVPC_START
    .saturating_add(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::CONTAINER_WORK_SPAN,
    )
    .saturating_add(Duration::from_secs(30));

/// How long a poll keeps trying before it gives up: the whole start of an `awsvpc` task
/// ([`AWSVPC_START`]), the work a standalone task is placed to do
/// (`state::CONTAINER_WORK_SPAN`) and the stop that follows it, which also waits for the
/// release of the interface such a task was allocated (one
/// `state::INTERFACE_RELEASE_STEP_SPAN` per step of `state::INTERFACE_DETACH_LIFECYCLE`),
/// with margin for a loaded test machine.
const POLL_BUDGET: Duration = AWSVPC_START
    .saturating_add(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::CONTAINER_WORK_SPAN,
    )
    .saturating_add(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::INTERFACE_RELEASE_STEP_SPAN
            .saturating_mul(3),
    )
    .saturating_add(Duration::from_secs(5));

/// How long a row waits before asserting that NOTHING was delivered: long enough that a
/// delivery that was going to happen would have.
const QUIET_BUDGET: Duration = AWSVPC_START.saturating_add(Duration::from_secs(1));

/// A router carrying Amazon ECS, EventBridge and Amazon SQS over ONE shared engine — the
/// three services a published task event travels through, mounted the way the production
/// router mounts them.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    // The tasks of this suite are placed in an `awsvpc` subnet, which a placement is
    // resolved against ([`crate::ecs_wire::seed_awsvpc_subnets`]).
    crate::ecs_wire::seed_awsvpc_subnets(&engine);
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::EmulatedService::new(
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

/// One `awsJson` RPC call against a service mounted beside Amazon ECS, the way a client
/// makes it: `POST /` with `X-Amz-Target: <target_prefix>.<operation>` and a credential
/// scoped to `signing_name`, which is what the router picks the service from.
async fn rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: Value,
) -> (u16, Value) {
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
        .body(bytes::Bytes::from(
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

/// One EventBridge call.
pub(crate) async fn events(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    rpc(router, "events", "AWSEvents", "1.1", operation, body).await
}

/// One Amazon SQS call.
pub(crate) async fn sqs(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    rpc(router, "sqs", "AmazonSQS", "1.0", operation, body).await
}

/// An SQS queue called `name`, created through Amazon SQS itself; answers its url and the
/// ARN a rule names it by.
pub(crate) async fn given_queue(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
) -> (String, String) {
    let (status, body) = sqs(router, "CreateQueue", json!({ "QueueName": name })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateQueue answered {status} {body}"
    );
    let url = body["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no QueueUrl in {body}"))
        .to_string();
    let (status, body) = sqs(
        router,
        "GetQueueAttributes",
        json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetQueueAttributes answered {status} {body}"
    );
    let queue_arn = body["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no QueueArn in {body}"))
        .to_string();
    (url, queue_arn)
}

/// Grant `events.amazonaws.com` the `sqs:SendMessage` a rule's target needs on the queue at
/// `url` — EventBridge makes that call as its own service principal and relies on the
/// queue's resource-based policy for it, so a queue that has granted it nothing receives
/// nothing.
pub(crate) async fn grant_delivery(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    queue_arn: &str,
) {
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "events.amazonaws.com" },
            "Action": "sqs:SendMessage",
            "Resource": queue_arn,
        }],
    })
    .to_string();
    let (status, body) = sqs(
        router,
        "SetQueueAttributes",
        json!({ "QueueUrl": url, "Attributes": { "Policy": policy } }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: SetQueueAttributes answered {status} {body}"
    );
}

/// Put the rule `name` with `pattern` and point it at the SQS queue `queue_arn`.
pub(crate) async fn given_rule(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    pattern: Value,
    queue_arn: &str,
) {
    let (status, body) = events(
        router,
        "PutRule",
        json!({ "Name": name, "EventPattern": pattern.to_string(), "State": "ENABLED" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}/{name}: PutRule answered {status} {body}"
    );
    // `PutTargets` succeeds whatever the target queue has granted: authorization is a
    // DELIVERY-time question, not a wiring-time one.
    let (status, body) = events(
        router,
        "PutTargets",
        json!({ "Rule": name, "Targets": [{ "Id": "sqs-target", "Arn": queue_arn }] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}/{name}: PutTargets answered {status} {body}"
    );
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "case {case:?}/{name}: PutTargets reported a failed entry: {body}"
    );
}

/// Every message waiting on the queue at `url` right now, as parsed event envelopes.
pub(crate) async fn drain(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<Value> {
    let (status, body) = sqs(
        router,
        "ReceiveMessage",
        json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ReceiveMessage answered {status} {body}"
    );
    body["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| {
            let raw = message["Body"].as_str().unwrap_or_default();
            serde_json::from_str::<Value>(raw).unwrap_or(Value::Null)
        })
        .collect()
}

/// Poll the queue at `url` — and ONLY the queue — until it has delivered an event for
/// `task_arn` carrying each of `statuses`, or the budget runs out.
///
/// Nothing in this loop calls Amazon ECS, which is the point: a client that watches a task
/// through EventBridge makes no further call to the service that publishes.
async fn collect_events(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    task_arn: &str,
    statuses: &[&str],
) -> Vec<Value> {
    let deadline = Instant::now() + POLL_BUDGET;
    let mut collected: Vec<Value> = Vec::new();
    loop {
        collected.extend(
            drain(router, case, url)
                .await
                .into_iter()
                .filter(|event| event["detail"]["taskArn"] == json!(task_arn)),
        );
        let reached = statuses.iter().all(|status| {
            collected
                .iter()
                .any(|event| event["detail"]["lastStatus"] == json!(status))
        });
        if reached || Instant::now() >= deadline {
            return collected;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The `lastStatus` of every collected event, in the order the events arrived.
fn statuses_of(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .map(|event| {
            event["detail"]["lastStatus"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The pattern the matching rule carries: the source and detail type Amazon ECS publishes a
/// task's moves under, narrowed to ONE cluster through the event's own `detail`.
fn task_event_pattern(cluster_arn: &str) -> Value {
    json!({
        "source": [EVENT_SOURCE],
        "detail-type": [TASK_STATE_CHANGE],
        "detail": { "clusterArn": [cluster_arn] },
    })
}

/// The ARN of the cluster these suites run their tasks in.
fn cluster_arn() -> String {
    cluster_arn_of(CLUSTER)
}

/// The ARN of the cluster `cluster` names, in the Region these suites sign for.
pub(crate) fn cluster_arn_of(cluster: &str) -> String {
    format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:cluster/{cluster}")
}

/// The pattern of a rule scoped to the tasks of ONE cluster that STOPPED with a container
/// that exited `exit_code`: the member it constrains sits inside the event's `containers`
/// array.
pub(crate) fn exit_code_pattern(cluster_arn: &str, exit_code: i64) -> Value {
    json!({
        "source": [EVENT_SOURCE],
        "detail-type": [TASK_STATE_CHANGE],
        "detail": {
            "clusterArn": [cluster_arn],
            "lastStatus": [STOPPED],
            "containers": { "exitCode": [exit_code] },
        },
    })
}

/// The envelopes among `events` that are about the task `task_arn`.
pub(crate) fn events_of<'a>(events: &'a [Value], task_arn: &str) -> Vec<&'a Value> {
    events
        .iter()
        .filter(|event| event["detail"]["taskArn"] == json!(task_arn))
        .collect()
}

/// Register one `awsvpc` FARGATE revision of `family` made of `containers`, and place ONE
/// task of it in `cluster`. Answers the task's ARN.
pub(crate) async fn place_fargate_task(
    router: &MultiServiceRouter,
    cluster: &str,
    family: &str,
    containers: Vec<Value>,
    case: &str,
) -> String {
    ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
            "networkMode": "awsvpc",
            "requiresCompatibilities": ["FARGATE"],
            // The documented Fargate task size the recorded check uses: 256 CPU units, 512 MiB.
            "cpu": "256",
            "memory": "512",
            "containerDefinitions": containers,
        }),
        case,
    )
    .await;
    let placed = ok(
        router,
        "RunTask",
        json!({
            "cluster": cluster,
            "taskDefinition": family,
            "launchType": "FARGATE",
            "count": 1,
            "networkConfiguration": awsvpc_network_configuration(),
        }),
        case,
    )
    .await;
    let tasks = list(&placed, "tasks", case);
    assert_eq!(
        tasks.len(),
        1,
        "case {case:?}: expected RunTask to place one task, got {placed}"
    );
    string(&tasks[0], "taskArn", case)
}

/// Register one revision of `family` in `network_mode` with a single container, plus the
/// Fargate task size such a revision needs when it is `FARGATE`-compatible.
async fn register(
    router: &MultiServiceRouter,
    family: &str,
    network_mode: &str,
    fargate: bool,
    case: &str,
) {
    register_running(router, family, network_mode, fargate, None, case).await;
}

/// The same, with `program` as the container's own command — a program run through a shell,
/// and none at all for `None`.
///
/// A row that reads a task stopping ON ITS OWN needs a container that EXITS: a task is
/// stopped as one whose essential container exited only once that container really exits
/// (`Container$exitCode` is "the exit code returned from the container").
async fn register_running(
    router: &MultiServiceRouter,
    family: &str,
    network_mode: &str,
    fargate: bool,
    program: Option<&str>,
    case: &str,
) {
    let mut web = container("web", IMAGE);
    if let Some(program) = program {
        web["command"] = json!(["sh", "-c", program]);
    }
    let mut body = json!({
        "family": family,
        "networkMode": network_mode,
        "containerDefinitions": [web],
    });
    if fargate {
        body["requiresCompatibilities"] = json!(["FARGATE"]);
        // The documented Fargate task size the recorded check uses: 256 CPU units, 512 MiB.
        body["cpu"] = json!("256");
        body["memory"] = json!("512");
    }
    ok(router, "RegisterTaskDefinition", body, case).await;
}

/// Which door places the task a row is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Placement {
    /// `RunTask`, the door the recorded check uses.
    RunTask,
    /// A service's own scheduler, driven by `CreateService`.
    Service,
}

/// One row of the table.
struct Row {
    case: &'static str,
    /// Which door places the task.
    placement: Placement,
    /// `RegisterTaskDefinition$networkMode` the revision is registered in.
    network_mode: &'static str,
    /// The launch type the placement names.
    launch_type: &'static str,
    /// Whether the caller stops the task once it is up.
    stop: bool,
    /// Whether the target queue grants EventBridge the delivery it needs.
    granted: bool,
    /// The statuses this row's task must be published in. Extra events (the statuses the
    /// clock takes it through on the way) are fine: the rule is that every move is
    /// published, not that only these are.
    statuses: &'static [&'static str],
}

/// The table spans the rule: which door moved the task, the network mode and launch type it
/// was placed with — and so the statuses it passes through — and whether the target queue
/// has granted the delivery.
const ROWS: &[Row] = &[
    Row {
        case: "an EC2 task that shares its host's networking",
        placement: Placement::RunTask,
        network_mode: "bridge",
        launch_type: "EC2",
        stop: false,
        granted: true,
        statuses: &["PENDING", "RUNNING"],
    },
    Row {
        case: "a FARGATE task allocated its own elastic network interface",
        placement: Placement::RunTask,
        network_mode: "awsvpc",
        launch_type: "FARGATE",
        stop: false,
        granted: true,
        statuses: &["PROVISIONING", "PENDING", "RUNNING"],
    },
    Row {
        case: "a task the caller stops",
        placement: Placement::RunTask,
        network_mode: "bridge",
        launch_type: "EC2",
        stop: true,
        granted: true,
        statuses: &["DEACTIVATING", "STOPPED"],
    },
    Row {
        case: "a task a service's own scheduler placed",
        placement: Placement::Service,
        network_mode: "bridge",
        launch_type: "EC2",
        stop: false,
        granted: true,
        statuses: &["PENDING", "RUNNING"],
    },
    Row {
        case: "a target queue that granted no delivery",
        placement: Placement::RunTask,
        network_mode: "bridge",
        launch_type: "EC2",
        stop: false,
        granted: false,
        statuses: &[],
    },
];

#[tokio::test]
async fn a_task_that_moves_publishes_a_task_state_change_event() {
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        // A fresh emulator per row, so one row's tasks and policies cannot decide another's.
        let router = emulator();

        let (matching_url, matching_arn) = given_queue(&router, case, "ecs-events").await;
        let (other_url, other_arn) = given_queue(&router, case, "unrelated-events").await;
        given_rule(
            &router,
            case,
            MATCHING_RULE,
            task_event_pattern(&cluster_arn()),
            &matching_arn,
        )
        .await;
        given_rule(
            &router,
            case,
            OTHER_RULE,
            // Another cluster's task events: nothing this row publishes can match it.
            task_event_pattern(&format!(
                "arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:cluster/elsewhere"
            )),
            &other_arn,
        )
        .await;
        if row.granted {
            grant_delivery(&router, case, &matching_url, &matching_arn).await;
        }
        grant_delivery(&router, case, &other_url, &other_arn).await;

        ok(
            &router,
            "CreateCluster",
            json!({ "clusterName": CLUSTER }),
            case,
        )
        .await;
        // The EC2 launch type "runs your tasks on Amazon EC2 instances registered to your
        // cluster", so such a placement needs capacity; a Fargate one needs none.
        if row.launch_type == "EC2" {
            register_capacity(&router, CLUSTER, case).await;
        }
        let family = format!("taskevents{index}");
        register(
            &router,
            &family,
            row.network_mode,
            row.launch_type == "FARGATE",
            case,
        )
        .await;

        let task_arn = place(&router, row, &family, case).await;

        if row.stop {
            // Past one span, so the start the clock owed this task has fallen due and the
            // stop below is a stop of a task that RAN.
            tokio::time::sleep(AWSVPC_START).await;
            let stopped = ok(
                &router,
                "StopTask",
                json!({ "cluster": CLUSTER, "task": task_arn, "reason": "the suite asked" }),
                case,
            )
            .await;
            assert_eq!(
                string(&stopped["task"], "desiredStatus", case),
                "STOPPED",
                "case {case:?}: expected StopTask to ask for STOPPED, got {stopped}"
            );
        }

        let delivered = collect_events(&router, case, &matching_url, &task_arn, row.statuses).await;

        if !row.granted {
            // Nothing was delivered, and `PutTargets` still succeeded: the queue's own
            // policy is what withheld it.
            tokio::time::sleep(QUIET_BUDGET).await;
            let late = drain(&router, case, &matching_url).await;
            assert!(
                delivered.is_empty() && late.is_empty(),
                "case {case:?}: a queue that granted nothing must receive nothing, got \
                 {delivered:?} then {late:?}"
            );
            continue;
        }

        let observed = statuses_of(&delivered);
        for status in row.statuses {
            assert!(
                observed.iter().any(|seen| seen == status),
                "case {case:?}: expected an event reporting {status} for task {task_arn}, got \
                 {observed:?}"
            );
        }

        // Every delivered envelope is an ECS task state change about THIS task, in THIS
        // cluster, and names the task in its `resources`.
        for event in &delivered {
            assert_eq!(
                event["source"],
                json!(EVENT_SOURCE),
                "case {case:?}: expected source {EVENT_SOURCE}, got {event}"
            );
            assert_eq!(
                event["detail-type"],
                json!(TASK_STATE_CHANGE),
                "case {case:?}: expected detail-type {TASK_STATE_CHANGE}, got {event}"
            );
            assert_eq!(
                event["detail"]["clusterArn"],
                json!(cluster_arn()),
                "case {case:?}: expected detail.clusterArn {}, got {event}",
                cluster_arn()
            );
            assert_eq!(
                event["resources"],
                json!([task_arn.clone()]),
                "case {case:?}: expected the task's ARN in resources, got {event}"
            );
        }

        // The LAST event about the task reports the task the control plane reports: the
        // event is the task's own state and not a second copy of it.
        let task = describe(&router, &task_arn, case).await;
        let last = delivered
            .last()
            .unwrap_or_else(|| panic!("case {case:?}: expected at least one delivered event"));
        assert_eq!(
            last["detail"]["taskDefinitionArn"],
            task["taskDefinitionArn"].clone(),
            "case {case:?}: the event's taskDefinitionArn must be the task's, got {last} and \
             {task}"
        );
        assert_eq!(
            last["detail"]["lastStatus"],
            task["lastStatus"].clone(),
            "case {case:?}: the event's lastStatus must be the task's, got {last} and {task}"
        );
        assert_eq!(
            last["detail"]["desiredStatus"],
            task["desiredStatus"].clone(),
            "case {case:?}: the event's desiredStatus must be the task's, got {last} and {task}"
        );

        // The rule that cannot match this cluster's events never receives one.
        let unrelated = drain(&router, case, &other_url).await;
        assert!(
            unrelated.is_empty(),
            "case {case:?}: the non-matching rule's target must receive nothing, got \
             {unrelated:?}"
        );
    }
}

/// Place one task the way `row` asks and answer its ARN.
async fn place(router: &MultiServiceRouter, row: &Row, family: &str, case: &str) -> String {
    let mut request = json!({
        "cluster": CLUSTER,
        "taskDefinition": family,
        "launchType": row.launch_type,
    });
    if row.network_mode == "awsvpc" {
        request["networkConfiguration"] = awsvpc_network_configuration();
    }
    match row.placement {
        Placement::RunTask => {
            request["count"] = json!(1);
            let body = ok(router, "RunTask", request, case).await;
            let tasks = list(&body, "tasks", case);
            assert_eq!(
                tasks.len(),
                1,
                "case {case:?}: expected RunTask to place one task, got {body}"
            );
            string(&tasks[0], "taskArn", case)
        }
        Placement::Service => {
            request["serviceName"] = json!(format!("svc-{family}"));
            request["desiredCount"] = json!(1);
            ok(router, "CreateService", request, case).await;
            let listed = ok(router, "ListTasks", json!({ "cluster": CLUSTER }), case).await;
            let arns = list(&listed, "taskArns", case);
            assert_eq!(
                arns.len(),
                1,
                "case {case:?}: expected the service's one task, got {listed}"
            );
            arns[0]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a task ARN, got {listed}"))
                .to_string()
        }
    }
}

/// Read ONE task back through `DescribeTasks` — the stored record, not the answer of the
/// call that created it.
async fn describe(router: &MultiServiceRouter, task_arn: &str, case: &str) -> Value {
    describe_task_in(router, CLUSTER, task_arn, case).await
}

/// The same read, of a task in the cluster `cluster` names.
pub(crate) async fn describe_task_in(
    router: &MultiServiceRouter,
    cluster: &str,
    task_arn: &str,
    case: &str,
) -> Value {
    let body = ok(
        router,
        "DescribeTasks",
        json!({ "cluster": cluster, "tasks": [task_arn] }),
        case,
    )
    .await;
    list(&body, "tasks", case)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: expected one task back, got {body}"))
}

/// The whole lifecycle reaches the target with NO further Amazon ECS call: the start the
/// clock owed the task, and then the stop, are published as they fall due — which is what a
/// client reacting to a task through EventBridge depends on.
#[tokio::test]
async fn the_clock_owed_transitions_are_published_without_another_ecs_call() {
    let case = "a task watched only through EventBridge";
    let router = emulator();

    let (url, queue_arn) = given_queue(&router, case, "ecs-events").await;
    given_rule(
        &router,
        case,
        MATCHING_RULE,
        task_event_pattern(&cluster_arn()),
        &queue_arn,
    )
    .await;
    grant_delivery(&router, case, &url, &queue_arn).await;

    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": CLUSTER }),
        case,
    )
    .await;
    register_capacity(&router, CLUSTER, case).await;
    register(&router, "watched", "bridge", false, case).await;

    let placed = ok(
        &router,
        "RunTask",
        json!({
            "cluster": CLUSTER,
            "taskDefinition": "watched",
            "launchType": "EC2",
            "count": 1,
        }),
        case,
    )
    .await;
    let task_arn = string(&list(&placed, "tasks", case)[0], "taskArn", case);

    // ONLY the queue is polled from here: the `RUNNING` the clock owes this task has to
    // arrive without anybody calling Amazon ECS again.
    let coming_up = collect_events(&router, case, &url, &task_arn, &["PENDING", "RUNNING"]).await;
    let observed = statuses_of(&coming_up);
    assert_eq!(
        observed,
        vec!["PENDING".to_string(), "RUNNING".to_string()],
        "case {case:?}: expected the task's placement and start to be published in order, got \
         {observed:?}"
    );

    let stopped = ok(
        &router,
        "StopTask",
        json!({ "cluster": CLUSTER, "task": task_arn, "reason": "the suite asked" }),
        case,
    )
    .await;
    assert_eq!(
        string(&stopped["task"], "lastStatus", case),
        "DEACTIVATING",
        "case {case:?}: expected StopTask to answer a task on its way out, got {stopped}"
    );

    // And again: the `STOPPED` the clock owes the stop arrives with no further call.
    let going_down =
        collect_events(&router, case, &url, &task_arn, &["DEACTIVATING", "STOPPED"]).await;
    let observed = statuses_of(&going_down);
    assert_eq!(
        observed,
        vec!["DEACTIVATING".to_string(), "STOPPED".to_string()],
        "case {case:?}: expected the stop and the stopped task to be published in order, got \
         {observed:?}"
    );
    // The stop the event reports is the stop the control plane reports.
    let task = describe(&router, &task_arn, case).await;
    let last = going_down
        .last()
        .unwrap_or_else(|| panic!("case {case:?}: expected a STOPPED event"));
    assert_eq!(
        last["detail"]["stopCode"],
        task["stopCode"].clone(),
        "case {case:?}: the event's stopCode must be the task's, got {last} and {task}"
    );
    assert_eq!(
        last["detail"]["stoppedReason"],
        task["stoppedReason"].clone(),
        "case {case:?}: the event's stoppedReason must be the task's, got {last} and {task}"
    );
}

/// A task that is READ repeatedly publishes each of its moves once: the event stream is the
/// task's MOVES, not the reads of it, so polling a settled task does not keep announcing it.
#[tokio::test]
async fn a_transition_is_published_once_however_often_the_task_is_read() {
    let case = "repeated reads of one running task";
    let router = emulator();

    let (url, queue_arn) = given_queue(&router, case, "ecs-events").await;
    given_rule(
        &router,
        case,
        MATCHING_RULE,
        task_event_pattern(&cluster_arn()),
        &queue_arn,
    )
    .await;
    grant_delivery(&router, case, &url, &queue_arn).await;

    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": CLUSTER }),
        case,
    )
    .await;
    register_capacity(&router, CLUSTER, case).await;
    // The task of this row ends ON ITS OWN, which is what makes its run four moves: its
    // container runs a program that exits, and that exit is what stops the task.
    register_running(&router, "polled", "bridge", false, Some("exit 0"), case).await;

    let placed = ok(
        &router,
        "RunTask",
        json!({
            "cluster": CLUSTER,
            "taskDefinition": "polled",
            "launchType": "EC2",
            "count": 1,
        }),
        case,
    )
    .await;
    let task_arn = string(&list(&placed, "tasks", case)[0], "taskArn", case);

    // Up and running — waiting for the CONDITION rather than for a span — and then read
    // through three doors. The reads are what this rule is about: none of them may announce
    // a move again.
    let deadline = Instant::now() + POLL_BUDGET;
    loop {
        let task = describe(&router, &task_arn, case).await;
        let status = string(&task, "lastStatus", case);
        if status == "RUNNING" || status == "DEACTIVATING" || status == "STOPPED" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the task to start within the poll budget, got {task}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    for operation in ["DescribeTasks", "DescribeTasks", "ListTasks"] {
        let body = if operation == "ListTasks" {
            json!({ "cluster": CLUSTER })
        } else {
            json!({ "cluster": CLUSTER, "tasks": [task_arn.clone()] })
        };
        ok(&router, operation, body, case).await;
    }

    // A task placed on its own ends when the essential container it was placed to run has
    // exited, so its WHOLE run is four moves: it comes up PENDING then RUNNING, and stops
    // DEACTIVATING then STOPPED. Collecting until the last of them arrives is what makes the
    // exact sequence assertable — the rule being that each move is published once, however
    // often the task was READ in between.
    let delivered = collect_events(
        &router,
        case,
        &url,
        &task_arn,
        &["PENDING", "RUNNING", "DEACTIVATING", "STOPPED"],
    )
    .await;
    // One more quiet stretch, LOOKED at over and over rather than slept through: a read that
    // queued a second announcement of a move would be found by one of these drains.
    let quiet_until = Instant::now() + QUIET_BUDGET;
    let mut late: Vec<Value> = Vec::new();
    while Instant::now() < quiet_until {
        tokio::time::sleep(POLL_INTERVAL).await;
        late.extend(
            drain(&router, case, &url)
                .await
                .into_iter()
                .filter(|event| event["detail"]["taskArn"] == json!(task_arn)),
        );
    }
    let observed = statuses_of(&delivered);
    let late_observed = statuses_of(&late);
    assert_eq!(
        observed,
        vec![
            "PENDING".to_string(),
            "RUNNING".to_string(),
            "DEACTIVATING".to_string(),
            "STOPPED".to_string(),
        ],
        "case {case:?}: expected the task's whole run published in order, once each, got \
         {observed:?} and then {late_observed:?}"
    );
    assert!(
        late.is_empty(),
        "case {case:?}: reading a settled task must publish nothing more, got {late_observed:?}"
    );
}
