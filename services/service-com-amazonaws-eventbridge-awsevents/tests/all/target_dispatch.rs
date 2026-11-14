// SPDX-License-Identifier: CC0-1.0

//! `PutEvents` INVOKES each target of every rule that matched, end to end over
//! the wire through the real `MultiServiceRouter`, with the services a target
//! lives in mounted beside EventBridge on one engine — so a delivery is observed
//! where a client observes it: on the queue, on the topic, on the state machine's
//! executions, and on the dead-letter queue and the `FailedInvocations` metric
//! when the delivery fails.
//!
//! The RULE asserted here: for every ENABLED rule whose `EventPattern` matches a
//! published event, EventBridge makes the API call that target's service is
//! invoked by — `sqs:SendMessage` for a queue, `sns:Publish` for a topic,
//! `states:StartExecution` for a state machine, `lambda:InvokeFunction` for a
//! function — carrying the whole event envelope. For the target kinds AWS invokes
//! UNDER the target's `RoleArn` ("for EC2 instances, Kinesis data streams, Step
//! Functions state machines and API Gateway APIs, EventBridge relies on IAM roles
//! that you specify in the `RoleARN` argument in `PutTargets`. For Lambda
//! functions, Amazon SNS topics and Amazon SQS queues, EventBridge relies on
//! resource-based policies") a role that `events.amazonaws.com` cannot assume, or
//! whose policies withhold that call, is a delivery that fails: the event goes to
//! the target's dead-letter queue and is counted on the rule's
//! `FailedInvocations` metric instead.
//!
//! The tables span the class rather than one recorded case: the rows differ along
//! the target's SERVICE (which invocation is made and what the target then
//! holds), and along WHY the role bars the delivery — no trust for the service
//! principal, a policy naming another action, a policy stating nothing at all, a
//! policy that grants the call, a role the account does not hold (which is
//! evidence of nothing, so the delivery goes ahead), and a target kind whose
//! own CALL the role's permissions do not govern — though the assume-role step is
//! reached by every target that NAMES a role, whatever its kind.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::{
    ComAmazonawsEventbridgeAwseventsState, InvocationOutcome, DEFAULT_EVENT_BUS_NAME,
    EVENT_BUS_NAME_DIMENSION, FAILED_INVOCATIONS_METRIC, INVOCATIONS_METRIC, MATCHED_EVENTS_METRIC,
    METRIC_NAMESPACE, RULE_NAME_DIMENSION, SERVICE_PRINCIPAL,
    SUCCESSFUL_INVOCATION_ATTEMPTS_METRIC, TRIGGERED_RULES_METRIC,
};
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    partition_key_hash, ACTIVATION_DELAY, SHARD_ITERATOR_TYPE_TRIM_HORIZON,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
pub(crate) const REGION: &str = "eu-west-2";

/// The source and detail type the rules below match on.
pub(crate) const SOURCE: &str = "com.demo.orders";
pub(crate) const DETAIL_TYPE: &str = "Order Created";
const DETAIL: &str = r#"{"orderId":"A-1","amount":7}"#;

/// A state machine definition that terminates on its own, so an execution
/// started by a delivery is a real execution rather than one waiting on a task.
const DEFINITION: &str = r#"{"StartAt":"Done","States":{"Done":{"Type":"Succeed"}}}"#;

/// The emulator: EventBridge and every service a target of these tests lives in,
/// on ONE engine — the shape `core/router/src/lib.rs` mounts them in.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

pub(crate) fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_sfn_awsstepfunctions::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_kinesis_kinesis_20131202::EmulatedService::new(Arc::clone(&engine)),
    );
    // Glue: a rule target that is a workflow is delivered to by counting the event
    // against that workflow's EVENT trigger, and the run it starts is read back through
    // Glue's own `GetWorkflowRuns`.
    router.register_service(
        service_com_amazonaws_glue_awsglue::EmulatedService::new(Arc::clone(&engine)),
    );
    // KMS: the key an event bus is encrypted with lives in that service's own store,
    // and a publication onto such a bus is decided against the state its handlers
    // (`DisableKey`/`EnableKey`) left on the key.
    router.register_service(
        service_com_amazonaws_kms_trentservice::EmulatedService::new(Arc::clone(&engine)),
    );
    // CloudWatch Logs: a rule target that is a log group is delivered to by writing a
    // log event into that group, and the write is read back through that service's own
    // `FilterLogEvents` — the resource policies that decide the write live there too.
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    Emulator { router, engine }
}

/// The access key id every call below presents unless a suite names another one: the
/// account holds no record of it, so it resolves to the ambient identity — the account's
/// own published credential.
pub(crate) const AMBIENT_KEY: &str = "AKIDTESTTESTTEST";

/// One `awsJson` RPC call against the service whose signing name is
/// `signing_name` — the credential scope is what routes it.
pub(crate) async fn rpc(
    emulator: &Emulator,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    rpc_as(
        emulator,
        AMBIENT_KEY,
        signing_name,
        target_prefix,
        json_version,
        operation,
        body,
    )
    .await
}

/// [`rpc`] presenting `access_key_id`, which is what selects the principal the request
/// speaks for: a key `iam:CreateAccessKey` minted speaks for the user it was minted for.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn rpc_as(
    emulator: &Emulator,
    access_key_id: &str,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header(
            "content-type",
            format!("application/x-amz-json-{json_version}"),
        )
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/{signing_name}/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&payload).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&payload)
            )
        })
    };
    (status, parsed)
}

