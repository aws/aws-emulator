// SPDX-License-Identifier: CC0-1.0

//! The SERVICE-level events Amazon ECS publishes reach the Amazon SQS targets of the
//! EventBridge rules scoped to each service, on a router that VERIFIES request signatures —
//! the configuration the emulator serves traffic in.
//!
//! The RULE under test: **every action the Amazon ECS service scheduler takes, and every state
//! its service DEPLOYMENTS move into, is published once onto the account's default EventBridge
//! bus under `source` `aws.ecs` — the scheduler's actions as `detail-type` `ECS Service Action`
//! and a deployment's rollout states as `ECS Deployment State Change` — with the envelope's
//! `resources` naming ONLY the service the action or the rollout belongs to, and an event that
//! reports a FAILURE carrying in `detail.reason` the account of what failed that the service's
//! own records hold (for a launch the scheduler lost, the `Task$stoppedReason` of the stop that
//! launch earned; for a rollout, its `rolloutStateReason`). A rule whose `EventPattern` matches
//! delivers the envelope to its targets, so a rule scoped by `resources` to one service
//! receives that service's events and never a sibling's — however its `detail` filter is
//! written.**
//!
//! Sources: the Amazon ECS Developer Guide's service action and service deployment state change
//! events for the `eventName`/`eventType` pairs and the `detail` members each carries;
//! `smithy_query(query="shape", shape="com.amazonaws.ecs#TaskStopCode")` ->
//! EssentialContainerExited, InfrastructureHealth, ServiceSchedulerInitiated, SpotInterruption,
//! TaskFailedToStart, TerminationNotice, UserInitiated for the stop a launch that was never made
//! earns; `Deployment$rolloutState` ("When a service deployment is started, it begins in an
//! `IN_PROGRESS` state. When the service reaches a steady state, the deployment transitions to a
//! `COMPLETED` state. If the service fails to reach a steady state and circuit breaker is turned
//! on, the deployment transitions to a `FAILED` state") and `DeploymentCircuitBreaker$rollback`
//! ("If rollback is on, when a service deployment fails, the service is rolled back to the last
//! deployment that completed successfully") for the sequence a release goes through; and
//! `PutTargets` for the grant a queue target needs ("for Lambda functions, Amazon SNS topics and
//! Amazon SQS queues, EventBridge relies on resource-based policies").
//!
//! What this suite adds to the crate-local suites of the Amazon ECS crate and to
//! [`crate::ecs_service_launch_failure_events_reach_their_sqs_target`], which mount three
//! services with request authentication OFF:
//!
//! * **The signatures are verified.** Every call below is signed with the keys this process
//!   publishes once authentication is on — and so is the publication Amazon ECS makes THROUGH
//!   THE EMULATOR'S OWN CALLBACK, which a router that authenticates every request has to accept
//!   before EventBridge ever sees the event.
//! * **The placement is a network created through Amazon EC2's own API**, so the image fetch the
//!   launch makes is decided against the records that service holds.
//! * **The queue's grant is SCOPED to its own rule** (`aws:SourceArn`), which is how a client
//!   that fans one service's events out to its own queue writes that policy.
//! * **The deployment lifecycle is driven to its END** — a release that fails and the rollback
//!   that follows it — inside one client's wait.
//!
//! The same two rules are driven over the WHOLE assembled emulator, as the binary a client
//! starts serves it, by [`crate::ecs_service_events_on_a_started_emulator`]; this suite mounts
//! the four services the chain needs, so it costs one router rather than the whole assembly.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use aws_credential_types::Credentials;
use aws_sigv4::http_request::{sign, SignableBody, SignableRequest, SigningSettings};
use aws_sigv4::sign::v4::SigningParams;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, form_encode};

/// The Region every call is signed for and addressed in.
const REGION: &str = "us-west-2";

/// The account every emulator ARN carries.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The `source` and the two `detail-type`s Amazon ECS publishes its service-level events
/// under, spelled out here rather than imported from the service crate, so a wrong constant in
/// the implementation cannot make this suite pass.
const EVENT_SOURCE: &str = "aws.ecs";
const SERVICE_ACTION: &str = "ECS Service Action";
const DEPLOYMENT_STATE_CHANGE: &str = "ECS Deployment State Change";

