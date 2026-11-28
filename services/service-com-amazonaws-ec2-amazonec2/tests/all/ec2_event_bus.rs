// SPDX-License-Identifier: CC0-1.0

//! The wiring the suites that watch Amazon EC2's EVENTS share: the emulator those events are
//! published on, a queue a real rule on the real default bus delivers them to, and the drain
//! that reads them off it.
//!
//! EC2 publishes an event by calling EventBridge, which delivers it to the targets of the
//! rules of the default bus, which is Amazon SQS calling in turn — so a suite that asserts on
//! an event needs all three services mounted on ONE engine, exactly as the assembled router
//! mounts them, and reads the event off the queue rather than out of this crate's own state.
//! That wiring is the same whatever class of event is being watched, so it lives here once and
//! each suite names only the `detail-type` its own rule matches.
//!
//! This module holds no test of its own: what each class of event asserts lives in that
//! class's own suite.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use service_com_amazonaws_eventbridge_awsevents as eventbridge;
use service_com_amazonaws_sqs_amazonsqs as sqs;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::call;

/// The Region every call of these suites is signed for.
pub const REGION: &str = "us-east-1";

/// The `source` every event Amazon EC2 publishes carries, spelled out here rather than
/// imported, so a wrong constant in the implementation cannot make a test pass.
pub const EC2_EVENT_SOURCE: &str = "aws.ec2";

/// How long a poll keeps asking before giving up. Generous: a publication is made once the
/// outcome it reports falls due, so this is many times the work a row does and still lets a
/// broken case fail rather than hang.
pub const DELIVERY_BUDGET: Duration = Duration::from_secs(60);

/// How long a poll waits between asks.
pub const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// How many `ReceiveMessage` rounds one drain makes: a receive answers at most ten messages,
/// and this is well above the events one suite's table publishes.
const MAX_RECEIVE_ROUNDS: usize = 8;