pub(crate) async fn events(
    emulator: &Emulator,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    rpc(emulator, "events", "AWSEvents", "1.1", operation, body).await
}

pub(crate) async fn sqs(emulator: &Emulator, operation: &str, body: &Value) -> (u16, Value) {
    rpc(emulator, "sqs", "AmazonSQS", "1.0", operation, body).await
}

/// An EventBridge call presenting `access_key_id` rather than the ambient key.
pub(crate) async fn events_as(
    emulator: &Emulator,
    access_key_id: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    rpc_as(
        emulator,
        access_key_id,
        "events",
        "AWSEvents",
        "1.1",
        operation,
        body,
    )
    .await
}

pub(crate) async fn sfn(emulator: &Emulator, operation: &str, body: &Value) -> (u16, Value) {
    rpc(
        emulator,
        "states",
        "AWSStepFunctions",
        "1.0",
        operation,
        body,
    )
    .await
}

pub(crate) async fn cloudwatch(emulator: &Emulator, operation: &str, body: &Value) -> (u16, Value) {
    rpc(
        emulator,
        "monitoring",
        "GraniteServiceVersion20100801",
        "1.0",
        operation,
        body,
    )
    .await
}

/// One Kinesis call, asserting a 200 and answering the body: the data plane a
/// stream target's records are read back from (`Kinesis_20131202`, awsJson1_1).
pub(crate) async fn kinesis(
    emulator: &Emulator,
    case: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let (status, answer) = rpc(
        emulator,
        "kinesis",
        "Kinesis_20131202",
        "1.1",
        operation,
        body,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: {operation} answered {status} {answer}"
    );
    answer
}

/// Publish one entry per `details` text, each with the source and detail type the
/// rules here match on, and answer the `EventId` of each in the request's order.
pub(crate) async fn publish_details(emulator: &Emulator, details: &[String]) -> Vec<String> {
    let entries: Vec<Value> = details
        .iter()
        .map(|detail| json!({ "Source": SOURCE, "DetailType": DETAIL_TYPE, "Detail": detail }))
        .collect();
    let (status, body) = events(emulator, "PutEvents", &json!({ "Entries": entries })).await;
    assert_eq!(status, 200, "PutEvents answered {status} {body}");
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "PutEvents reported a failed entry: {body}"
    );
    body["Entries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            entry["EventId"]
                .as_str()
                .unwrap_or_else(|| panic!("no EventId in {entry}"))
                .to_string()
        })
        .collect()
}

/// Percent-encode one form value the way an AWS SDK encodes an `awsQuery` body.
pub(crate) fn form_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// One `awsQuery` call against the service whose signing name is `signing_name`.
async fn query(
    emulator: &Emulator,
    signing_name: &str,
    api_version: &str,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={api_version}");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{REGION}/{signing_name}/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    (status, body)
}

pub(crate) async fn iam(
    emulator: &Emulator,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    query(emulator, "iam", "2010-05-08", action, params).await
}

pub(crate) async fn sns(
    emulator: &Emulator,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    query(emulator, "sns", "2010-03-31", action, params).await
}

/// The text of the first `<tag>` of an XML body.
pub(crate) fn xml_text(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(xml[start..end].to_string())
}

/// A queue called `name`, carrying the policy a rule's target needs
/// ([`granting_events`]); answers its url and the ARN a rule names it by.
///
/// The grant is part of the PREREQUISITE because "for Lambda functions, Amazon SNS
/// topics and Amazon SQS queues, EventBridge relies on resource-based policies"
/// (`PutTargets`): a queue that has granted nothing receives nothing, which is the
/// rule `tests/all/queue_target_policy_authorization.rs` is the suite for. The suites
/// here are about what a delivery CARRIES and which targets it reaches, so their
/// queues grant the send.
pub(crate) async fn given_queue(emulator: &Emulator, name: &str) -> (String, String) {
    given_queue_with_policy(emulator, name, Some(&granting_events)).await
}

/// The queue policy a rule's SQS target needs for EventBridge to deliver to it: the
/// service principal allowed `sqs:SendMessage` on that queue, unconditionally.
pub(crate) fn granting_events(queue_arn: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AllowEventBridge",
            "Effect": "Allow",
            "Principal": {"Service": SERVICE_PRINCIPAL},
            "Action": "sqs:SendMessage",
            "Resource": queue_arn,
        }],
    })
    .to_string()
}

/// A queue called `name` carrying the document `policy` builds from the queue's own
/// ARN, or none at all when `policy` is `None`; answers its url and that ARN.
pub(crate) async fn given_queue_with_policy(
    emulator: &Emulator,
    name: &str,
    policy: Option<&dyn Fn(&str) -> String>,
) -> (String, String) {
    let (status, body) = sqs(emulator, "CreateQueue", &json!({ "QueueName": name })).await;
    assert_eq!(status, 200, "case {name}: CreateQueue answered {status} {body}");
    let url = body["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: no QueueUrl in {body}"))
        .to_string();
    let (status, body) = sqs(
        emulator,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name}: GetQueueAttributes answered {status} {body}"
    );
    let arn = body["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: no QueueArn in {body}"))
        .to_string();
    if let Some(policy) = policy {
        let (status, body) = sqs(
            emulator,
            "SetQueueAttributes",
            &json!({ "QueueUrl": url, "Attributes": { "Policy": policy(&arn) } }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name}: SetQueueAttributes answered {status} {body}"
        );
    }
    (url, arn)
}