/// The `eventName`s and `eventType`s under test, as the Developer Guide's tables record them.
const TASK_CONFIGURATION_FAILURE: &str = "SERVICE_TASK_CONFIGURATION_FAILURE";
const STEADY_STATE: &str = "SERVICE_STEADY_STATE";
const DEPLOYMENT_IN_PROGRESS: &str = "SERVICE_DEPLOYMENT_IN_PROGRESS";
const DEPLOYMENT_COMPLETED: &str = "SERVICE_DEPLOYMENT_COMPLETED";
const DEPLOYMENT_FAILED: &str = "SERVICE_DEPLOYMENT_FAILED";
const INFO: &str = "INFO";
const ERROR: &str = "ERROR";

/// The error class a container runtime reports an image it could not fetch under, which is the
/// fixed text such a launch failure's `reason` begins with.
const CANNOT_PULL: &str = "CannotPullContainerError";

/// `Deployment$rolloutState` values a release passes through.
const IN_PROGRESS: &str = "IN_PROGRESS";
const COMPLETED: &str = "COMPLETED";
const FAILED: &str = "FAILED";

/// The service principal EventBridge delivers to an SQS target as, which the queue's own policy
/// has to allow the send for.
const EVENTS_PRINCIPAL: &str = "events.amazonaws.com";

/// Amazon EC2's signing name and the API version its form wire carries.
const EC2_SIGNING_NAME: &str = "ec2";
const EC2_API_VERSION: &str = "2016-11-15";

/// Amazon ECS's signing name and the target prefix its `awsJson1_1` wire carries.
const ECS_SIGNING_NAME: &str = "ecs";
const ECS_TARGET_PREFIX: &str = "AmazonEC2ContainerServiceV20141113";

/// The destination of the route that carries traffic off a VPC.
const INTERNET_DESTINATION: &str = "0.0.0.0/0";

/// An image the public gallery this emulator serves PUBLISHES a release of, and a reference into
/// the same repository naming a tag it publishes no release under — the configuration a
/// service's launches cannot be made with.
const PULLABLE_IMAGE: &str = "public.ecr.aws/docker/library/busybox:latest";
const UNPULLABLE_IMAGE: &str = "public.ecr.aws/docker/library/busybox:no-such-release";

/// How long a wait keeps looking before giving up, and how often it looks meanwhile. Generous —
/// what is asserted is that the envelope arrives at all, never how fast the host runs — and
/// still far inside the span a suite may take, because every wait here is on the CONDITION and
/// the whole chain settles in seconds.
const DELIVERY_WAIT: Duration = Duration::from_secs(50);
const LOOK_INTERVAL: Duration = Duration::from_millis(100);

/// The four services this chain travels, over ONE shared engine, mounted the way the production
/// router mounts them — and with request authentication ON, as the emulator serves traffic.
fn emulator() -> (MultiServiceRouter, Signer) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(true);
    router.register_service(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        engine,
    ));
    let published = ::emulator_server::SigV4Authenticator::minted_credentials()
        .expect("turning authentication on publishes this process's keys");
    let signer = Signer {
        key_id: published.access_key_id.clone(),
        secret: published.secret_access_key.clone(),
    };
    (router, signer)
}

/// The key pair this process publishes once request authentication is on, which every call below
/// is signed with.
struct Signer {
    key_id: String,
    secret: String,
}

