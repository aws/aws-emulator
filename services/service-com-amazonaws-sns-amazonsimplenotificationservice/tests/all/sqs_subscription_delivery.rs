// SPDX-License-Identifier: CC0-1.0

//! `Publish` DELIVERS the message to the SQS queues subscribed to the topic, end to end over
//! the wire (awsQuery for SNS, `awsJson1_0` for SQS) through the real `MultiServiceRouter`,
//! with SQS mounted on the same engine so a delivery is observed where a client observes it:
//! on the queue.
//!
//! The RULE asserted here: "Sends a message to an Amazon SNS topic […] When a messageId is
//! returned, the message has been saved and Amazon SNS will attempt to deliver it shortly"
//! (`Publish`) — every CONFIRMED subscription of the topic whose endpoint is an SQS queue
//! receives exactly one message per publish. Its body is the SNS notification envelope
//! (`Type`, `MessageId`, `TopicArn`, `Subject`, `Message`, `Timestamp`, `MessageAttributes`),
//! or the published message EXACTLY as it was published when the subscription's
//! `RawMessageDelivery` is `true`: "When set to true, enables raw message delivery to Amazon
//! SQS […] endpoints. This eliminates the need for the endpoints to process JSON formatting,
//! which is otherwise created for Amazon SNS metadata."
//! (`GetSubscriptionAttributesResponse$Attributes`).
//!
//! The rows differ along what the rule generalises over: how many queues are subscribed,
//! whether the subscription asked for raw delivery, whether the publish carried a subject or
//! message attributes, and whether the subscribed queue belongs to the topic published to at
//! all.
//!
//! Every queue here is created WITH the queue policy an SNS subscriber needs
//! ([`grant_to_sns`]), because a delivery SNS is not granted is withheld by the queue; which
//! policies grant it is `sqs_delivery_authorization.rs`.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::topics_subscriptions_publish::percent_encode;

/// The Region every call below is signed for.
const REGION: &str = "us-east-1";

/// SNS, SQS and KMS on ONE engine: a subscription's endpoint is a queue, the delivery is
/// asserted on the queue, and the key a queue is encrypted with is a resource of KMS — whose
/// own key policy decides a delivery into such a queue
/// (`sqs_delivery_queue_encryption.rs`).
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

pub(crate) fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_kms_trentservice::EmulatedService::new(Arc::clone(&engine)),
    );
    Emulator { router }
}