/// Every message waiting on the queue at `url`, with its attributes.
pub(crate) async fn drain(emulator: &Emulator, url: &str) -> Vec<Value> {
    let (status, body) = sqs(
        emulator,
        "ReceiveMessage",
        &json!({
            "QueueUrl": url,
            "MaxNumberOfMessages": 10,
            "MessageAttributeNames": ["All"],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "receive {url}: ReceiveMessage answered {status} {body}"
    );
    body["Messages"].as_array().cloned().unwrap_or_default()
}

/// A rule called `name` matching the published event, carrying `targets`.
pub(crate) async fn given_rule(emulator: &Emulator, name: &str, targets: Value) {
    let pattern = json!({ "source": [SOURCE], "detail-type": [DETAIL_TYPE] }).to_string();
    let (status, body) = events(
        emulator,
        "PutRule",
        &json!({ "Name": name, "EventPattern": pattern, "State": "ENABLED" }),
    )
    .await;
    assert_eq!(status, 200, "case {name}: PutRule answered {status} {body}");
    let (status, body) = events(
        emulator,
        "PutTargets",
        &json!({ "Rule": name, "Targets": targets }),
    )
    .await;
    assert_eq!(status, 200, "case {name}: PutTargets answered {status} {body}");
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "case {name}: PutTargets reported a failed entry: {body}"
    );
}

/// Publish the one event every test below routes, and answer its `EventId`.
pub(crate) async fn publish(emulator: &Emulator) -> String {
    let (status, body) = events(
        emulator,
        "PutEvents",
        &json!({
            "Entries": [{ "Source": SOURCE, "DetailType": DETAIL_TYPE, "Detail": DETAIL }],
        }),
    )
    .await;
    assert_eq!(status, 200, "PutEvents answered {status} {body}");
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "PutEvents reported a failed entry: {body}"
    );
    body["Entries"][0]["EventId"]
        .as_str()
        .unwrap_or_else(|| panic!("no EventId in {body}"))
        .to_string()
}

/// What EventBridge recorded about each handover: `(rule, target arn, outcome)`.
pub(crate) fn invocations(emulator: &Emulator) -> Vec<(String, String, InvocationOutcome)> {
    emulator
        .engine
        .state::<ComAmazonawsEventbridgeAwseventsState>()
        .lock()
        .invocations
        .iter()
        .map(|invocation| {
            (
                invocation.rule.clone(),
                invocation.target_arn.clone(),
                invocation.outcome.clone(),
            )
        })
        .collect()
}

/// The envelope the published event is delivered as, with the values this
/// publication fixes. Source: the EventBridge event-structure reference (and the
/// `PutEvents` entry members the event is built from).
fn assert_whole_envelope(case: &str, body: &str, event_id: &str) {
    let parsed: Value = serde_json::from_str(body)
        .unwrap_or_else(|error| panic!("case {case}: the delivered body is not JSON ({error})"));
    let rows = [
        ("version", json!("0")),
        ("id", json!(event_id)),
        ("detail-type", json!(DETAIL_TYPE)),
        ("source", json!(SOURCE)),
        ("account", json!(engine::emulator_account_id())),
        ("region", json!(REGION)),
        (
            "detail",
            serde_json::from_str::<Value>(DETAIL).expect("the detail is JSON"),
        ),
    ];
    for (member, expected) in rows {
        assert_eq!(
            parsed[member], expected,
            "case {case}: expected the envelope's {member} to be {expected}, got {parsed}"
        );
    }
}

/// The target kinds a rule's matched event is invoked on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Queue,
    Topic,
    StateMachine,
}

/// A matched event is handed to the target's OWN API, for every target kind this
/// emulator invokes — and what the target then holds is the whole event
/// envelope.
#[tokio::test]
async fn a_matched_event_is_invoked_on_every_target_kind() {
    for kind in [Kind::Queue, Kind::Topic, Kind::StateMachine] {
        let emulator = emulator();
        let (target_arn, observe) = given_target(&emulator, kind).await;
        let mut target = json!({ "Id": "t1", "Arn": target_arn });
        // A state machine is invoked AS the role the target names ("for EC2
        // instances, Kinesis Data Streams, Step Functions state machines and API
        // Gateway APIs, EventBridge relies on IAM roles that you specify in the
        // RoleARN argument in PutTargets"), so that row names one; a queue and a
        // topic are invoked under their own resource-based policies.
        if kind == Kind::StateMachine {
            target["RoleArn"] = json!(format!(
                "arn:aws:iam::{}:role/events-invoke",
                engine::emulator_account_id()
            ));
        }
        given_rule(&emulator, "dispatch", json!([target])).await;

        let event_id = publish(&emulator).await;

        let recorded = invocations(&emulator);
        assert_eq!(
            recorded,
            vec![(
                "dispatch".to_string(),
                target_arn.clone(),
                InvocationOutcome::Invoked
            )],
            "case {kind:?}: expected one invoked handover, got {recorded:?}"
        );
        let delivered = observe.read(&emulator).await;
        assert_eq!(
            delivered.len(),
            1,
            "case {kind:?}: expected the target to hold the event exactly once, got {delivered:?}"
        );
        assert_whole_envelope(&format!("{kind:?}"), &delivered[0], &event_id);
    }
}

/// How a row reads back what its target received.
enum Observe {
    /// The bodies of the messages on this queue.
    Queue(String),
    /// The messages SNS delivered to this topic's subscriptions.
    Topic(String),
    /// The inputs of this state machine's executions.
    StateMachine(String),
}

