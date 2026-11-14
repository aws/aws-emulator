// SPDX-License-Identifier: CC0-1.0

//! What an Amazon SQS event source mapping HANDS its function, and what it does with a batch
//! the function did not take, across the services that own the halves of it: Amazon SQS holds
//! the queue, its records and the attributes it keeps for them, IAM answers for the execution
//! role the poller reads as, CloudFormation archives the inline handler into the function's
//! deployment package, and Lambda polls the queue and RUNS that handler.
//!
//! The rule pinned here: **a record an event source mapping hands its function carries the
//! message as the queue holds it — its `body`, each of its message attributes with the
//! `dataType` the sender gave it beside that attribute's own value, and the system attributes
//! the queue keeps for the message (`SentTimestamp`, and `ApproximateReceiveCount` counting
//! the deliveries the message has had) — and an invocation that did NOT succeed processes
//! nothing of its batch: no message of it is deleted, so each stays on the queue, comes back
//! when the hold on it lapses and is handed over again under a higher
//! `ApproximateReceiveCount`, while the mapping reports the attempt as a failed function
//! call.**
//!
//! Four crates have to agree for that to hold (Amazon SQS records what was sent and decides
//! who may receive it, IAM answers for the execution role, CloudFormation turns the inline
//! source into a runnable package, Lambda polls and invokes), so the suite lives here rather
//! than in any one of them. Every observation is one a client makes: the marker queue the
//! handler writes the record it was handed to, the mapping's own `LastProcessingResult`, and
//! the source queue's own `ReceiveMessage`.
//!
//! The handler reports before it FAILS: it forwards each record of the event to a second queue
//! and then raises, so "the handler ran" and "the batch was not processed" are told apart —
//! which is what distinguishes a mapping that never invoked anything from one that invoked and
//! then deleted a message it should have kept.
//!
//! The rows differ along what the rule generalises over — whether the message the mapping takes
//! carries attributes at all: a `String` attribute beside a `Number` one, and a message carrying
//! none. The full catalog of attribute data types a record has to carry is tabled where it
//! belongs, in the owning crate's own
//! `services/service-com-amazonaws-lambda-awsgirapiservice/tests/all/sqs_event_source_record_attributes.rs`;
//! what is pinned HERE is that a RUN handler is handed those attributes and that the batch it
//! failed on survives, which only the assembled emulator can show.

use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::LAST_PROCESSING_RESULT_FUNCTION_CALL_FAILED;
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::query_wire::{element, form_encode};

/// The Region every request below is signed for — the one the shared stack reader
/// ([`wait_for_stack_status`]) signs for.
const REGION: &str = "us-east-1";

/// The `StackStatus` each row's consumer stack has to reach.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The execution role every row's function runs as, created before any of the row queues: the
/// grant below therefore names the ACTIONS the poller and the handler make rather than each
/// row's own queue.
const ROLE_NAME: &str = "esm-failed-batch-consumer";

/// Where `CreateEventSourceMapping` is bound (`POST /2015-03-31/event-source-mappings`).
const MAPPINGS_URI: &str = "/2015-03-31/event-source-mappings";

/// How long the source queue hides a record it hands a consumer, in seconds, and the mapped
/// function's own `Timeout`: Lambda accepts a queue mapping only while the queue's
/// `VisibilityTimeout` is at least the function's `Timeout`, and the span is the one a message
/// a failed attempt left behind comes back after.
const HOLD_SECONDS: i64 = 3;

/// The `State` a mapping reports once it polls its event source.
const STATE_ENABLED: &str = "Enabled";

/// How long an observation of work the CLOCK owes — the stack coming up, the function going
/// `Active`, the mapping settling, an invocation, a record coming back — waits before giving
/// up, and how long between two looks. Generous: a row fails only when the work did not happen
/// at all.
const BUDGET: Duration = Duration::from_secs(60);
/// See [`BUDGET`].
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// `eventSource` every record of a queue-sourced event carries.
const SQS_EVENT_SOURCE: &str = "aws:sqs";

/// The handler every row deploys inline: it forwards each record it was handed to the marker
/// queue its environment names — addressed by NAME through the service's own `GetQueueUrl`, so
/// the source carries nothing the deployment had to compute — and THEN raises, so every
/// invocation of it fails after reporting what it received.
const REPORTING_THEN_FAILING_SOURCE: &str = r#"
import json
import os