/// One awsQuery form call against SNS, with whatever status it answered: the call every
/// SNS request in this module goes through, and the one a REFUSED call is read from.
pub(crate) async fn sns_call(
    emulator: &Emulator,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-03-31");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        // The form encoding is the suite-shared one, not a second copy.
        body.push_str(&percent_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sns.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{REGION}/sns/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    (status, text)
}

/// One awsQuery form call against SNS that must SUCCEED.
pub(crate) async fn sns(emulator: &Emulator, action: &str, params: &[(&str, &str)]) -> String {
    let (status, text) = sns_call(emulator, action, params).await;
    assert_eq!(
        status, 200,
        "{action}: expected HTTP 200, got {status} / {text}"
    );
    text
}

/// One `awsJson1_0` call against SQS that must SUCCEED.
pub(crate) async fn sqs(emulator: &Emulator, operation: &str, body: &Value) -> Value {
    let (status, parsed) = sqs_call(emulator, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// One `awsJson1_0` call against SQS, with whatever status it answered: the call every SQS
/// request in this suite goes through, and the one a REFUSED call is read from.
pub(crate) async fn sqs_call(
    emulator: &Emulator,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sqs.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{REGION}/sqs/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed: Value = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One `awsJson1_1` call against KMS that must SUCCEED: `POST /` with
/// `X-Amz-Target: TrentService.<Operation>`, through the same router.
///
/// The key a subscribed queue is encrypted with is a KMS resource, and the document that
/// decides a delivery into such a queue is that key's own policy, so a suite asserting the
/// delivery writes the key and its policy here.
pub(crate) async fn kms(emulator: &Emulator, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("kms.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("TrentService.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{REGION}/kms/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed: Value = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "{operation}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// The text of the first `<tag>…</tag>` element of an awsQuery response.
pub(crate) fn element(body: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = body
        .find(&open)
        .unwrap_or_else(|| panic!("no <{tag}> in {body}"))
        + open.len();
    let end = body[start..]
        .find(&close)
        .unwrap_or_else(|| panic!("no </{tag}> in {body}"));
    body[start..start + end].to_string()
}

/// FETCH `url` through this same router, as a client that was handed it would: the host and
/// the path (with its query) are taken from the URL itself, so a URL this emulator minted is
/// followed exactly as it was handed out.
///
/// Answers the status and the body as text. `pub(crate)` because a URL a notification
/// carries is only a URL if following it reaches a document
/// (`notification_signature.rs`).
pub(crate) async fn fetch(emulator: &Emulator, url: &str) -> (u16, String) {
    let addressed = url.split_once("://").map_or(url, |(_, rest)| rest);
    let (host, path) = match addressed.find('/') {
        Some(at) => (&addressed[..at], &addressed[at..]),
        None => (addressed, "/"),
    };
    let request = http::Request::builder()
        .method("GET")
        .uri(path)
        .header("host", host)
        .body(bytes::Bytes::new())
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    (status, text)
}

/// A queue called `name` an SNS topic may deliver to: its url and the ARN a subscription
/// names it by, with the queue policy that grants SNS the send.
pub(crate) async fn given_queue(emulator: &Emulator, name: &str) -> (String, String) {
    let (url, arn) = given_queue_without_policy(emulator, name).await;
    set_queue_policy(emulator, &url, &grant_to_sns(&arn)).await;
    (url, arn)
}

/// A queue called `name` with NO policy of its own: its url and its ARN.
pub(crate) async fn given_queue_without_policy(
    emulator: &Emulator,
    name: &str,
) -> (String, String) {
    let created = sqs(emulator, "CreateQueue", &json!({"QueueName": name})).await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateQueue {name}: no QueueUrl in {created}"))
        .to_string();
    let arn = queue_arn(emulator, &url).await;
    (url, arn)
}

/// The `QueueArn` the queue at `url` reports: the ARN a subscription names that queue by.
pub(crate) async fn queue_arn(emulator: &Emulator, url: &str) -> String {
    let attributes = sqs(
        emulator,
        "GetQueueAttributes",
        &json!({"QueueUrl": url, "AttributeNames": ["QueueArn"]}),
    )
    .await;
    attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("GetQueueAttributes {url}: no QueueArn in {attributes}"))
        .to_string()
}

/// Write `policy` as the `Policy` attribute of the queue at `url`.
pub(crate) async fn set_queue_policy(emulator: &Emulator, url: &str, policy: &str) {
    sqs(
        emulator,
        "SetQueueAttributes",
        &json!({"QueueUrl": url, "Attributes": {"Policy": policy}}),
    )
    .await;
}

/// The queue policy AWS documents for an SQS queue subscribed to an SNS topic: the service
/// principal `sns.amazonaws.com`, allowed `sqs:SendMessage` on that queue.
pub(crate) fn grant_to_sns(queue_arn: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AllowSNSDelivery",
            "Effect": "Allow",
            "Principal": {"Service": "sns.amazonaws.com"},
            "Action": "sqs:SendMessage",
            "Resource": queue_arn,
        }],
    })
    .to_string()
}

/// Every message body waiting on the queue at `url`, as raw text, with the message
/// attributes each one carries.
pub(crate) async fn drain(emulator: &Emulator, url: &str) -> Vec<(String, Value)> {
    let received = sqs(
        emulator,
        "ReceiveMessage",
        &json!({
            "QueueUrl": url,
            "MaxNumberOfMessages": 10,
            "MessageAttributeNames": ["All"],
        }),
    )
    .await;
    received["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| {
            (
                message["Body"].as_str().unwrap_or_default().to_string(),
                message["MessageAttributes"].clone(),
            )
        })
        .collect()
}