impl Signer {
    /// Sign `request` for `signing_name` in [`REGION`], in the `Authorization` header an SDK
    /// request carries.
    fn sign(
        &self,
        mut request: http::Request<Vec<u8>>,
        signing_name: &str,
    ) -> http::Request<Vec<u8>> {
        let identity = Credentials::new(&self.key_id, &self.secret, None, None, "test").into();
        let params = SigningParams::builder()
            .identity(&identity)
            .region(REGION)
            .name(signing_name)
            .time(SystemTime::now())
            .settings(SigningSettings::default())
            .build()
            .expect("the signing parameters are complete")
            .into();
        let (method, uri) = (request.method().to_string(), request.uri().to_string());
        let headers: Vec<(String, String)> = request
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_string(),
                    value.to_str().expect("a text header").to_string(),
                )
            })
            .collect();
        let signable = SignableRequest::new(
            &method,
            &uri,
            headers
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str())),
            SignableBody::Bytes(request.body()),
        )
        .expect("the request is signable");
        let (instructions, _) = sign(signable, &params)
            .expect("the request signs")
            .into_parts();
        instructions.apply_to_request_http1x(&mut request);
        request
    }
}

/// One SIGNED `POST /` on the regional endpoint of `signing_name`.
async fn call(
    router: &MultiServiceRouter,
    signer: &Signer,
    signing_name: &str,
    content_type: &str,
    target: Option<&str>,
    body: Vec<u8>,
) -> (u16, Vec<u8>) {
    let host = format!("{signing_name}.{REGION}.amazonaws.com");
    let mut builder = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", &host)
        .header("content-type", content_type);
    if let Some(target) = target {
        builder = builder.header("x-amz-target", target);
    }
    let built = builder.body(body).expect("the test request is well formed");
    let signed = signer.sign(built, signing_name);
    let (parts, payload) = signed.into_parts();
    let request = http::Request::from_parts(parts, bytes::Bytes::from(payload));
    let response = router.dispatch(request).await;
    (response.status().as_u16(), response.body().to_vec())
}

/// One `X-Amz-Target` RPC call, requiring a 200 and answering the parsed body.
async fn rpc(
    router: &MultiServiceRouter,
    signer: &Signer,
    case: &str,
    signing_name: &str,
    content_type: &str,
    target: &str,
    body: &Value,
) -> Value {
    let (status, payload) = call(
        router,
        signer,
        signing_name,
        content_type,
        Some(target),
        body.to_string().into_bytes(),
    )
    .await;
    let parsed: Value = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {target}, got {status} with {parsed}"
    );
    parsed
}

/// One Amazon ECS call (`awsJson1_1`).
async fn ecs(
    router: &MultiServiceRouter,
    signer: &Signer,
    case: &str,
    operation: &str,
    body: &Value,
) -> Value {
    rpc(
        router,
        signer,
        case,
        ECS_SIGNING_NAME,
        "application/x-amz-json-1.1",
        &format!("{ECS_TARGET_PREFIX}.{operation}"),
        body,
    )
    .await
}

/// One EventBridge call (`awsJson1_1`).
async fn events(
    router: &MultiServiceRouter,
    signer: &Signer,
    case: &str,
    operation: &str,
    body: &Value,
) -> Value {
    rpc(
        router,
        signer,
        case,
        "events",
        "application/x-amz-json-1.1",
        &format!("AWSEvents.{operation}"),
        body,
    )
    .await
}

/// One Amazon SQS call (`awsJson1_0`).
async fn sqs(
    router: &MultiServiceRouter,
    signer: &Signer,
    case: &str,
    operation: &str,
    body: &Value,
) -> Value {
    rpc(
        router,
        signer,
        case,
        "sqs",
        "application/x-amz-json-1.0",
        &format!("AmazonSQS.{operation}"),
        body,
    )
    .await
}

/// One Amazon EC2 call over its `ec2Query` FORM wire, requiring a 200 and answering the XML.
async fn ec2(
    router: &MultiServiceRouter,
    signer: &Signer,
    case: &str,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    let mut body = format!("Action={action}&Version={EC2_API_VERSION}");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let (status, payload) = call(
        router,
        signer,
        EC2_SIGNING_NAME,
        "application/x-www-form-urlencoded",
        None,
        body.into_bytes(),
    )
    .await;
    let text = String::from_utf8_lossy(&payload).to_string();
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from EC2 {action}, got {status} with {text}"
    );
    text
}