import boto3


def handler(event, context):
    client = boto3.client("sqs")
    url = client.get_queue_url(QueueName=os.environ["MARKER_QUEUE"])["QueueUrl"]
    for record in event["Records"]:
        client.send_message(QueueUrl=url, MessageBody=json.dumps(record))
    raise RuntimeError("this handler always fails after reporting what it was handed")
"#;

/// The emulator as it is ASSEMBLED, on one engine: the handler's own SDK call is dispatched
/// back into this same router, so the marker queue it writes is the queue this suite reads.
fn emulator() -> MultiServiceRouter {
    let engine = std::sync::Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    // The suite exercises the services, not SigV4: every call carries a credential scope so
    // the router can name the service it belongs to.
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` whose credential scope names `signing_name`, which is what routes
/// the request. The signature itself is never checked here.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// Dispatch one request as it arrives on the wire, with whatever status it answered.
async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Bytes,
) -> (u16, Bytes) {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let response = router
        .dispatch(builder.body(body).expect("a well-formed request"))
        .await;
    let (parts, body) = response.into_parts();
    (parts.status.as_u16(), body)
}

/// One Amazon SQS call (`awsJson1_0`, `AmazonSQS.<Operation>`) that must answer 200.
async fn sqs(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, answered) = dispatch(
        router,
        "POST",
        &format!("sqs.{REGION}.amazonaws.com"),
        "/",
        &[
            ("content-type", "application/x-amz-json-1.0"),
            ("x-amz-target", &format!("AmazonSQS.{operation}")),
            ("authorization", &authorization("sqs")),
        ],
        Bytes::from(body.to_string()),
    )
    .await;
    let parsed: Value = if answered.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&answered)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&answered).to_string()))
    };
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from SQS {operation}, got {status} with {parsed}"
    );
    parsed
}

/// One AWS Lambda call over its `restJson1` wire, with whatever status it answered.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let (status, answered) = dispatch(
        router,
        method,
        &format!("lambda.{REGION}.amazonaws.com"),
        path,
        &[
            ("content-type", "application/json"),
            ("authorization", &authorization("lambda")),
        ],
        Bytes::from(payload),
    )
    .await;
    let parsed: Value = if answered.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&answered)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&answered).to_string()))
    };
    (status, parsed)
}

/// One awsQuery call of `signing_name` at `host`, as its status and XML body.
async fn query(
    router: &MultiServiceRouter,
    signing_name: &str,
    host: &str,
    version: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let (status, answered) = dispatch(
        router,
        "POST",
        host,
        "/",
        &[
            ("content-type", "application/x-www-form-urlencoded"),
            ("authorization", &authorization(signing_name)),
        ],
        Bytes::from(body),
    )
    .await;
    (status, String::from_utf8_lossy(&answered).to_string())
}

/// One IAM call that must answer 200.
async fn iam(router: &MultiServiceRouter, action: &str, members: &[(&str, &str)]) -> String {
    let (status, body) = query(
        router,
        "iam",
        "iam.amazonaws.com",
        "2010-05-08",
        action,
        members,
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected HTTP 200 from IAM {action}, got {status} with {body}"
    );
    body
}

/// One CloudFormation call that must answer 200.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> String {
    let (status, body) = query(
        router,
        "cloudformation",
        &format!("cloudformation.{REGION}.amazonaws.com"),
        "2010-05-15",
        action,
        members,
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected HTTP 200 from CloudFormation {action}, got {status} with {body}"
    );
    body
}

/// The execution role every row's function runs as: trusted by Lambda, granting the reads a
/// queue mapping's poller makes on its source and the writes the handler makes to the marker
/// queue. Answers its ARN as IAM reports it.
async fn given_role(router: &MultiServiceRouter) -> String {
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "lambda.amazonaws.com" },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string();
    let created = iam(
        router,
        "CreateRole",
        &[
            ("RoleName", ROLE_NAME),
            ("AssumeRolePolicyDocument", &trust),
        ],
    )
    .await;
    let arn = element(&created, "Arn");
    let permissions = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": [
                "sqs:ReceiveMessage",
                "sqs:DeleteMessage",
                "sqs:GetQueueAttributes",
                "sqs:GetQueueUrl",
                "sqs:SendMessage",
            ],
            // The row queues are created after the role, so the grant names the actions the
            // poller and the handler make rather than each row's own queue.
            "Resource": "*",
        }],
    })
    .to_string();
    iam(
        router,
        "PutRolePolicy",
        &[
            ("RoleName", ROLE_NAME),
            ("PolicyName", "consume-and-report"),
            ("PolicyDocument", &permissions),
        ],
    )
    .await;
    arn
}

/// A queue called `name` hiding a delivered record for `visibility_timeout` seconds; answers
/// its url and the ARN Amazon SQS itself reports for it.
async fn given_queue(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    visibility_timeout: i64,
) -> (String, String) {
    let created = sqs(
        router,
        case,
        "CreateQueue",
        &json!({
            "QueueName": name,
            "Attributes": { "VisibilityTimeout": visibility_timeout.to_string() },
        }),
    )
    .await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: CreateQueue {name:?} reports no QueueUrl, got {created}")
        })
        .to_string();
    let read = sqs(
        router,
        case,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    let arn = read["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the queue {name:?} reports no QueueArn"))
        .to_string();
    (url, arn)
}

/// Deploy the row's consumer: a stack holding the one function, whose inline code
/// CloudFormation archives into the deployment package Lambda runs.
async fn given_function(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    function: &str,
    marker_queue: &str,
    role_arn: &str,
) {
    let template = json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "Consumer": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": function,
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    "Role": role_arn,
                    "Timeout": HOLD_SECONDS,
                    "Environment": { "Variables": { "MARKER_QUEUE": marker_queue } },
                    "Code": { "ZipFile": REPORTING_THEN_FAILING_SOURCE },
                },
            },
        },
    })
    .to_string();
    cloudformation(
        router,
        "CreateStack",
        &[("StackName", stack), ("TemplateBody", &template)],
    )
    .await;
    let (reached, described) = wait_for_stack_status(router, stack, CREATE_COMPLETE).await;
    assert_eq!(
        reached, CREATE_COMPLETE,
        "case {case:?}: expected the consumer's stack to reach {CREATE_COMPLETE}, got {described}"
    );
    let deadline = Instant::now() + BUDGET;
    loop {
        let (status, configuration) = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{function}/configuration"),
            None,
        )
        .await;
        if status == 200 && configuration["State"].as_str() == Some("Active") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: Lambda never reported {function} Active, last read {status} \
             {configuration}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// The enabled mapping on the row's source queue, one message per batch; answers its `UUID`
/// once Lambda reports it `Enabled`.
async fn given_mapping(
    router: &MultiServiceRouter,
    case: &str,
    function: &str,
    queue_arn: &str,
) -> String {
    let (status, created) = lambda(
        router,
        "POST",
        MAPPINGS_URI,
        Some(&json!({
            "FunctionName": function,
            "EventSourceArn": queue_arn,
            "BatchSize": 1,
            "Enabled": true,
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {case:?}: expected HTTP 202 from CreateEventSourceMapping, got {status} with \
         {created}"
    );
    let uuid = created["UUID"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the created mapping reports no UUID, got {created}"))
        .to_string();
    let deadline = Instant::now() + BUDGET;
    loop {
        let (status, read) = lambda(router, "GET", &format!("{MAPPINGS_URI}/{uuid}"), None).await;
        if status == 200 && read["State"].as_str() == Some(STATE_ENABLED) {
            return uuid;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the mapping {uuid} never reported {STATE_ENABLED}, last read \
             {status} {read}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// What `GetEventSourceMapping` reports as the result of the mapping's last processing
/// attempt, or `None` while it has made none.
async fn last_processing_result(
    router: &MultiServiceRouter,
    case: &str,
    uuid: &str,
) -> Option<String> {
    let (status, read) = lambda(router, "GET", &format!("{MAPPINGS_URI}/{uuid}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetEventSourceMapping {uuid} answered {status} {read}"
    );
    read["LastProcessingResult"].as_str().map(str::to_string)
}

/// Take whatever is waiting on the marker queue `url` addresses, deleting each message so a
/// later look finds only later markers, and answer the records the handler forwarded.
async fn take_markers(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<Value> {
    let received = sqs(
        router,
        case,
        "ReceiveMessage",
        &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
    )
    .await;
    let messages = received["Messages"].as_array().cloned().unwrap_or_default();
    let mut records = Vec::with_capacity(messages.len());
    for message in &messages {
        let text = message["Body"].as_str().unwrap_or_default().to_string();
        sqs(
            router,
            case,
            "DeleteMessage",
            &json!({
                "QueueUrl": url,
                "ReceiptHandle": message["ReceiptHandle"].as_str().unwrap_or_default(),
            }),
        )
        .await;
        records.push(serde_json::from_str(&text).unwrap_or_else(|error| {
            panic!("case {case:?}: expected a forwarded record as a JSON document, got {text:?} ({error})")
        }));
    }
    records
}

/// Poll the marker queue until the handler has reported a record whose
/// `ApproximateReceiveCount` is at least `deliveries`, and answer every record seen so far in
/// the order they arrived.
async fn await_delivery(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    seen: &mut Vec<Value>,
    deliveries: u64,
) {
    let deadline = Instant::now() + BUDGET;
    loop {
        seen.extend(take_markers(router, case, url).await);
        if seen.iter().any(|record| receive_count(record) >= deliveries) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the handler to be handed the message a {deliveries}(th) \
             time, it reported {seen:?}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// The `ApproximateReceiveCount` a forwarded record carries, as a number — 0 when it carries
/// none, which fails the assertion that reads it.
fn receive_count(record: &Value) -> u64 {
    record["attributes"]["ApproximateReceiveCount"]
        .as_str()
        .and_then(|count| count.parse().ok())
        .unwrap_or_default()
}

/// Poll `ReceiveMessage` on the source queue as an ordinary consumer until it is handed a
/// message, and answer it.
async fn await_message_back(router: &MultiServiceRouter, case: &str, url: &str) -> Value {
    let deadline = Instant::now() + BUDGET;
    loop {
        let received = sqs(
            router,
            case,
            "ReceiveMessage",
            &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
        )
        .await;
        let messages = received["Messages"].as_array().cloned().unwrap_or_default();
        if let Some(message) = messages.first() {
            return message.clone();
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: a message no invocation processed must come back to its queue, \
             {url} handed out nothing"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// One row: the slug its resources are named from, the body it sends, the `MessageAttributes`
/// it sends and the `messageAttributes` the record has to carry for them.
struct Row {
    case: &'static str,
    slug: &'static str,
    body: &'static str,
    /// What `SendMessage` is given, keyed by attribute name.
    sent: Value,
    /// What the record the function is handed must carry, keyed by the same names — the
    /// `dataType` the sender gave beside that attribute's own value, as a queue-source record
    /// reports one.
    expected: Value,
}

/// The rows: one per set of message attributes a record has to carry through unchanged.
///
/// `String` and `Number` are two of the attribute data types Amazon SQS publishes for
/// `MessageAttributeValue$DataType` ("Amazon SQS supports the following logical data types:
/// String, Number, and Binary", `smithy_query(query="doc",shape="MessageAttributeValue")`); the
/// third, and the rest of that catalog, is tabled in the owning crate's own suite named in this
/// module's header, so the rows here span the one dimension this suite adds to it — a message
/// carrying attributes against one carrying none.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a String attribute beside a Number one",
            slug: "esm-keeps-typed",
            body: "{\"order\": \"A-1\"}",
            sent: json!({
                "orderId": { "DataType": "String", "StringValue": "A-1" },
                "attempts": { "DataType": "Number", "StringValue": "7" },
            }),
            expected: json!({
                "orderId": { "dataType": "String", "stringValue": "A-1" },
                "attempts": { "dataType": "Number", "stringValue": "7" },
            }),
        },
        Row {
            case: "a message carrying no attributes at all",
            slug: "esm-keeps-bare",
            body: "a body that is not a JSON document",
            sent: json!({}),
            expected: json!({}),
        },
    ]
}

/// RULE: the record an SQS event source mapping hands its function carries the message's body,
/// its message attributes with their data types and its system attributes; and an invocation
/// that failed processes nothing, so the message stays on the queue, is handed over again under
/// a higher `ApproximateReceiveCount` and comes back to an ordinary consumer.
#[tokio::test]
async fn a_failed_invocation_keeps_its_message_and_its_record_carries_every_attribute() {
    let router = emulator();
    let role_arn = given_role(&router).await;

    for row in rows() {
        let case = row.case;
        let marker_name = format!("{}-marker", row.slug);
        let (marker_url, _) = given_queue(&router, case, &marker_name, HOLD_SECONDS).await;
        let (source_url, source_arn) =
            given_queue(&router, case, &format!("{}-source", row.slug), HOLD_SECONDS).await;
        let function = format!("{}-consumer", row.slug);
        given_function(
            &router,
            case,
            &format!("{}-stack", row.slug),
            &function,
            &marker_name,
            &role_arn,
        )
        .await;
        let uuid = given_mapping(&router, case, &function, &source_arn).await;

        let mut send = json!({ "QueueUrl": source_url, "MessageBody": row.body });
        if row.sent.as_object().is_some_and(|sent| !sent.is_empty()) {
            send["MessageAttributes"] = row.sent.clone();
        }
        let sent = sqs(&router, case, "SendMessage", &send).await;
        let message_id = sent["MessageId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: SendMessage reports no MessageId, got {sent}"))
            .to_string();

        // The handler reports before it raises, so the FIRST record it forwarded is the one
        // the mapping handed it on the message's first delivery.
        let mut seen: Vec<Value> = Vec::new();
        await_delivery(&router, case, &marker_url, &mut seen, 1).await;
        let first = seen
            .iter()
            .find(|record| receive_count(record) == 1)
            .unwrap_or_else(|| panic!("case {case:?}: no record of the first delivery in {seen:?}"))
            .clone();

        assert_eq!(
            first["messageId"].as_str(),
            Some(message_id.as_str()),
            "case {case:?}: the record must name the message that was sent, got {first}"
        );
        assert_eq!(
            first["body"].as_str(),
            Some(row.body),
            "case {case:?}: the record must carry the body as the queue holds it, got {first}"
        );
        assert_eq!(
            first["eventSource"].as_str(),
            Some(SQS_EVENT_SOURCE),
            "case {case:?}: the record must name its event source, got {first}"
        );
        assert_eq!(
            first["eventSourceARN"].as_str(),
            Some(source_arn.as_str()),
            "case {case:?}: the record must name the queue it came from, got {first}"
        );
        assert_eq!(
            first["messageAttributes"], row.expected,
            "case {case:?}: expected the message attributes {}, got {}",
            row.expected, first["messageAttributes"]
        );
        let sent_timestamp = first["attributes"]["SentTimestamp"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            sent_timestamp.parse::<u64>().is_ok_and(|millis| millis > 0),
            "case {case:?}: the record must carry the queue's own SentTimestamp as epoch \
             milliseconds, got {sent_timestamp:?}"
        );

        // Nothing of the batch was processed, which the mapping reports and which leaves the
        // message to be handed over again: the second delivery's own record proves it was
        // never deleted.
        await_delivery(&router, case, &marker_url, &mut seen, 2).await;
        let redelivered = seen
            .iter()
            .filter(|record| receive_count(record) >= 2)
            .cloned()
            .collect::<Vec<Value>>();
        for record in &redelivered {
            assert_eq!(
                record["messageId"].as_str(),
                Some(message_id.as_str()),
                "case {case:?}: a redelivery must carry the same message, got {record}"
            );
            assert_eq!(
                record["messageAttributes"], row.expected,
                "case {case:?}: a redelivery must carry the same attributes, got {record}"
            );
        }
        assert_eq!(
            last_processing_result(&router, case, &uuid).await,
            Some(LAST_PROCESSING_RESULT_FUNCTION_CALL_FAILED.to_string()),
            "case {case:?}: the mapping must report its attempt as a failed function call"
        );

        // With the mapping gone nothing polls the queue, so the message a failed invocation
        // left behind is handed to an ordinary consumer — the client-visible half of "the
        // message returns to the queue".
        let (status, deleted) = lambda(&router, "DELETE", &format!("{MAPPINGS_URI}/{uuid}"), None).await;
        assert_eq!(
            status, 202,
            "case {case:?}: DeleteEventSourceMapping answered {status} {deleted}"
        );
        let back = await_message_back(&router, case, &source_url).await;
        assert_eq!(
            back["MessageId"].as_str(),
            Some(message_id.as_str()),
            "case {case:?}: the message that came back must be the one that was sent, got {back}"
        );
        assert_eq!(
            back["Body"].as_str(),
            Some(row.body),
            "case {case:?}: the message that came back must carry its body unchanged, got {back}"
        );
    }
}
