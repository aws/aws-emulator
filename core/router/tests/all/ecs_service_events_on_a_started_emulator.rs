// SPDX-License-Identifier: CC0-1.0

//! The SERVICE-level events Amazon ECS publishes reach the Amazon SQS targets of their rules on
//! the emulator a client STARTS: the built binary, on its own port, with request authentication
//! on, reached over TLS with SigV4-signed calls.
//!
//! The RULE under test: **every action the Amazon ECS service scheduler takes is published once
//! onto the account's default event bus as `source` `aws.ecs` / `detail-type` `ECS Service
//! Action`, with the envelope's `resources` naming ONLY the service it is about, and an action
//! the scheduler took because it could NOT do what the service asks carrying in `detail.reason`
//! the sentence the service's own records hold for that failure; EventBridge routes each
//! envelope by its content, so a rule scoped by `resources` to one service receives that
//! service's actions and never a sibling's.**
//!
//! [`crate::ecs_service_events_reach_their_per_service_queues`] pins that rule over a router
//! assembled inside the test process, on a runtime the test chose. The emulator a client runs is
//! a different configuration of the same code: the publication Amazon ECS makes is signed with
//! the credentials THAT process vends, every transition the clock owes is materialised from a
//! task of its own on the binary's own multi-thread runtime, EventBridge's delivery is
//! authorized by the queue's own policy, and one process serves every call. This suite is the
//! one that pins the chain THERE, so an action that is delivered in-process and not on the
//! started emulator is a failure a test reports rather than one only a client sees.
//!
//! The two services span what the rule generalises over: one whose revision names an image tag
//! no registry serves, whose every launch the scheduler loses, and one on a release the public
//! gallery publishes, whose rollout reaches a steady state — so the queue that receives no
//! failure is one whose rule WITHHELD it rather than a bus that published nothing.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::cfn_deployed_pipeline_on_a_started_emulator::Started;
use crate::query_wire::element;

/// The Region the started emulator's calls are signed for. It must stay the one [`Started`]
/// signs with.
const REGION: &str = "us-east-1";

/// The zone the subnet of this suite sits in.
const ZONE: &str = "us-east-1a";

/// The account every emulator ARN carries.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// What the envelope says about itself, spelled out here rather than imported from the service
/// crate, so a wrong constant in the implementation cannot make this suite pass.
const EVENT_SOURCE: &str = "aws.ecs";
const SERVICE_ACTION: &str = "ECS Service Action";
const DEPLOYMENT_STATE_CHANGE: &str = "ECS Deployment State Change";
const TASK_CONFIGURATION_FAILURE: &str = "SERVICE_TASK_CONFIGURATION_FAILURE";
const STEADY_STATE: &str = "SERVICE_STEADY_STATE";
const DEPLOYMENT_IN_PROGRESS: &str = "SERVICE_DEPLOYMENT_IN_PROGRESS";
const DEPLOYMENT_COMPLETED: &str = "SERVICE_DEPLOYMENT_COMPLETED";
const DEPLOYMENT_FAILED: &str = "SERVICE_DEPLOYMENT_FAILED";
const INFO: &str = "INFO";
const ERROR: &str = "ERROR";

/// `Deployment$rolloutState` values a release passes through.
const IN_PROGRESS: &str = "IN_PROGRESS";
const COMPLETED: &str = "COMPLETED";
const FAILED: &str = "FAILED";

/// The error class a container runtime reports an image it could not fetch under, which is the
/// fixed text such a failure's `reason` begins with.
const CANNOT_PULL: &str = "CannotPullContainerError";

/// The service principal EventBridge delivers to a queue target as, which is what the queue's
/// own resource policy has to allow.
const EVENTS_PRINCIPAL: &str = "events.amazonaws.com";

/// Amazon EC2's form-wire API version, and Amazon ECS's `awsJson1_1` target prefix.
const EC2_API_VERSION: &str = "2016-11-15";
const ECS_TARGET_PREFIX: &str = "AmazonEC2ContainerServiceV20141113";

/// The cluster both services run in.
const CLUSTER: &str = "startedactions";

/// An image the public gallery this emulator serves PUBLISHES a release of, and a reference into
/// the same repository naming a tag it publishes no release under.
const PULLABLE_IMAGE: &str = "public.ecr.aws/docker/library/busybox:latest";
const UNPULLABLE_IMAGE: &str = "public.ecr.aws/docker/library/busybox:no-such-release";