impl Observe {
    async fn read(&self, emulator: &Emulator) -> Vec<String> {
        match self {
            Self::Queue(url) => drain(emulator, url)
                .await
                .iter()
                .map(|message| message["Body"].as_str().unwrap_or_default().to_string())
                .collect(),
            Self::Topic(arn) => emulator
                .engine
                .state::<service_com_amazonaws_sns_amazonsimplenotificationservice::ComAmazonawsSnsAmazonsimplenotificationserviceState>()
                .lock()
                .delivered
                .values()
                .flatten()
                .filter(|published| &published.topic_arn == arn)
                .map(|published| published.message.to_string())
                .collect(),
            Self::StateMachine(arn) => {
                let (status, listed) = sfn(
                    emulator,
                    "ListExecutions",
                    &json!({ "stateMachineArn": arn }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "ListExecutions answered {status} {listed}"
                );
                let mut inputs = Vec::new();
                for execution in listed["executions"].as_array().cloned().unwrap_or_default() {
                    let execution_arn = execution["executionArn"].as_str().unwrap_or_default();
                    let (status, described) = sfn(
                        emulator,
                        "DescribeExecution",
                        &json!({ "executionArn": execution_arn }),
                    )
                    .await;
                    assert_eq!(
                        status, 200,
                        "DescribeExecution answered {status} {described}"
                    );
                    inputs.push(described["input"].as_str().unwrap_or_default().to_string());
                }
                inputs
            }
        }
    }
}

/// Create the resource a row's target names, and answer its ARN plus how to read
/// back what it received.
async fn given_target(emulator: &Emulator, kind: Kind) -> (String, Observe) {
    match kind {
        Kind::Queue => {
            let (url, arn) = given_queue(emulator, "dispatch-queue").await;
            (arn, Observe::Queue(url))
        }
        Kind::Topic => {
            let (status, created) = sns(emulator, "CreateTopic", &[("Name", "dispatch-topic")]).await;
            assert_eq!(status, 200, "CreateTopic answered {status}: {created}");
            let arn = xml_text(&created, "TopicArn")
                .unwrap_or_else(|| panic!("no TopicArn in {created}"));
            // The grant is part of the PREREQUISITE, for the reason [`given_queue`]
            // gives: "for Lambda functions, Amazon SNS topics and Amazon SQS queues,
            // EventBridge relies on resource-based policies", so a topic that has
            // granted this service nothing is published nothing — which is the rule
            // `tests/all/topic_target_policy_authorization.rs` is the suite for.
            let policy = json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Sid": "AllowEventBridge",
                    "Effect": "Allow",
                    "Principal": {"Service": SERVICE_PRINCIPAL},
                    "Action": "sns:Publish",
                    "Resource": arn,
                }],
            })
            .to_string();
            let (status, written) = sns(
                emulator,
                "SetTopicAttributes",
                &[
                    ("TopicArn", &arn),
                    ("AttributeName", "Policy"),
                    ("AttributeValue", &policy),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "SetTopicAttributes answered {status}: {written}"
            );
            // A delivery is observed on a SUBSCRIPTION, so the topic has one.
            let (_, queue_arn) = given_queue(emulator, "topic-subscriber").await;
            let (status, subscribed) = sns(
                emulator,
                "Subscribe",
                &[
                    ("TopicArn", &arn),
                    ("Protocol", "sqs"),
                    ("Endpoint", &queue_arn),
                ],
            )
            .await;
            assert_eq!(status, 200, "Subscribe answered {status}: {subscribed}");
            (arn.clone(), Observe::Topic(arn))
        }
        Kind::StateMachine => {
            let arn = given_state_machine(emulator, "dispatch-flow").await;
            (arn.clone(), Observe::StateMachine(arn))
        }
    }
}

/// A state machine called `name`, answered with its ARN.
pub(crate) async fn given_state_machine(emulator: &Emulator, name: &str) -> String {
    let role = format!(
        "arn:aws:iam::{}:role/state-machine-execution",
        engine::emulator_account_id()
    );
    let (status, created) = sfn(
        emulator,
        "CreateStateMachine",
        &json!({ "name": name, "definition": DEFINITION, "roleArn": role }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name}: CreateStateMachine answered {status} {created}"
    );
    created["stateMachineArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: no stateMachineArn in {created}"))
        .to_string()
}

/// A trust policy with one statement, written as a client sends it.
pub(crate) fn trust_policy(principal: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": principal },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// An identity policy allowing one action on every resource.
fn identity_policy(action: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": action, "Resource": "*" }],
    })
    .to_string()
}

/// A role trusting `principal`, carrying an inline policy allowing `allowed`
/// when a row names one; answered with the ARN IAM persisted.
pub(crate) async fn given_role(
    emulator: &Emulator,
    name: &str,
    principal: &str,
    allowed: Option<&str>,
) -> String {
    let document = trust_policy(principal);
    let (status, created) = iam(
        emulator,
        "CreateRole",
        &[("RoleName", name), ("AssumeRolePolicyDocument", &document)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name}: CreateRole answered {status}: {created}"
    );
    let arn =
        xml_text(&created, "Arn").unwrap_or_else(|| panic!("case {name}: no Arn in {created}"));
    if let Some(action) = allowed {
        let document = identity_policy(action);
        let (status, written) = iam(
            emulator,
            "PutRolePolicy",
            &[
                ("RoleName", name),
                ("PolicyName", "delivery"),
                ("PolicyDocument", &document),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name}: PutRolePolicy answered {status}: {written}"
        );
    }
    arn
}