#[tokio::test]
async fn a_published_message_reaches_every_subscribed_queue() {
    let emulator = emulator();
    let topic = element(&sns(&emulator, "CreateTopic", &[("Name", "orders")]).await, "TopicArn");
    let other_topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "invoices")]).await,
        "TopicArn",
    );

    // (case, the queue's name, the topic it subscribes to, whether it asked for raw delivery,
    //  how many messages it must receive)
    let rows: &[(&str, &str, bool, bool, usize)] = &[
        ("the first subscriber of the topic", "fan-out-one", true, false, 1),
        ("a second subscriber of the same topic", "fan-out-two", true, false, 1),
        ("a raw-delivery subscriber", "raw-delivery", true, true, 1),
        ("a subscriber of another topic", "another-topic", false, false, 0),
    ];

    let mut queues: Vec<(&str, String, bool)> = Vec::new();
    for (case, name, own_topic, raw, _) in rows {
        let (url, arn) = given_queue(&emulator, name).await;
        let subscription = element(
            &sns(
                &emulator,
                "Subscribe",
                &[
                    ("TopicArn", if *own_topic { &topic } else { &other_topic }),
                    ("Protocol", "sqs"),
                    ("Endpoint", &arn),
                ],
            )
            .await,
            "SubscriptionArn",
        );
        if *raw {
            sns(
                &emulator,
                "SetSubscriptionAttributes",
                &[
                    ("SubscriptionArn", &subscription),
                    ("AttributeName", "RawMessageDelivery"),
                    ("AttributeValue", "true"),
                ],
            )
            .await;
        }
        assert!(
            subscription.contains(':'),
            "case {case:?}: an SQS subscription is confirmed at once, got {subscription:?}"
        );
        queues.push((case, url, *raw));
    }

    let message = r#"{"orderId":"A-1"}"#;
    sns(
        &emulator,
        "Publish",
        &[
            ("TopicArn", &topic),
            ("Message", message),
            ("Subject", "an order"),
            ("MessageAttributes.entry.1.Name", "kind"),
            ("MessageAttributes.entry.1.Value.DataType", "String"),
            ("MessageAttributes.entry.1.Value.StringValue", "created"),
        ],
    )
    .await;

    for ((case, url, raw), (_, _, _, _, expected)) in queues.iter().zip(rows) {
        let delivered = drain(&emulator, url).await;
        assert_eq!(
            delivered.len(),
            *expected,
            "case {case:?}: expected {expected} delivered message(s), got {delivered:?}"
        );
        let Some((body, attributes)) = delivered.first() else {
            continue;
        };
        if *raw {
            // "enables raw message delivery […] This eliminates the need for the endpoints to
            // process JSON formatting, which is otherwise created for Amazon SNS metadata."
            assert_eq!(
                body, message,
                "case {case:?}: a raw delivery must carry the published message exactly, got \
                 {body:?}"
            );
            assert_eq!(
                attributes["kind"]["StringValue"].as_str(),
                Some("created"),
                "case {case:?}: a raw delivery must carry the published message attributes as \
                 the queue's own, got {attributes}"
            );
            continue;
        }
        let envelope: Value = serde_json::from_str(body)
            .unwrap_or_else(|error| panic!("case {case:?}: body is not JSON ({error}): {body}"));
        assert_eq!(
            envelope["Type"].as_str(),
            Some("Notification"),
            "case {case:?}: the envelope must declare its type, got {envelope}"
        );
        assert_eq!(
            envelope["TopicArn"].as_str(),
            Some(topic.as_str()),
            "case {case:?}: the envelope must name the topic, got {envelope}"
        );
        assert_eq!(
            envelope["Message"].as_str(),
            Some(message),
            "case {case:?}: the envelope must carry the message as published, got {envelope}"
        );
        assert_eq!(
            envelope["Subject"].as_str(),
            Some("an order"),
            "case {case:?}: the envelope must carry the subject as published, got {envelope}"
        );
        assert_eq!(
            envelope["MessageAttributes"]["kind"],
            json!({"Type": "String", "Value": "created"}),
            "case {case:?}: the envelope must carry the published attributes, got {envelope}"
        );
        assert!(
            envelope["MessageId"].as_str().is_some_and(|id| !id.is_empty()),
            "case {case:?}: the envelope must carry the id of the published message, got \
             {envelope}"
        );
        assert!(
            envelope
                .get("Signature")
                .and_then(Value::as_str)
                .is_some_and(|signature| !signature.is_empty()),
            "case {case:?}: an enveloped delivery is signed, so it must carry a signature, \
             got {envelope}"
        );
        assert_eq!(
            *attributes,
            Value::Null,
            "case {case:?}: an enveloped delivery carries the attributes IN the envelope, not \
             as the queue's own, got {attributes}"
        );
    }
}