/// How long the suite waits for a delivery, and how often it looks meanwhile.
///
/// Generous on purpose: every wait is on a CONDITION, looked for again until it holds, so a fast
/// machine never waits the whole span and a loaded one is given room to spend — and still far
/// inside the span a suite may take, because the whole chain settles in seconds.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(50);
const LOOK_INTERVAL: Duration = Duration::from_millis(200);

/// One `X-Amz-Target` RPC call over the started emulator's signed-call plumbing, which must
/// answer 200.
fn rpc_ok(
    emulator: &Started,
    case: &str,
    signing_name: &str,
    target: &str,
    content_type: &str,
    body: &Value,
) -> Value {
    let (status, text) = emulator.rpc(signing_name, target, content_type, body);
    let parsed: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {target}, got {status} with {parsed}; the \
         log:\n{}",
        emulator.log()
    );
    parsed
}

/// One Amazon ECS call that must answer 200.
fn ecs(emulator: &Started, case: &str, operation: &str, body: &Value) -> Value {
    rpc_ok(
        emulator,
        case,
        "ecs",
        &format!("{ECS_TARGET_PREFIX}.{operation}"),
        "application/x-amz-json-1.1",
        body,
    )
}

/// One EventBridge call that must answer 200.
fn events(emulator: &Started, case: &str, operation: &str, body: &Value) -> Value {
    rpc_ok(
        emulator,
        case,
        "events",
        &format!("AWSEvents.{operation}"),
        "application/x-amz-json-1.1",
        body,
    )
}

/// One Amazon SQS call that must answer 200.
fn sqs(emulator: &Started, case: &str, operation: &str, body: &Value) -> Value {
    rpc_ok(
        emulator,
        case,
        "sqs",
        &format!("AmazonSQS.{operation}"),
        "application/x-amz-json-1.0",
        body,
    )
}

/// One Amazon EC2 form-wire call that must answer 200, answered with its XML body.
fn ec2(emulator: &Started, case: &str, action: &str, params: &[(&str, &str)]) -> String {
    let (status, body) = emulator.form_call("ec2", EC2_API_VERSION, action, params);
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from EC2 {action}, got {status} with {body}; the \
         log:\n{}",
        emulator.log()
    );
    body
}

/// A VPC with one PUBLIC subnet, built through Amazon EC2's own API: an internet gateway attached
/// to it and a default route to that gateway, which is the path a task given a public address
/// fetches a public registry's image over.
fn public_subnet(emulator: &Started, case: &str) -> String {
    let vpc = element(
        &ec2(emulator, case, "CreateVpc", &[("CidrBlock", "10.74.0.0/16")]),
        "vpcId",
    );
    let subnet = element(
        &ec2(
            emulator,
            case,
            "CreateSubnet",
            &[
                ("VpcId", &vpc),
                ("CidrBlock", "10.74.1.0/24"),
                ("AvailabilityZone", ZONE),
            ],
        ),
        "subnetId",
    );
    let gateway = element(
        &ec2(emulator, case, "CreateInternetGateway", &[]),
        "internetGatewayId",
    );
    ec2(
        emulator,
        case,
        "AttachInternetGateway",
        &[("InternetGatewayId", &gateway), ("VpcId", &vpc)],
    );
    let table = element(
        &ec2(
            emulator,
            case,
            "DescribeRouteTables",
            &[("Filter.1.Name", "vpc-id"), ("Filter.1.Value.1", &vpc)],
        ),
        "routeTableId",
    );
    ec2(
        emulator,
        case,
        "CreateRoute",
        &[
            ("RouteTableId", &table),
            ("DestinationCidrBlock", "0.0.0.0/0"),
            ("GatewayId", &gateway),
        ],
    );
    subnet
}

/// The ARN Amazon ECS mints for the service `name`, which a rule's `resources` clause scopes to.
fn service_arn(name: &str) -> String {
    format!("arn:aws:ecs:{REGION}:{ACCOUNT}:service/{CLUSTER}/{name}")
}