/// A VPC with one PUBLIC subnet, built through Amazon EC2's own API: an internet gateway
/// attached to it and a default route to that gateway in its main route table, which is the path
/// a task given a public address fetches a public registry's image over.
async fn public_subnet(router: &MultiServiceRouter, signer: &Signer, case: &str) -> String {
    let vpc = element(
        &ec2(
            router,
            signer,
            case,
            "CreateVpc",
            &[("CidrBlock", "10.76.0.0/16")],
        )
        .await,
        "vpcId",
    );
    let subnet = element(
        &ec2(
            router,
            signer,
            case,
            "CreateSubnet",
            &[
                ("VpcId", &vpc),
                ("CidrBlock", "10.76.1.0/24"),
                ("AvailabilityZone", &format!("{REGION}a")),
            ],
        )
        .await,
        "subnetId",
    );
    let gateway = element(
        &ec2(router, signer, case, "CreateInternetGateway", &[]).await,
        "internetGatewayId",
    );
    ec2(
        router,
        signer,
        case,
        "AttachInternetGateway",
        &[("InternetGatewayId", &gateway), ("VpcId", &vpc)],
    )
    .await;
    let table = element(
        &ec2(
            router,
            signer,
            case,
            "DescribeRouteTables",
            &[("Filter.1.Name", "vpc-id"), ("Filter.1.Value.1", &vpc)],
        )
        .await,
        "routeTableId",
    );
    ec2(
        router,
        signer,
        case,
        "CreateRoute",
        &[
            ("RouteTableId", &table),
            ("DestinationCidrBlock", INTERNET_DESTINATION),
            ("GatewayId", &gateway),
        ],
    )
    .await;
    subnet
}

/// A queue a rule can target, with its own policy allowing EventBridge the send ONLY for the
/// rule `rule_arn` — the scoped grant a client writes per rule.
async fn given_scoped_queue(
    router: &MultiServiceRouter,
    signer: &Signer,
    case: &str,
    name: &str,
    rule_arn: &str,
) -> (String, String) {
    let created = sqs(
        router,
        signer,
        case,
        "CreateQueue",
        &json!({"QueueName": name}),
    )
    .await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a QueueUrl in {created}"))
        .to_string();
    let attributes = sqs(
        router,
        signer,
        case,
        "GetQueueAttributes",
        &json!({"QueueUrl": &url, "AttributeNames": ["QueueArn"]}),
    )
    .await;
    let arn = attributes["Attributes"]["QueueArn"]
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
            "Resource": &arn,
            "Condition": {"ArnEquals": {"aws:SourceArn": rule_arn}},
        }],
    })
    .to_string();
    sqs(
        router,
        signer,
        case,
        "SetQueueAttributes",
        &json!({"QueueUrl": &url, "Attributes": {"Policy": policy}}),
    )
    .await;
    (url, arn)
}

/// The ARN EventBridge mints for the rule `name` on the default bus, which the queue's scoped
/// grant names.
fn rule_arn(name: &str) -> String {
    format!("arn:aws:events:{REGION}:{ACCOUNT}:rule/{name}")
}

/// The ARN Amazon ECS mints for the service `name` of `cluster`, which a rule's `resources`
/// clause scopes to.
fn service_arn(cluster: &str, name: &str) -> String {
    format!("arn:aws:ecs:{REGION}:{ACCOUNT}:service/{cluster}/{name}")
}

/// Put the rule `name` with `pattern` and point it at the queue `queue_arn`.
async fn given_rule(
    router: &MultiServiceRouter,
    signer: &Signer,
    case: &str,
    name: &str,
    pattern: &Value,
    queue_arn: &str,
) {
    let put = events(
        router,
        signer,
        case,
        "PutRule",
        &json!({"Name": name, "EventPattern": pattern.to_string(), "State": "ENABLED"}),
    )
    .await;
    assert_eq!(
        put["RuleArn"],
        json!(rule_arn(name)),
        "case {case:?}: expected the rule's ARN to be the one its queue's grant names, got {put}"
    );
    let targeted = events(
        router,
        signer,
        case,
        "PutTargets",
        &json!({"Rule": name, "Targets": [{"Id": "q", "Arn": queue_arn}]}),
    )
    .await;
    assert_eq!(
        targeted["FailedEntryCount"],
        json!(0),
        "case {case:?}: the queue target must be accepted, got {targeted}"
    );
}