/// Epoch seconds, for the metric window a statistics read asks about.
fn epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// How long the deliveries of one publication are given to reach a stream before
/// the records that never arrived are a failed assertion, and how long a read
/// sleeps before trying again.
const DELIVERY_BUDGET: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// One shard of a stream: its id and the slice of the 128-bit hash key space it
/// covers, as `ListShards` publishes them.
pub(crate) struct Shard {
    pub(crate) id: String,
    starting: u128,
    ending: u128,
}

/// One record a shard served: the shard it came from, its partition key and its
/// data as text.
#[derive(Debug, Clone)]
pub(crate) struct Held {
    pub(crate) shard: String,
    pub(crate) key: String,
    pub(crate) data: String,
}

/// A stream called `stream` of `shard_count` shards, past the transition its
/// creation armed; answered with the ARN a target names it by.
pub(crate) async fn given_stream(
    emulator: &Emulator,
    case: &str,
    stream: &str,
    shard_count: i64,
) -> String {
    kinesis(
        emulator,
        case,
        "CreateStream",
        &json!({ "StreamName": stream, "ShardCount": shard_count }),
    )
    .await;
    tokio::time::sleep(ACTIVATION_DELAY + Duration::from_millis(400)).await;
    kinesis(
        emulator,
        case,
        "DescribeStreamSummary",
        &json!({ "StreamName": stream }),
    )
    .await["StreamDescriptionSummary"]["StreamARN"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: DescribeStreamSummary reported no StreamARN"))
        .to_string()
}

/// Every shard of `stream`, with the hash key range each covers; `expected` is the
/// map the stream was created with.
pub(crate) async fn shard_map(
    emulator: &Emulator,
    case: &str,
    stream: &str,
    expected: i64,
) -> Vec<Shard> {
    let listed = kinesis(emulator, case, "ListShards", &json!({ "StreamName": stream })).await;
    let described = listed["Shards"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        described.len(),
        usize::try_from(expected).expect("the shard count fits a usize"),
        "case {case}: expected the stream's {expected} shards, got {listed}"
    );
    described
        .iter()
        .map(|shard| {
            let bound = |member: &str| {
                shard["HashKeyRange"][member]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case}: no {member} on {shard}"))
                    .parse::<u128>()
                    .unwrap_or_else(|error| {
                        panic!("case {case}: {member} is not a hash key ({error}): {shard}")
                    })
            };
            Shard {
                id: shard["ShardId"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case}: a shard carries no ShardId: {shard}"))
                    .to_string(),
                starting: bound("StartingHashKey"),
                ending: bound("EndingHashKey"),
            }
        })
        .collect()
}

/// The shard of `shards` a record keyed on `partition_key` belongs on: the one
/// whose published range covers that key's hash ("an MD5 hash function is used to
/// map partition keys to 128-bit integer values and to map associated data records
/// to shards", `PutRecordInput$PartitionKey`).
pub(crate) fn covering_shard(shards: &[Shard], partition_key: &str) -> Option<String> {
    let hash = partition_key_hash(partition_key);
    shards
        .iter()
        .find(|shard| (shard.starting..=shard.ending).contains(&hash))
        .map(|shard| shard.id.clone())
}

/// Read every shard of `stream` from its oldest retained record and give back what
/// they served, each record WITH the shard it sat on: `GetShardIterator` at the
/// trim horizon, then one `GetRecords`.
pub(crate) async fn read_shards(
    emulator: &Emulator,
    case: &str,
    stream: &str,
    shards: &[Shard],
) -> Vec<Held> {
    let mut held = Vec::new();
    for shard in shards {
        let iterator = kinesis(
            emulator,
            case,
            "GetShardIterator",
            &json!({
                "StreamName": stream,
                "ShardId": shard.id,
                "ShardIteratorType": SHARD_ITERATOR_TYPE_TRIM_HORIZON,
            }),
        )
        .await["ShardIterator"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: no ShardIterator for {}", shard.id))
            .to_string();
        let answer = kinesis(
            emulator,
            case,
            "GetRecords",
            &json!({ "ShardIterator": iterator, "Limit": 100 }),
        )
        .await;
        for record in answer["Records"].as_array().cloned().unwrap_or_default() {
            let encoded = record["Data"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: a record carries no Data: {record}"));
            let data = aws_smithy_types::base64::decode(encoded).unwrap_or_else(|error| {
                panic!("case {case}: a record's Data is not base64 ({error}): {record}")
            });
            held.push(Held {
                shard: shard.id.clone(),
                key: record["PartitionKey"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case}: a record carries no PartitionKey: {record}")
                    })
                    .to_string(),
                data: String::from_utf8_lossy(&data).to_string(),
            });
        }
    }
    held
}