/// A queue behind the rule `rule`, whose pattern is `pattern`: the queue's own policy grants the
/// EventBridge principal the send on it, and the rule is pointed at it. Answers the queue's url.
fn given_rule_and_queue(
    emulator: &Started,
    case: &str,
    rule: &str,
    queue: &str,
    pattern: &Value,
) -> String {
    let created = sqs(emulator, case, "CreateQueue", &json!({"QueueName": queue}));
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a QueueUrl in {created}"))
        .to_string();
    let attributes = sqs(
        emulator,
        case,
        "GetQueueAttributes",
        &json!({"QueueUrl": &url, "AttributeNames": ["QueueArn"]}),
    );
    let queue_arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a QueueArn in {attributes}"))
        .to_string();
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AllowEventBridgeDelivery",
            "Effect": "Allow",
            "Principal": {"Service": EVENTS_PRINCIPAL},
            "Action": "sqs:SendMessage",
            "Resource": &queue_arn,
            "Condition": {
                "ArnEquals": {
                    "aws:SourceArn": format!("arn:aws:events:{REGION}:{ACCOUNT}:rule/{rule}"),
                },
            },
        }],
    })
    .to_string();
    sqs(
        emulator,
        case,
        "SetQueueAttributes",
        &json!({"QueueUrl": &url, "Attributes": {"Policy": policy}}),
    );
    let put = events(
        emulator,
        case,
        "PutRule",
        &json!({"Name": rule, "EventPattern": pattern.to_string(), "State": "ENABLED"}),
    );
    assert_eq!(
        put["RuleArn"],
        json!(format!("arn:aws:events:{REGION}:{ACCOUNT}:rule/{rule}")),
        "case {case:?}: expected the rule ARN the queue's grant names, got {put}"
    );
    let targeted = events(
        emulator,
        case,
        "PutTargets",
        &json!({"Rule": rule, "Targets": [{"Id": "q", "Arn": &queue_arn}]}),
    );
    assert_eq!(
        targeted["FailedEntryCount"],
        json!(0),
        "case {case:?}: the queue target must be accepted, got {targeted}"
    );
    url
}

/// Register one `FARGATE`-compatible revision of `family` running `image`.
fn register(emulator: &Started, case: &str, family: &str, image: &str) {
    ecs(
        emulator,
        case,
        "RegisterTaskDefinition",
        &json!({
            "family": family,
            "networkMode": "awsvpc",
            "requiresCompatibilities": ["FARGATE"],
            // A documented Fargate task size: 256 CPU units, 512 MiB.
            "cpu": "256",
            "memory": "512",
            "containerDefinitions": [{"name": "web", "image": image, "essential": true}],
        }),
    );
}

/// Create one `FARGATE` service on `family`, placed in `subnet` with the public address a public
/// registry's image is fetched over; `breaker` turns the deployment circuit breaker and its
/// rollback on. Answers the ARN Amazon ECS reports for it.
fn create_service(
    emulator: &Started,
    case: &str,
    name: &str,
    family: &str,
    subnet: &str,
    breaker: bool,
) -> String {
    let created = ecs(
        emulator,
        case,
        "CreateService",
        &json!({
            "cluster": CLUSTER,
            "serviceName": name,
            "taskDefinition": family,
            "launchType": "FARGATE",
            "desiredCount": 1,
            "networkConfiguration": {
                "awsvpcConfiguration": {
                    "subnets": [subnet],
                    "assignPublicIp": "ENABLED",
                },
            },
            "deploymentConfiguration": {
                "deploymentCircuitBreaker": {"enable": breaker, "rollback": breaker},
            },
        }),
    );
    let arn = created["service"]["serviceArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a serviceArn in {created}"))
        .to_string();
    assert_eq!(
        arn,
        service_arn(name),
        "case {case:?}: expected the service ARN a rule is scoped to, got {created}"
    );
    arn
}

/// Receive everything waiting on the queue at `url` and delete it; answers the parsed envelopes.
fn drain(emulator: &Started, case: &str, url: &str) -> Vec<Value> {
    let received = sqs(
        emulator,
        case,
        "ReceiveMessage",
        &json!({"QueueUrl": url, "MaxNumberOfMessages": 10}),
    );
    let messages = received["Messages"].as_array().cloned().unwrap_or_default();
    let mut envelopes = Vec::new();
    for message in &messages {
        if let Some(handle) = message["ReceiptHandle"].as_str() {
            sqs(
                emulator,
                case,
                "DeleteMessage",
                &json!({"QueueUrl": url, "ReceiptHandle": handle}),
            );
        }
        envelopes.push(
            serde_json::from_str::<Value>(message["Body"].as_str().unwrap_or_default())
                .unwrap_or(Value::Null),
        );
    }
    envelopes
}