/// Receive everything waiting on the queue at `url` and delete it; answers the parsed envelopes.
async fn drain(router: &MultiServiceRouter, signer: &Signer, case: &str, url: &str) -> Vec<Value> {
    let received = sqs(
        router,
        signer,
        case,
        "ReceiveMessage",
        &json!({"QueueUrl": url, "MaxNumberOfMessages": 10}),
    )
    .await;
    let messages = received["Messages"].as_array().cloned().unwrap_or_default();
    let mut envelopes = Vec::new();
    for message in &messages {
        if let Some(handle) = message["ReceiptHandle"].as_str() {
            sqs(
                router,
                signer,
                case,
                "DeleteMessage",
                &json!({"QueueUrl": url, "ReceiptHandle": handle}),
            )
            .await;
        }
        envelopes.push(
            serde_json::from_str::<Value>(message["Body"].as_str().unwrap_or_default())
                .unwrap_or(Value::Null),
        );
    }
    envelopes
}

/// Whether `held` carries an envelope whose `detail.eventName` is `event_name`.
fn holds(held: &[Value], event_name: &str) -> bool {
    held.iter()
        .any(|event| event.pointer("/detail/eventName") == Some(&json!(event_name)))
}

/// The envelopes of `held` whose `detail.eventName` is `event_name`.
fn named<'held>(held: &'held [Value], event_name: &str) -> Vec<&'held Value> {
    held.iter()
        .filter(|event| event.pointer("/detail/eventName") == Some(&json!(event_name)))
        .collect()
}

/// Register one `FARGATE`-compatible revision of `family` running `image`; answers its ARN.
async fn register(
    router: &MultiServiceRouter,
    signer: &Signer,
    case: &str,
    family: &str,
    image: &str,
) -> String {
    let registered = ecs(
        router,
        signer,
        case,
        "RegisterTaskDefinition",
        &json!({
            "family": family,
            "networkMode": "awsvpc",
            "requiresCompatibilities": ["FARGATE"],
            // A documented Fargate task size: 256 CPU units, 512 MiB.
            "cpu": "256",
            "memory": "512",
            "containerDefinitions": [{
                "name": "web",
                "image": image,
                "essential": true,
            }],
        }),
    )
    .await;
    registered["taskDefinition"]["taskDefinitionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a taskDefinitionArn in {registered}"))
        .to_string()
}

/// What one service of these suites is created as: where it runs, what it runs, where its tasks
/// are placed, and whether the deployment circuit breaker and its rollback are turned on.
///
/// A struct rather than a parameter list, because the service a row creates is ONE description
/// and reads as one at each call.
struct ServiceSpec<'spec> {
    cluster: &'spec str,
    name: &'spec str,
    task_definition: &'spec str,
    subnet: &'spec str,
    breaker: bool,
}

/// Create one `FARGATE` service as `spec` describes it, with the public address a public
/// registry's image is fetched over.
async fn create_service(
    router: &MultiServiceRouter,
    signer: &Signer,
    case: &str,
    spec: &ServiceSpec<'_>,
) -> String {
    let request = json!({
        "cluster": spec.cluster,
        "serviceName": spec.name,
        "taskDefinition": spec.task_definition,
        "launchType": "FARGATE",
        "desiredCount": 1,
        "networkConfiguration": {
            "awsvpcConfiguration": {
                "subnets": [spec.subnet],
                "assignPublicIp": "ENABLED",
            },
        },
        "deploymentConfiguration": {
            "deploymentCircuitBreaker": {"enable": spec.breaker, "rollback": spec.breaker},
            "minimumHealthyPercent": 100,
            "maximumPercent": 200,
        },
    });
    let created = ecs(router, signer, case, "CreateService", &request).await;
    let arn = created["service"]["serviceArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a serviceArn in {created}"))
        .to_string();
    assert_eq!(
        arn,
        service_arn(spec.cluster, spec.name),
        "case {case:?}: expected the service ARN a rule is scoped to, got {created}"
    );
    arn
}