/// A `(key, value)` request parameter.
pub fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// Amazon EC2, EventBridge and Amazon SQS over ONE shared engine, exactly as the assembled
/// router mounts them: the bus an event is published onto and the queue a rule delivers it to
/// are each that service's own resource.
pub fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(ec2::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(eventbridge::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(sqs::EmulatedService::new(engine));
    router
}

/// One `awsJson` RPC call against the service whose signing name is `signing_name`, answering
/// the status and the payload.
async fn json_call(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    let request = ::http::Request::builder()
        .method("POST")
        .uri(format!("https://{signing_name}.{REGION}.amazonaws.com/"))
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header(
            "content-type",
            format!("application/x-amz-json-{json_version}"),
        )
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEC2EVENTBUS/20260101/{REGION}/\
                 {signing_name}/aws4_request, SignedHeaders=host;x-amz-target, Signature=0"
            ),
        )
        .body(::bytes::Bytes::from(
            ::serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed JSON request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = if response.body().is_empty() {
        Value::Null
    } else {
        ::serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, payload)
}

/// One EventBridge call, which must be served.
pub async fn events_ok(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: Value,
) -> Value {
    let (status, payload) = json_call(router, "events", "AWSEvents", "1.1", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: EventBridge {operation} must answer 200, got {status} with {payload}"
    );
    payload
}

/// One Amazon SQS call, which must be served.
pub async fn sqs_ok(router: &MultiServiceRouter, case: &str, operation: &str, body: Value) -> Value {
    let (status, payload) = json_call(router, "sqs", "AmazonSQS", "1.0", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: Amazon SQS {operation} must answer 200, got {status} with {payload}"
    );
    payload
}

/// One EC2 call, which must be served, as its body.
pub async fn ec2_ok(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(String, String)],
) -> String {
    let (status, body) = call(router, REGION, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// One EC2 call that must be REFUSED, answering the status and the body.
pub async fn ec2_refused(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    let (status, body) = call(router, REGION, action, params).await;
    assert_ne!(
        status, 200,
        "case {case:?}: {action} must be refused, got {status} with body {body}"
    );
    (status, body)
}

/// A queue, created through Amazon SQS itself and carrying the grant a rule's target needs —
/// EventBridge makes the target's `sqs:SendMessage` as its own service principal and relies on
/// the queue's resource-based policy — answering its url and the ARN a rule names it by.
async fn queue(router: &MultiServiceRouter, case: &str, name: &str) -> (String, String) {
    let created = sqs_ok(router, case, "CreateQueue", json!({ "QueueName": name })).await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: CreateQueue must answer a QueueUrl, got {created}")
        })
        .to_string();
    let attributes = sqs_ok(
        router,
        case,
        "GetQueueAttributes",
        json!({ "QueueUrl": &url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    let arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: GetQueueAttributes must answer a QueueArn, got {attributes}")
        })
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
    sqs_ok(
        router,
        case,
        "SetQueueAttributes",
        json!({ "QueueUrl": &url, "Attributes": { "Policy": policy } }),
    )
    .await;
    (url, arn)
}

/// A default-bus rule matching every `aws.ec2` event of the class `detail_type` names,
/// delivering to a queue of its own, and that queue's url — the wiring a suite watches its
/// events through.
pub async fn watched_queue(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    detail_type: &str,
) -> String {
    let (url, queue_arn) = queue(router, case, name).await;
    events_ok(
        router,
        case,
        "PutRule",
        json!({
            "Name": name,
            "State": "ENABLED",
            "EventPattern": json!({
                "source": [EC2_EVENT_SOURCE],
                "detail-type": [detail_type],
            })
            .to_string(),
        }),
    )
    .await;
    events_ok(
        router,
        case,
        "PutTargets",
        json!({ "Rule": name, "Targets": [{ "Id": "t1", "Arn": queue_arn }] }),
    )
    .await;
    url
}

/// Every event waiting on `url` right now, taken off the queue so a later drain reports only
/// what arrived after this one.
pub async fn drain(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<Value> {
    let mut events = Vec::new();
    for _ in 0..MAX_RECEIVE_ROUNDS {
        let received = sqs_ok(
            router,
            case,
            "ReceiveMessage",
            json!({ "QueueUrl": url, "MaxNumberOfMessages": 10, "WaitTimeSeconds": 0 }),
        )
        .await;
        let messages = received["Messages"].as_array().cloned().unwrap_or_default();
        if messages.is_empty() {
            break;
        }
        for message in messages {
            if let Some(handle) = message["ReceiptHandle"].as_str() {
                sqs_ok(
                    router,
                    case,
                    "DeleteMessage",
                    json!({ "QueueUrl": url, "ReceiptHandle": handle }),
                )
                .await;
            }
            let body = message["Body"].as_str().unwrap_or_default();
            events.push(::serde_json::from_str::<Value>(body).unwrap_or_else(|error| {
                panic!("case {case:?}: a delivered event is JSON ({error}): {body}")
            }));
        }
    }
    events
}

/// The envelope every event of the class `detail_type` names carries: the EC2 source and that
/// `detail-type`. Asserted once per event a suite reads, so no suite reads an event of another
/// class as one of its own.
pub fn assert_envelope(case: &str, detail_type: &str, event: &Value) {
    assert_eq!(
        event["source"].as_str(),
        Some(EC2_EVENT_SOURCE),
        "case {case:?}: expected the source {EC2_EVENT_SOURCE}, got {event}"
    );
    assert_eq!(
        event["detail-type"].as_str(),
        Some(detail_type),
        "case {case:?}: expected the detail-type {detail_type}, got {event}"
    );
}

/// The ARNs an event names in its `resources`.
pub fn resources(event: &Value) -> Vec<String> {
    event["resources"]
        .as_array()
        .map(|resources| {
            resources
                .iter()
                .filter_map(|resource| resource.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}