/// The envelopes of `held` whose `detail.eventName` is `event_name`.
fn named<'held>(held: &'held [Value], event_name: &str) -> Vec<&'held Value> {
    held.iter()
        .filter(|event| event.pointer("/detail/eventName") == Some(&json!(event_name)))
        .collect()
}

/// The service `name`, as `DescribeServices` reports it.
fn describe(emulator: &Started, case: &str, name: &str) -> Value {
    let described = ecs(
        emulator,
        case,
        "DescribeServices",
        &json!({"cluster": CLUSTER, "services": [name]}),
    );
    described["services"]
        .as_array()
        .and_then(|services| services.first())
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected the service back, got {described}"))
}

/// The service's PRIMARY deployment, as `DescribeServices` reports it.
fn primary(service: &Value) -> Value {
    service["deployments"]
        .as_array()
        .and_then(|deployments| {
            deployments
                .iter()
                .find(|deployment| deployment["status"] == json!("PRIMARY"))
        })
        .cloned()
        .unwrap_or(Value::Null)
}

/// The deployment of `service` whose id is `id`, and `Value::Null` when it holds none.
fn deployment(service: &Value, id: &str) -> Value {
    service["deployments"]
        .as_array()
        .and_then(|deployments| {
            deployments
                .iter()
                .find(|deployment| deployment["id"] == json!(id))
        })
        .cloned()
        .unwrap_or(Value::Null)
}

/// Read the service again until `settled` holds of the reading, or the generous deadline passes;
/// answers the last reading either way, so the assertion that follows names it.
fn poll_service(
    emulator: &Started,
    case: &str,
    name: &str,
    settled: impl Fn(&Value) -> bool,
) -> Value {
    let started = Instant::now();
    loop {
        let answered = describe(emulator, case, name);
        if settled(&answered) || started.elapsed() >= DELIVERY_DEADLINE {
            return answered;
        }
        std::thread::sleep(LOOK_INTERVAL);
    }
}

#[test]
fn a_service_action_reaches_its_own_rules_queue_on_a_started_emulator() {
    let case = "an unpullable service beside a healthy sibling";
    let emulator = Started::start();

    let subnet = public_subnet(&emulator, case);
    ecs(
        &emulator,
        case,
        "CreateCluster",
        &json!({"clusterName": CLUSTER}),
    );
    register(&emulator, case, "goodfamily", PULLABLE_IMAGE);
    register(&emulator, case, "badfamily", UNPULLABLE_IMAGE);

    let bad_service = service_arn("badservice");
    let good_service = service_arn("goodservice");
    let bad_url = given_rule_and_queue(
        &emulator,
        case,
        "started-bad-actions",
        "started-bad-queue",
        &json!({
            "source": [EVENT_SOURCE],
            "detail-type": [SERVICE_ACTION],
            "resources": [&bad_service],
        }),
    );
    let good_url = given_rule_and_queue(
        &emulator,
        case,
        "started-good-actions",
        "started-good-queue",
        &json!({
            "source": [EVENT_SOURCE],
            "detail-type": [SERVICE_ACTION],
            "resources": [&good_service],
        }),
    );

    create_service(&emulator, case, "badservice", "badfamily", &subnet, false);
    create_service(&emulator, case, "goodservice", "goodfamily", &subnet, false);

    // ONLY the queues are polled: a client watching a service through EventBridge makes no
    // further Amazon ECS call, so the actions have to arrive with nobody asking Amazon ECS.
    let mut bad_held: Vec<Value> = Vec::new();
    let mut good_held: Vec<Value> = Vec::new();
    let started = Instant::now();
    while started.elapsed() < DELIVERY_DEADLINE
        && !(!named(&bad_held, TASK_CONFIGURATION_FAILURE).is_empty()
            && !named(&good_held, STEADY_STATE).is_empty())
    {
        bad_held.extend(drain(&emulator, case, &bad_url));
        good_held.extend(drain(&emulator, case, &good_url));
        std::thread::sleep(LOOK_INTERVAL);
    }

    let failures = named(&bad_held, TASK_CONFIGURATION_FAILURE);
    assert!(
        !failures.is_empty(),
        "case {case:?}: expected {TASK_CONFIGURATION_FAILURE} for {bad_service} on its own \
         rule's queue, got {bad_held:?}; the log:\n{}",
        emulator.log()
    );
    let cluster_arn = format!("arn:aws:ecs:{REGION}:{ACCOUNT}:cluster/{CLUSTER}");
    for envelope in &failures {
        for (pointer, expected) in [
            ("/source", json!(EVENT_SOURCE)),
            ("/detail-type", json!(SERVICE_ACTION)),
            ("/region", json!(REGION)),
            ("/account", json!(ACCOUNT)),
            ("/resources", json!([&bad_service])),
            ("/detail/eventType", json!(ERROR)),
            ("/detail/clusterArn", json!(&cluster_arn)),
        ] {
            assert_eq!(
                envelope.pointer(pointer),
                Some(&expected),
                "case {case:?}: expected {pointer} to be {expected}, got {envelope}"
            );
        }
        let reason = envelope
            .pointer("/detail/reason")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            reason.starts_with(CANNOT_PULL) && reason.contains(UNPULLABLE_IMAGE),
            "case {case:?}: expected detail.reason to begin {CANNOT_PULL:?} and name \
             {UNPULLABLE_IMAGE:?}, got {reason:?}"
        );
    }

    let steady = named(&good_held, STEADY_STATE);
    assert!(
        !steady.is_empty(),
        "case {case:?}: expected {STEADY_STATE} for {good_service} on its own rule's queue, \
         got {good_held:?}; the log:\n{}",
        emulator.log()
    );
    for envelope in &steady {
        assert_eq!(
            envelope.pointer("/resources"),
            Some(&json!([&good_service])),
            "case {case:?}: expected resources to name only {good_service}, got {envelope}"
        );
        assert_eq!(
            envelope.pointer("/detail/eventType"),
            Some(&json!(INFO)),
            "case {case:?}: expected eventType {INFO}, got {envelope}"
        );
    }

    // The negative half: each rule is scoped to ONE service, so neither queue holds the other
    // service's action.
    assert!(
        named(&good_held, TASK_CONFIGURATION_FAILURE).is_empty(),
        "case {case:?}: the healthy service's rule must receive no failure of {bad_service}, \
         got {good_held:?}"
    );
    assert!(
        named(&bad_held, STEADY_STATE).is_empty(),
        "case {case:?}: the failing service reaches no steady state, got {bad_held:?}"
    );
}