#[tokio::test]
async fn a_batched_publish_reaches_the_subscribed_queue_once_per_entry() {
    let emulator = emulator();
    let topic = element(&sns(&emulator, "CreateTopic", &[("Name", "batched")]).await, "TopicArn");
    let (url, arn) = given_queue(&emulator, "batched-queue").await;
    sns(
        &emulator,
        "Subscribe",
        &[
            ("TopicArn", &topic),
            ("Protocol", "sqs"),
            ("Endpoint", &arn),
        ],
    )
    .await;

    // The rule the batched form must satisfy is the same one a single publish does: every
    // confirmed queue subscription receives one message per PUBLISHED message. Rows differ in
    // how many entries the batch carries.
    let rows: &[(&str, &[&str], usize)] = &[
        ("one entry", &["first"], 1),
        ("three entries", &["a", "b", "c"], 3),
        ("ten entries, the documented maximum", &["1", "2", "3", "4", "5", "6", "7", "8", "9", "10"], 10),
    ];

    for (case, messages, expected) in rows {
        let mut params: Vec<(String, String)> = vec![("TopicArn".to_string(), topic.clone())];
        for (index, message) in messages.iter().enumerate() {
            let prefix = format!("PublishBatchRequestEntries.member.{}", index + 1);
            params.push((format!("{prefix}.Id"), format!("e{index}")));
            params.push((format!("{prefix}.Message"), (*message).to_string()));
        }
        let borrowed: Vec<(&str, &str)> = params
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        sns(&emulator, "PublishBatch", &borrowed).await;

        let delivered = drain(&emulator, &url).await;
        assert_eq!(
            delivered.len(),
            *expected,
            "case {case:?}: expected {expected} delivered message(s), got {delivered:?}"
        );
        for (body, _) in &delivered {
            let envelope: Value = serde_json::from_str(body).unwrap_or_else(|error| {
                panic!("case {case:?}: body is not the SNS envelope ({error}): {body}")
            });
            assert!(
                messages.contains(&envelope["Message"].as_str().unwrap_or_default()),
                "case {case:?}: the envelope must carry one of the published messages, got \
                 {envelope}"
            );
        }
    }

    // A batch holding an entry a single `Publish` would be refused for is refused WHOLE,
    // so not one of its siblings reaches the queue.
    let (status, body) = sns_call(
        &emulator,
        "PublishBatch",
        &[
            ("TopicArn", &topic),
            ("PublishBatchRequestEntries.member.1.Id", "kept"),
            ("PublishBatchRequestEntries.member.1.Message", "kept"),
            ("PublishBatchRequestEntries.member.2.Id", "empty"),
            ("PublishBatchRequestEntries.member.2.Message", ""),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "a batch with an entry carrying no message must be refused, got {status} / {body}"
    );
    let delivered = drain(&emulator, &url).await;
    assert!(
        delivered.is_empty(),
        "a refused batch must deliver nothing, got {delivered:?}"
    );
}

#[tokio::test]
async fn each_publish_delivers_once_and_only_to_confirmed_subscriptions() {
    let emulator = emulator();
    let topic = element(&sns(&emulator, "CreateTopic", &[("Name", "events")]).await, "TopicArn");
    let (url, arn) = given_queue(&emulator, "events-queue").await;
    sns(
        &emulator,
        "Subscribe",
        &[
            ("TopicArn", &topic),
            ("Protocol", "sqs"),
            ("Endpoint", &arn),
        ],
    )
    .await;
    // An endpoint that has to confirm its subscription receives nothing, and its presence
    // must not change what the confirmed queue receives.
    sns(
        &emulator,
        "Subscribe",
        &[
            ("TopicArn", &topic),
            ("Protocol", "https"),
            ("Endpoint", "https://example.invalid/hook"),
        ],
    )
    .await;

    // Rows differ in how many publishes precede the read: each one delivers exactly one
    // message, so the queue holds as many as were published.
    for (case, publishes) in [("one publish", 1_usize), ("three more publishes", 3)] {
        for index in 0..publishes {
            sns(
                &emulator,
                "Publish",
                &[("TopicArn", &topic), ("Message", &format!("event-{index}"))],
            )
            .await;
        }
        let delivered = drain(&emulator, &url).await;
        assert_eq!(
            delivered.len(),
            publishes,
            "case {case:?}: expected {publishes} delivered message(s), got {delivered:?}"
        );
    }
}

/// One row of the attribute table: the case, the published attribute's data type and value,
/// and the value the queue must receive as its own attribute.
type AttributeCase<'a> = (&'a str, &'a str, &'a str, &'a str);

/// A RAW delivery carries the published attributes as the QUEUE's own, and the value of a
/// `Number` attribute is the number it names — "Number – Numbers are positive or negative
/// integers or floating-point numbers" (`MessageAttributeValue$DataType`) — so the redundant
/// sign and zero padding the publisher spelled it with are not part of it. A value of any
/// other type is text, and a `Number` whose value is not a decimal number cannot be respelled,
/// so both are carried exactly as published.
///
/// The rows differ along what the rule generalises over: the data type, the sign, the padding
/// (leading zeros, trailing fraction zeros), the magnitude (one no binary float holds), the
/// notation, and a value that is not a number at all.
#[tokio::test]
async fn a_raw_delivery_carries_the_number_a_number_attribute_names() {
    let emulator = emulator();
    let topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "attribute-values")]).await,
        "TopicArn",
    );
    let (url, arn) = given_queue(&emulator, "attribute-values-queue").await;
    let subscription = element(
        &sns(
            &emulator,
            "Subscribe",
            &[
                ("TopicArn", &topic),
                ("Protocol", "sqs"),
                ("Endpoint", &arn),
                ("ReturnSubscriptionArn", "true"),
            ],
        )
        .await,
        "SubscriptionArn",
    );
    sns(
        &emulator,
        "SetSubscriptionAttributes",
        &[
            ("SubscriptionArn", &subscription),
            ("AttributeName", "RawMessageDelivery"),
            ("AttributeValue", "true"),
        ],
    )
    .await;

    let rows: &[AttributeCase<'_>] = &[
        ("a number with a redundant sign", "Number", "+5", "5"),
        ("a number already canonical", "Number", "5", "5"),
        ("a negative number", "Number", "-5", "-5"),
        ("a number padded with leading zeros", "Number", "007", "7"),
        ("a fraction padded with trailing zeros", "Number", "5.50", "5.5"),
        ("a signed zero", "Number", "-0", "0"),
        (
            // A magnitude no binary float holds: the digits the publisher sent survive.
            "a number past the precision of a double",
            "Number",
            "12345678901234567890.5",
            "12345678901234567890.5",
        ),
        (
            // SNS leaves exponent notation as published; the queue's own `SendMessage`
            // then stores a `Number` as the number it names, so what the queue receives
            // is that number written out plainly.
            "a number in exponent notation",
            "Number",
            "1e2",
            "100",
        ),
        // Only a Number is a number: a String's value is text, sign and all — so a value
        // that is not a number at all is carried verbatim under a type that takes text.
        ("a string that looks like a number", "String", "+5", "+5"),
        ("a string that is not a number", "String", "five", "five"),
    ];

    for (case, data_type, published, expected) in rows {
        sns(
            &emulator,
            "Publish",
            &[
                ("TopicArn", &topic),
                ("Message", "an attributed message"),
                ("MessageAttributes.entry.1.Name", "n"),
                ("MessageAttributes.entry.1.Value.DataType", data_type),
                ("MessageAttributes.entry.1.Value.StringValue", published),
            ],
        )
        .await;
        let delivered = drain(&emulator, &url).await;
        assert_eq!(
            delivered.len(),
            1,
            "case {case:?}: expected one delivered message, got {delivered:?}"
        );
        let (_, attributes) = &delivered[0];
        let actual = attributes["n"]["StringValue"].as_str();
        assert_eq!(
            actual,
            Some(*expected),
            "case {case:?}: expected the queue to receive {expected:?}, got {actual:?}"
        );
        let carried_type = attributes["n"]["DataType"].as_str();
        assert_eq!(
            carried_type,
            Some(*data_type),
            "case {case:?}: the attribute keeps the data type it was published with, got \
             {carried_type:?}"
        );
    }

    // A `Number` attribute whose value is not a number is one SNS does not accept at
    // all: "Number – Numbers are positive or negative integers or floating-point
    // numbers" (`MessageAttributeValue$DataType` doc), so the publish is refused rather
    // than fanned out to a queue whose own `SendMessage` would refuse it in turn.
    let (status, body) = sns_call(
        &emulator,
        "Publish",
        &[
            ("TopicArn", &topic),
            ("Message", "an attributed message"),
            ("MessageAttributes.entry.1.Name", "n"),
            ("MessageAttributes.entry.1.Value.DataType", "Number"),
            ("MessageAttributes.entry.1.Value.StringValue", "five"),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"a number that is not a number\": expected HTTP 400, got {status} / {body}"
    );
    let delivered = drain(&emulator, &url).await;
    assert!(
        delivered.is_empty(),
        "case \"a number that is not a number\": a refused publish delivers nothing, got \
         {delivered:?}"
    );
}