/// Everything `stream` holds once it holds `want` records, so a delivery still in
/// flight is read again rather than read as missing; gives up after
/// [`DELIVERY_BUDGET`] so a delivery that never arrives fails an assertion instead
/// of hanging.
pub(crate) async fn read_shards_until(
    emulator: &Emulator,
    case: &str,
    stream: &str,
    shards: &[Shard],
    want: usize,
) -> Vec<Held> {
    let deadline = Instant::now() + DELIVERY_BUDGET;
    loop {
        let held = read_shards(emulator, case, stream, shards).await;
        if held.len() >= want || Instant::now() >= deadline {
            return held;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The dimensions a rule's metrics are reported under where the rule's WHOLE
/// identity is named: the bus the rule is on and the rule's name.
pub(crate) fn rule_dimensions(bus: &str, rule: &str) -> Value {
    json!([
        { "Name": EVENT_BUS_NAME_DIMENSION, "Value": bus },
        { "Name": RULE_NAME_DIMENSION, "Value": rule },
    ])
}

/// The other dimension set a HANDOVER counter of the same rule is reported
/// under: the rule's name on its own.
pub(crate) fn rule_name_dimensions(rule: &str) -> Value {
    json!([{ "Name": RULE_NAME_DIMENSION, "Value": rule }])
}

/// The dimensions a reader names when it asks for a bus-wide series: the event
/// bus on its own, with no rule.
pub(crate) fn bus_dimensions(bus: &str) -> Value {
    json!([{ "Name": EVENT_BUS_NAME_DIMENSION, "Value": bus }])
}

/// The AWS/Events counters EventBridge publishes under the rule's WHOLE identity
/// and under nothing looser: the two that count what a bus's routing table did
/// with an event.
///
/// Stated HERE, in the test, rather than read from the service: a golden that
/// asked the service which dimension sets it publishes under would move with a
/// regression instead of catching it.
pub(crate) const ROUTING_COUNTERS: [&str; 2] = [MATCHED_EVENTS_METRIC, TRIGGERED_RULES_METRIC];

/// The AWS/Events counters EventBridge publishes under the rule's whole identity
/// AND under the rule name on its own: the three that count what became of a
/// handover. Stated here for the same reason as [`ROUTING_COUNTERS`].
pub(crate) const HANDOVER_COUNTERS: [&str; 3] = [
    INVOCATIONS_METRIC,
    SUCCESSFUL_INVOCATION_ATTEMPTS_METRIC,
    FAILED_INVOCATIONS_METRIC,
];

/// Whether `metric` is one of the counters published under the rule name ALONE
/// beside the rule's whole identity, by the test's own table above. Panics for a
/// counter neither table names, so a new counter cannot slip past these goldens.
pub(crate) fn published_under_rule_name_alone(metric: &str) -> bool {
    assert!(
        ROUTING_COUNTERS.contains(&metric) || HANDOVER_COUNTERS.contains(&metric),
        "the counter {metric} is in neither the routing nor the handover table"
    );
    HANDOVER_COUNTERS.contains(&metric)
}

/// Every series the metrics `metrics` of the rule `rule` on `bus` are published
/// as: the rule's whole identity for every counter, plus the rule name alone for
/// a handover counter, by the test's OWN table
/// ([`ROUTING_COUNTERS`] / [`HANDOVER_COUNTERS`]).
pub(crate) fn rule_series(bus: &str, rule: &str, metrics: &[&str]) -> Vec<Value> {
    let mut series = Vec::new();
    for metric in metrics {
        let mut dimension_sets = vec![rule_dimensions(bus, rule)];
        if published_under_rule_name_alone(metric) {
            dimension_sets.push(rule_name_dimensions(rule));
        }
        for dimensions in dimension_sets {
            series.push(json!({
                "Namespace": METRIC_NAMESPACE,
                "MetricName": metric,
                "Dimensions": dimensions,
            }));
        }
    }
    series
}

/// `series` rendered in a stable order, so a listing is compared as the SET of
/// series it is.
pub(crate) fn ordered(series: &[Value]) -> Vec<String> {
    let mut rendered: Vec<String> = series.iter().map(Value::to_string).collect();
    rendered.sort();
    rendered
}

/// The `Sum` of `metric` for the rule `rule` of the bus `bus`, over a window
/// around now, in EventBridge's own namespace.
pub(crate) async fn metric_sum(
    emulator: &Emulator,
    metric: &str,
    bus: &str,
    rule: &str,
) -> f64 {
    metric_sum_over(emulator, metric, &rule_dimensions(bus, rule)).await
}

/// The `Sum` of `metric` over the ONE series `dimensions` names, over a window
/// around now, in EventBridge's own namespace.
pub(crate) async fn metric_sum_over(
    emulator: &Emulator,
    metric: &str,
    dimensions: &Value,
) -> f64 {
    let now = epoch_seconds();
    metric_sum_in_window(emulator, metric, dimensions, now - 300.0, now + 300.0, 600).await
}

/// The same sum read the way a client watching a LIVE series reads it: the one
/// `Period`-wide minute ending now, so the read is aligned to the period it asks
/// for instead of spanning a window far wider than the series.
pub(crate) async fn metric_sum_over_last_minute(
    emulator: &Emulator,
    metric: &str,
    dimensions: &Value,
) -> f64 {
    let now = epoch_seconds();
    metric_sum_in_window(emulator, metric, dimensions, now - 60.0, now, 60).await
}

/// The `Sum` of `metric` for the series `dimensions` names over
/// `[start, end)`, folded into `period`-wide windows.
async fn metric_sum_in_window(
    emulator: &Emulator,
    metric: &str,
    dimensions: &Value,
    start: f64,
    end: f64,
    period: i64,
) -> f64 {
    let (status, stats) = cloudwatch(
        emulator,
        "GetMetricStatistics",
        &json!({
            "Namespace": METRIC_NAMESPACE,
            "MetricName": metric,
            "Dimensions": dimensions,
            "StartTime": start,
            "EndTime": end,
            "Period": period,
            "Statistics": ["Sum"],
        }),
    )
    .await;
    assert_eq!(status, 200, "GetMetricStatistics answered {status} {stats}");
    stats["Datapoints"]
        .as_array()
        .map(|points| {
            points
                .iter()
                .filter_map(|point| point["Sum"].as_f64())
                .sum::<f64>()
        })
        .unwrap_or_default()
}

/// Every series CloudWatch holds in EventBridge's namespace.
pub(crate) async fn published_series(emulator: &Emulator) -> Vec<Value> {
    let (status, listed) = cloudwatch(
        emulator,
        "ListMetrics",
        &json!({ "Namespace": METRIC_NAMESPACE }),
    )
    .await;
    assert_eq!(status, 200, "ListMetrics answered {status} {listed}");
    listed["Metrics"].as_array().cloned().unwrap_or_default()
}

/// The series CloudWatch holds in EventBridge's namespace that carry the
/// dimension `name` with the value `value` — a `DimensionFilter`, which matches a
/// series whose dimension set CONTAINS that dimension: "If you specify one
/// dimension name and a metric has that dimension and also other dimensions, it
/// will be returned." (`ListMetricsInput$Dimensions`)
pub(crate) async fn series_filtered_by(
    emulator: &Emulator,
    name: &str,
    value: &str,
) -> Vec<Value> {
    let (status, listed) = cloudwatch(
        emulator,
        "ListMetrics",
        &json!({
            "Namespace": METRIC_NAMESPACE,
            "Dimensions": [{ "Name": name, "Value": value }],
        }),
    )
    .await;
    assert_eq!(status, 200, "ListMetrics answered {status} {listed}");
    listed["Metrics"].as_array().cloned().unwrap_or_default()
}

/// What the rule's two invocation metrics count, and the series CloudWatch holds
/// in EventBridge's namespace: `(Invocations, FailedInvocations, series)`.
async fn invocation_metrics(emulator: &Emulator, rule: &str) -> (f64, f64, Vec<Value>) {
    let series = published_series(emulator).await;
    let invoked =
        metric_sum(emulator, INVOCATIONS_METRIC, DEFAULT_EVENT_BUS_NAME, rule).await;
    let failed = metric_sum(
        emulator,
        FAILED_INVOCATIONS_METRIC,
        DEFAULT_EVENT_BUS_NAME,
        rule,
    )
    .await;
    (invoked, failed, series)
}

/// One row of the role table: why the role does or does not bar the delivery.
struct RoleCase {
    case: &'static str,
    kind: Kind,
    /// The service principal the role's trust policy names, and the one action
    /// its inline policy allows — `None` for a role carrying no inline policy at
    /// all, and no role at all when `role` is `false`.
    principal: &'static str,
    allowed: Option<&'static str>,
    /// Whether the account holds the role the target names.
    role_exists: bool,
    expected: Expected,
}

/// What a row expects of the handover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    Invoked,
    Failed(&'static str),
}

/// A delivery the target's role bars is dead-lettered and counted; one the role
/// allows, and one the account holds no role for, go through.
#[tokio::test]
async fn a_delivery_the_targets_role_bars_is_dead_lettered_and_counted() {
    let rows = [
        RoleCase {
            case: "the role does not trust the EventBridge service principal",
            kind: Kind::StateMachine,
            principal: "lambda.amazonaws.com",
            allowed: Some("states:StartExecution"),
            role_exists: true,
            // The condition's own name, asserted as the LITERAL EventBridge
            // reports rather than through the constant the service words it
            // with, so a change to that constant's value fails this row. A role
            // EventBridge cannot assume at all failed at the assume-role step,
            // before any target API call was made.
            expected: Expected::Failed("FAILED_TO_ASSUME_ROLE"),
        },
        RoleCase {
            case: "the role's policy names another action",
            kind: Kind::StateMachine,
            principal: SERVICE_PRINCIPAL,
            allowed: Some("sqs:SendMessage"),
            role_exists: true,
            expected: Expected::Failed("NO_PERMISSIONS"),
        },
        RoleCase {
            case: "the role's policies state nothing about the call",
            kind: Kind::StateMachine,
            principal: SERVICE_PRINCIPAL,
            allowed: None,
            role_exists: true,
            expected: Expected::Failed("NO_PERMISSIONS"),
        },
        RoleCase {
            // A queue target's own CALL is decided by the QUEUE's resource
            // policy rather than by the role's policies, so a role that is
            // assumable but allows nothing does not stop the delivery.
            case: "a target kind whose invocation the role's permissions do not govern",
            kind: Kind::Queue,
            principal: SERVICE_PRINCIPAL,
            allowed: None,
            role_exists: true,
            expected: Expected::Invoked,
        },
        RoleCase {
            // The assume-role step, though, is reached by every target that
            // NAMES a role: a role EventBridge cannot act as fails the handover
            // for a queue target too.
            case: "a queue target under a role that does not trust this service",
            kind: Kind::Queue,
            principal: "lambda.amazonaws.com",
            allowed: None,
            role_exists: true,
            expected: Expected::Failed("FAILED_TO_ASSUME_ROLE"),
        },
        RoleCase {
            case: "the role may start an execution",
            kind: Kind::StateMachine,
            principal: SERVICE_PRINCIPAL,
            allowed: Some("states:StartExecution"),
            role_exists: true,
            expected: Expected::Invoked,
        },
        RoleCase {
            case: "the account holds no such role",
            kind: Kind::Queue,
            principal: SERVICE_PRINCIPAL,
            allowed: None,
            role_exists: false,
            expected: Expected::Invoked,
        },
    ];

    for row in rows {
        let emulator = emulator();
        let (target_arn, observe) = given_target(&emulator, row.kind).await;
        let (dead_letter_url, dead_letter_arn) = given_queue(&emulator, "dead-letters").await;
        let role_arn = if row.role_exists {
            given_role(&emulator, "delivery-role", row.principal, row.allowed).await
        } else {
            format!(
                "arn:aws:iam::{}:role/not-in-this-account",
                engine::emulator_account_id()
            )
        };
        given_rule(
            &emulator,
            "guarded",
            json!([{
                "Id": "t1",
                "Arn": target_arn,
                "RoleArn": role_arn,
                "DeadLetterConfig": { "Arn": dead_letter_arn },
            }]),
        )
        .await;

        let event_id = publish(&emulator).await;

        let recorded = invocations(&emulator);
        let outcome = recorded
            .first()
            .map(|(_, _, outcome)| outcome.clone())
            .unwrap_or_else(|| panic!("case {:?}: nothing was recorded", row.case));
        let dead_letters = drain(&emulator, &dead_letter_url).await;
        let (invoked, sum, series) = invocation_metrics(&emulator, "guarded").await;
        match row.expected {
            Expected::Invoked => {
                assert_eq!(
                    outcome,
                    InvocationOutcome::Invoked,
                    "case {:?}: expected the delivery to go through, got {outcome:?}",
                    row.case
                );
                assert!(
                    dead_letters.is_empty(),
                    "case {:?}: a delivered event is not dead-lettered, got {dead_letters:?}",
                    row.case
                );
                assert_eq!(
                    sum, 0.0,
                    "case {:?}: a delivered event counts no failed invocation, got {sum}",
                    row.case
                );
                assert_eq!(
                    invoked, 1.0,
                    "case {:?}: a delivered event counts one invocation, got {invoked} \
                     over the series {series:?}",
                    row.case
                );
                assert_eq!(
                    ordered(&series),
                    ordered(&rule_series(
                        DEFAULT_EVENT_BUS_NAME,
                        "guarded",
                        &[
                            INVOCATIONS_METRIC,
                            SUCCESSFUL_INVOCATION_ATTEMPTS_METRIC,
                            MATCHED_EVENTS_METRIC,
                            TRIGGERED_RULES_METRIC,
                        ],
                    )),
                    "case {:?}: expected the rule's own match and accepted-handover series, \
                     got {series:?}",
                    row.case
                );
                let delivered = observe.read(&emulator).await;
                assert_eq!(
                    delivered.len(),
                    1,
                    "case {:?}: expected the target to hold the event, got {delivered:?}",
                    row.case
                );
            }
            Expected::Failed(code) => {
                let InvocationOutcome::Failed { code: actual, .. } = &outcome else {
                    panic!(
                        "case {:?}: expected a failed invocation coded {code}, got {outcome:?}",
                        row.case
                    );
                };
                assert_eq!(
                    actual, code,
                    "case {:?}: expected the condition {code}, got {outcome:?}",
                    row.case
                );
                assert_eq!(
                    dead_letters.len(),
                    1,
                    "case {:?}: an undeliverable event goes to the dead-letter queue, \
                     got {dead_letters:?}",
                    row.case
                );
                let message = &dead_letters[0];
                assert_whole_envelope(
                    row.case,
                    message["Body"].as_str().unwrap_or_default(),
                    &event_id,
                );
                let attributes = &message["MessageAttributes"];
                assert_eq!(
                    attributes["TARGET_ARN"]["StringValue"],
                    json!(target_arn),
                    "case {:?}: the dead-letter message names the target, got {message}",
                    row.case
                );
                assert_eq!(
                    attributes["ERROR_CODE"]["StringValue"],
                    json!(code),
                    "case {:?}: the dead-letter message carries the condition, got {message}",
                    row.case
                );
                assert_eq!(
                    attributes["RULE_ARN"]["StringValue"],
                    json!(format!(
                        "arn:aws:events:{REGION}:{}:rule/guarded",
                        engine::emulator_account_id()
                    )),
                    "case {:?}: the dead-letter message names the rule, got {message}",
                    row.case
                );
                assert!(
                    attributes["ERROR_MESSAGE"]["StringValue"]
                        .as_str()
                        .is_some_and(|text| !text.is_empty()),
                    "case {:?}: the dead-letter message says why, got {message}",
                    row.case
                );
                assert_eq!(
                    sum, 1.0,
                    "case {:?}: one undeliverable event counts one failed invocation, got {sum} \
                     over the series {series:?}",
                    row.case
                );
                assert_eq!(
                    invoked, 0.0,
                    "case {:?}: an undeliverable event counts no invocation, got {invoked} \
                     over the series {series:?}",
                    row.case
                );
                assert_eq!(
                    ordered(&series),
                    ordered(&rule_series(
                        DEFAULT_EVENT_BUS_NAME,
                        "guarded",
                        &[
                            FAILED_INVOCATIONS_METRIC,
                            MATCHED_EVENTS_METRIC,
                            TRIGGERED_RULES_METRIC,
                        ],
                    )),
                    "case {:?}: expected the rule's own match and FailedInvocations series, \
                     got {series:?}",
                    row.case
                );
                if row.kind == Kind::StateMachine {
                    let delivered = observe.read(&emulator).await;
                    assert!(
                        delivered.is_empty(),
                        "case {:?}: a barred delivery reaches the target with nothing, \
                         got {delivered:?}",
                        row.case
                    );
                }
            }
        }
    }
}