/// RULE (the `ECS Deployment State Change` half): a release publishes every state its rollout
/// moves into, naming the deployment `DescribeServices` reports — its start and its completion as
/// progress, and the state the deployment circuit breaker leaves a release that cannot launch its
/// revision as an error, with the reason the API reports for it — and the rollback that breaker
/// performs takes the service back to the revision that completed, all on the emulator a client
/// starts.
#[test]
fn a_release_publishes_its_rollout_states_on_a_started_emulator() {
    let case = "a release whose revision cannot be launched";
    let service = "releaseservice";
    let emulator = Started::start();

    let subnet = public_subnet(&emulator, case);
    ecs(
        &emulator,
        case,
        "CreateCluster",
        &json!({"clusterName": CLUSTER}),
    );
    register(&emulator, case, "releasefamily", PULLABLE_IMAGE);
    let arn = service_arn(service);
    let url = given_rule_and_queue(
        &emulator,
        case,
        "started-deployment-states",
        "started-deployment-queue",
        &json!({
            "source": [EVENT_SOURCE],
            "detail-type": [DEPLOYMENT_STATE_CHANGE],
            "resources": [&arn],
        }),
    );

    create_service(&emulator, case, service, "releasefamily", &subnet, true);

    // The release of a revision that CAN be launched: its rollout starts and completes, and both
    // moves are published naming the deployment the API reports.
    let settled = poll_service(&emulator, case, service, |service| {
        primary(service)["rolloutState"] == json!(COMPLETED)
    });
    let first_deployment = primary(&settled)["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !first_deployment.is_empty(),
        "case {case:?}: expected the PRIMARY deployment to carry an id, got {settled}"
    );
    let mut held: Vec<Value> = Vec::new();
    let started = Instant::now();
    while started.elapsed() < DELIVERY_DEADLINE
        && !(!named(&held, DEPLOYMENT_IN_PROGRESS).is_empty()
            && !named(&held, DEPLOYMENT_COMPLETED).is_empty())
    {
        held.extend(drain(&emulator, case, &url));
        std::thread::sleep(LOOK_INTERVAL);
    }
    for event_name in [DEPLOYMENT_IN_PROGRESS, DEPLOYMENT_COMPLETED] {
        let delivered = named(&held, event_name);
        assert!(
            !delivered.is_empty(),
            "case {case:?}: expected {event_name} of {arn} on the rule's queue, got {held:?}; \
             the log:\n{}",
            emulator.log()
        );
        for envelope in delivered {
            for (pointer, expected) in [
                ("/source", json!(EVENT_SOURCE)),
                ("/detail-type", json!(DEPLOYMENT_STATE_CHANGE)),
                ("/resources", json!([&arn])),
                ("/detail/eventType", json!(INFO)),
                ("/detail/deploymentId", json!(&first_deployment)),
            ] {
                assert_eq!(
                    envelope.pointer(pointer),
                    Some(&expected),
                    "case {case:?}: expected {pointer} to be {expected}, got {envelope}"
                );
            }
        }
    }

    // The release of a revision that CANNOT be launched: the breaker gives that rollout the
    // terminal state of the state machine, published as an error naming the deployment the API
    // reports as failed.
    register(&emulator, case, "releasefamily", UNPULLABLE_IMAGE);
    ecs(
        &emulator,
        case,
        "UpdateService",
        &json!({
            "cluster": CLUSTER,
            "service": service,
            "taskDefinition": "releasefamily:2",
        }),
    );
    let releasing = poll_service(&emulator, case, service, |service| {
        let deployment = primary(service);
        deployment["taskDefinition"]
            .as_str()
            .is_some_and(|reference| reference.ends_with("releasefamily:2"))
            && [IN_PROGRESS, FAILED]
                .iter()
                .any(|state| deployment["rolloutState"] == json!(state))
    });
    let failing = primary(&releasing)["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_ne!(
        failing, first_deployment,
        "case {case:?}: the release is a new deployment of its own, got {releasing}"
    );
    let failed = poll_service(&emulator, case, service, |service| {
        deployment(service, &failing)["rolloutState"] == json!(FAILED)
    });
    let reported = deployment(&failed, &failing);
    assert_eq!(
        reported["rolloutState"],
        json!(FAILED),
        "case {case:?}: expected the release to be reported {FAILED}, got {failed}; the log:\n{}",
        emulator.log()
    );
    let why = reported["rolloutStateReason"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        why.contains("circuit breaker"),
        "case {case:?}: expected the rollout state reason to name the circuit breaker, got {why:?}"
    );
    let started = Instant::now();
    while started.elapsed() < DELIVERY_DEADLINE && named(&held, DEPLOYMENT_FAILED).is_empty() {
        held.extend(drain(&emulator, case, &url));
        std::thread::sleep(LOOK_INTERVAL);
    }
    let failures = named(&held, DEPLOYMENT_FAILED);
    assert!(
        !failures.is_empty(),
        "case {case:?}: expected {DEPLOYMENT_FAILED} of {arn} on the rule's queue, got {held:?}; \
         the log:\n{}",
        emulator.log()
    );
    for envelope in &failures {
        for (pointer, expected) in [
            ("/resources", json!([&arn])),
            ("/detail/eventType", json!(ERROR)),
            ("/detail/deploymentId", json!(&failing)),
            ("/detail/reason", json!(&why)),
        ] {
            assert_eq!(
                envelope.pointer(pointer),
                Some(&expected),
                "case {case:?}: expected {pointer} to be {expected}, got {envelope}"
            );
        }
    }

    // And where the breaker's ROLLBACK leaves the service: back on the revision that completed,
    // with that redeployment itself reported complete.
    let rolled_back = poll_service(&emulator, case, service, |service| {
        let deployment = primary(service);
        deployment["taskDefinition"]
            .as_str()
            .is_some_and(|reference| reference.ends_with("releasefamily:1"))
            && deployment["rolloutState"] == json!(COMPLETED)
    });
    let restored = primary(&rolled_back);
    assert!(
        restored["taskDefinition"]
            .as_str()
            .is_some_and(|reference| reference.ends_with("releasefamily:1")),
        "case {case:?}: expected the rollback to put the first revision back, got {rolled_back}; \
         the log:\n{}",
        emulator.log()
    );
    assert_eq!(
        restored["rolloutState"],
        json!(COMPLETED),
        "case {case:?}: expected the restored release to complete, got {rolled_back}"
    );
    assert_ne!(
        restored["id"],
        json!(&failing),
        "case {case:?}: the rollback is a release of its own, got {rolled_back}"
    );
}