/// The service `name` of `cluster`, as `DescribeServices` reports it.
async fn describe(
    router: &MultiServiceRouter,
    signer: &Signer,
    case: &str,
    cluster: &str,
    name: &str,
) -> Value {
    let described = ecs(
        router,
        signer,
        case,
        "DescribeServices",
        &json!({"cluster": cluster, "services": [name]}),
    )
    .await;
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

/// Look again at what `read` answers until `settled` holds of it, or the generous deadline
/// passes; answers the last reading either way, so the assertion that follows names it.
async fn poll_until<Read, Reading>(mut read: Read, settled: impl Fn(&Value) -> bool) -> Value
where
    Read: FnMut() -> Reading,
    Reading: std::future::Future<Output = Value>,
{
    let started = Instant::now();
    loop {
        let answered = read().await;
        if settled(&answered) || started.elapsed() >= DELIVERY_WAIT {
            return answered;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// RULE (the `ECS Service Action` half): the scheduler's action on ONE service reaches the queue
/// of the rule scoped to that service — a failure carrying what the launch failed on in
/// `detail.reason` — and never the queue of a rule scoped to a sibling, however that rule's
/// `detail` filter is written.
#[tokio::test]
async fn a_service_action_reaches_only_the_rule_scoped_to_its_own_service() {
    let case = "an unpullable service beside a healthy sibling";
    let cluster = "actionscope";
    let (router, signer) = emulator();

    let subnet = public_subnet(&router, &signer, case).await;
    ecs(
        &router,
        &signer,
        case,
        "CreateCluster",
        &json!({"clusterName": cluster}),
    )
    .await;
    register(&router, &signer, case, "goodfamily", PULLABLE_IMAGE).await;
    register(&router, &signer, case, "badfamily", UNPULLABLE_IMAGE).await;

    // Three rules on the same source and detail type, differing in the service their
    // `resources` clause names — and the third also filtering on `detail.eventName`, which is
    // the near miss: it lists the failure, and the service it is scoped to has none.
    let bad_service = service_arn(cluster, "badservice");
    let good_service = service_arn(cluster, "goodservice");
    let rules = [
        (
            "bad-service-actions",
            "bad-actions-queue",
            json!({
                "source": [EVENT_SOURCE],
                "detail-type": [SERVICE_ACTION],
                "resources": [&bad_service],
            }),
        ),
        (
            "good-service-actions",
            "good-actions-queue",
            json!({
                "source": [EVENT_SOURCE],
                "detail-type": [SERVICE_ACTION],
                "resources": [&good_service],
            }),
        ),
        (
            "good-service-failures",
            "near-miss-queue",
            json!({
                "source": [EVENT_SOURCE],
                "detail-type": [SERVICE_ACTION],
                "resources": [&good_service],
                "detail": {"eventName": [TASK_CONFIGURATION_FAILURE, STEADY_STATE]},
            }),
        ),
    ];
    let mut urls: Vec<String> = Vec::new();
    for (rule, queue, pattern) in &rules {
        let (url, queue_arn) =
            given_scoped_queue(&router, &signer, case, queue, &rule_arn(rule)).await;
        given_rule(&router, &signer, case, rule, pattern, &queue_arn).await;
        urls.push(url);
    }

    for (name, family) in [("badservice", "badfamily"), ("goodservice", "goodfamily")] {
        create_service(
            &router,
            &signer,
            case,
            &ServiceSpec {
                cluster,
                name,
                task_definition: family,
                subnet: &subnet,
                breaker: false,
            },
        )
        .await;
    }

    // ONLY the queues are polled: a client that watches a service through EventBridge makes no
    // further Amazon ECS call, so the actions have to arrive with nobody asking Amazon ECS.
    let mut bad_held: Vec<Value> = Vec::new();
    let mut good_held: Vec<Value> = Vec::new();
    let mut near_held: Vec<Value> = Vec::new();
    let started = Instant::now();
    while started.elapsed() < DELIVERY_WAIT
        && !(holds(&bad_held, TASK_CONFIGURATION_FAILURE) && holds(&good_held, STEADY_STATE))
    {
        bad_held.extend(drain(&router, &signer, case, &urls[0]).await);
        good_held.extend(drain(&router, &signer, case, &urls[1]).await);
        near_held.extend(drain(&router, &signer, case, &urls[2]).await);
        tokio::time::sleep(LOOK_INTERVAL).await;
    }

    let failures = named(&bad_held, TASK_CONFIGURATION_FAILURE);
    assert!(
        !failures.is_empty(),
        "case {case:?}: expected the scheduler's {TASK_CONFIGURATION_FAILURE} for \
         {bad_service} on its own rule's queue, got {bad_held:?}"
    );
    let cluster_arn = format!("arn:aws:ecs:{REGION}:{ACCOUNT}:cluster/{cluster}");
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
        // WHY the launch could not be made: the account the container runtime gives of the
        // image it could not fetch, naming the reference the revision asked for.
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

    // The healthy sibling's own rule receives its steady state, scoped to its own ARN, and
    // reports no reason: an action that reports progress explains nothing.
    let steady = named(&good_held, STEADY_STATE);
    assert!(
        !steady.is_empty(),
        "case {case:?}: expected {STEADY_STATE} for {good_service} on its own rule's queue, \
         got {good_held:?}"
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
        assert_eq!(
            envelope.pointer("/detail/reason"),
            None,
            "case {case:?}: a steady state explains nothing, got {envelope}"
        );
    }

    // The NEGATIVE half, which is what the per-service scoping is for: the near-miss rule lists
    // the failure in its `detail.eventName` and is scoped to the healthy service, so it
    // receives that service's steady state and none of the sibling's failures.
    assert!(
        holds(&near_held, STEADY_STATE),
        "case {case:?}: expected the near-miss rule to receive {STEADY_STATE} of \
         {good_service}, got {near_held:?}"
    );
    for (label, held) in [
        ("the near-miss rule", &near_held),
        ("the healthy service's rule", &good_held),
    ] {
        let leaked = named(held, TASK_CONFIGURATION_FAILURE);
        assert!(
            leaked.is_empty(),
            "case {case:?}: {label} must receive no failure of {bad_service}, got {leaked:?}"
        );
    }
    let misrouted = named(&bad_held, STEADY_STATE);
    assert!(
        misrouted.is_empty(),
        "case {case:?}: the failing service reaches no steady state, got {misrouted:?}"
    );
}

/// RULE (the `ECS Deployment State Change` half): a release publishes every state its rollout
/// moves into, naming the deployment `DescribeServices` reports — its start and its completion
/// as progress, and the state the deployment circuit breaker leaves a release that cannot launch
/// its revision as an error, with the reason the API reports for it — and the rollback that
/// breaker performs takes the service back to the revision that completed, inside one client's
/// wait.
#[tokio::test]
async fn a_release_publishes_its_rollout_states_and_its_rollback() {
    let case = "a release whose revision cannot be launched";
    let cluster = "releasescope";
    let service = "releaseservice";
    let (router, signer) = emulator();

    let subnet = public_subnet(&router, &signer, case).await;
    ecs(
        &router,
        &signer,
        case,
        "CreateCluster",
        &json!({"clusterName": cluster}),
    )
    .await;
    let good_revision = register(&router, &signer, case, "releasefamily", PULLABLE_IMAGE).await;
    let bad_revision = register(&router, &signer, case, "releasefamily", UNPULLABLE_IMAGE).await;

    let arn = service_arn(cluster, service);
    let (url, queue_arn) = given_scoped_queue(
        &router,
        &signer,
        case,
        "deployment-events-queue",
        &rule_arn("deployment-states"),
    )
    .await;
    given_rule(
        &router,
        &signer,
        case,
        "deployment-states",
        &json!({
            "source": [EVENT_SOURCE],
            "detail-type": [DEPLOYMENT_STATE_CHANGE],
            "resources": [&arn],
        }),
        &queue_arn,
    )
    .await;

    create_service(
        &router,
        &signer,
        case,
        &ServiceSpec {
            cluster,
            name: service,
            task_definition: &good_revision,
            subnet: &subnet,
            breaker: true,
        },
    )
    .await;

    // The release of a revision that CAN be launched: its rollout starts and completes, and
    // both moves are published naming the deployment the API reports.
    let settled = poll_until(
        || describe(&router, &signer, case, cluster, service),
        |service| primary(service)["rolloutState"] == json!(COMPLETED),
    )
    .await;
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
    while started.elapsed() < DELIVERY_WAIT
        && !(holds(&held, DEPLOYMENT_IN_PROGRESS) && holds(&held, DEPLOYMENT_COMPLETED))
    {
        held.extend(drain(&router, &signer, case, &url).await);
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    for event_name in [DEPLOYMENT_IN_PROGRESS, DEPLOYMENT_COMPLETED] {
        let delivered = named(&held, event_name);
        assert!(
            !delivered.is_empty(),
            "case {case:?}: expected {event_name} of {arn} on the rule's queue, got {held:?}"
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

    // The release of the revision that CANNOT be launched: the circuit breaker gives that
    // rollout the terminal state of the state machine, which is published as an error naming
    // the deployment the API reports as failed.
    ecs(
        &router,
        &signer,
        case,
        "UpdateService",
        &json!({"cluster": cluster, "service": service, "taskDefinition": &bad_revision}),
    )
    .await;
    let releasing = poll_until(
        || describe(&router, &signer, case, cluster, service),
        |service| {
            let deployment = primary(service);
            deployment["taskDefinition"] == json!(&bad_revision)
                && [IN_PROGRESS, FAILED]
                    .iter()
                    .any(|state| deployment["rolloutState"] == json!(state))
        },
    )
    .await;
    let failing = primary(&releasing)["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_ne!(
        failing, first_deployment,
        "case {case:?}: the release is a new deployment of its own, got {releasing}"
    );
    let failed = poll_until(
        || describe(&router, &signer, case, cluster, service),
        |service| deployment(service, &failing)["rolloutState"] == json!(FAILED),
    )
    .await;
    let reported = deployment(&failed, &failing);
    assert_eq!(
        reported["rolloutState"],
        json!(FAILED),
        "case {case:?}: expected the release to be reported {FAILED}, got {failed}"
    );
    let why = reported["rolloutStateReason"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        why.contains("circuit breaker"),
        "case {case:?}: expected the rollout state reason to name the circuit breaker, got \
         {why:?}"
    );
    let started = Instant::now();
    while started.elapsed() < DELIVERY_WAIT && !holds(&held, DEPLOYMENT_FAILED) {
        held.extend(drain(&router, &signer, case, &url).await);
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    let failures = named(&held, DEPLOYMENT_FAILED);
    assert!(
        !failures.is_empty(),
        "case {case:?}: expected {DEPLOYMENT_FAILED} of {arn} on the rule's queue, got {held:?}"
    );
    for envelope in &failures {
        for (pointer, expected) in [
            ("/resources", json!([&arn])),
            ("/detail/eventType", json!(ERROR)),
            ("/detail/deploymentId", json!(&failing)),
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
        assert_eq!(
            reason, why,
            "case {case:?}: the event and the API must agree about why the release failed, got \
             {envelope}"
        );
    }

    // And where the breaker's ROLLBACK leaves the service: back on the revision that
    // completed, with that redeployment itself reported complete.
    let rolled_back = poll_until(
        || describe(&router, &signer, case, cluster, service),
        |service| {
            let deployment = primary(service);
            deployment["taskDefinition"] == json!(&good_revision)
                && deployment["rolloutState"] == json!(COMPLETED)
        },
    )
    .await;
    let restored = primary(&rolled_back);
    assert_eq!(
        restored["taskDefinition"],
        json!(&good_revision),
        "case {case:?}: expected the rollback to put {good_revision} back, got {rolled_back}"
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
