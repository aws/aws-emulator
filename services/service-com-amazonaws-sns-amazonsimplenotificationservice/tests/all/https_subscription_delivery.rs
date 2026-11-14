// SPDX-License-Identifier: CC0-1.0

//! `Subscribe` and `Publish` POST to an `https` subscription's ENDPOINT, end to end over the
//! wire through the real `MultiServiceRouter`, with the endpoint served by another service of
//! this same emulator — a Lambda function URL whose handler records every request it was sent
//! into an Amazon SQS queue and answers the status the request's own path asks for.
//!
//! The RULES asserted here, all about one subscription to a URL:
//!
//! * "https – delivery of JSON-encoded message via HTTPS POST" (`SubscribeInput$Protocol`):
//!   `Subscribe` POSTs a `SubscriptionConfirmation` to the endpoint — carrying the `Token`
//!   `ConfirmSubscription` is called with and the `SubscribeURL` that presents it — and, once
//!   the subscription is confirmed, every `Publish` is POSTed as a `Notification` whose
//!   `x-amz-sns-message-id` is the `MessageId` the publish answered with.
//! * "DeliveryPolicy – The policy that defines how Amazon SNS retries failed deliveries to
//!   HTTP/S endpoints" (`SetSubscriptionAttributesInput$AttributeName`): the attempts one
//!   notification costs are one plus the `healthyRetryPolicy`'s `numRetries` of the policy in
//!   force on that subscription — the one `EffectiveDeliveryPolicy` reports — so a
//!   subscription written no retry budget is POSTed to exactly once and one written a larger
//!   budget is attempted again for the SAME publish.
//! * "RedrivePolicy – When specified, sends undeliverable messages to the specified Amazon SQS
//!   dead-letter queue" (same member): the notification an endpoint never took is held in that
//!   queue, once per publish, in the form the subscription renders the message.
//!
//! The rows differ along what the rules generalise over: whether the endpoint takes the
//! notification, the retry budget the subscription was written (none at all, zero, one, two),
//! whether it asks for a redrive, and whether it asks for raw delivery. The attempts are
//! counted on the REQUESTS the endpoint recorded, so no row can pass without the POSTs really
//! being made.
//!
//! The `http` protocol's own half of the rule is the same path with the other scheme; what
//! differs is only the scheme an endpoint is read under, which the delivery module's own unit
//! test pins over both. This suite drives `https`, because a URL served inside this emulator
//! is published as an `https` one.

use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;

use crate::lambda_subscription_dead_letter::{given_queue, held};
use crate::lambda_subscription_delivery::{
    emulator, given_function_running, lambda, publish, sns, sqs, subscribe, Emulator,
};
use crate::sqs_subscription_delivery::element;

/// The subscription protocol under test: the endpoint is a URL, POSTed to over HTTPS.
const HTTPS_PROTOCOL: &str = "https";

/// The `Content-Type` a notification is POSTed with: the request policy of the subscription's
/// effective delivery policy, which for a subscription nobody gave one is the account system
/// default (`GetSubscriptionAttributesResponse$Attributes`, `EffectiveDeliveryPolicy`).
const DELIVERY_CONTENT_TYPE: &str = "text/plain; charset=UTF-8";

/// The header a RAW delivery carries, which is how an endpoint tells the bare published body
/// it was handed from the notification envelope it would otherwise receive.
const RAW_DELIVERY_HEADER: &str = "x-amz-sns-rawdelivery";

/// How long a wait for a recorded request or a dead letter lasts before a case fails: far
/// longer than any delivery here takes, so a loaded machine does not fail a case the emulator
/// would have served.
const DEADLINE: Duration = Duration::from_secs(60);

/// How often such a wait looks again.
const LOOK_INTERVAL: Duration = Duration::from_millis(25);

/// The handler behind the function URL every row's endpoint addresses.
///
/// It records every request it was sent — the path, the headers and the body — on the capture
/// queue whose url is formatted into it, which is how the POSTs SNS made are observed from
/// outside the endpoint, and then answers from the request's own path: a confirmation request
/// is always taken, a notification to a `/fail` path is refused with a server error, and every
/// other notification is taken.
const SINK_HANDLER: &str = r#"
import base64
import json

import boto3


def lambda_handler(event, context):
    headers = event.get("headers") or {}
    path = event.get("rawPath") or "/"
    body = event.get("body") or ""
    if event.get("isBase64Encoded"):
        body = base64.b64decode(body).decode("utf-8")
    boto3.client("sqs").send_message(
        QueueUrl="{queue_url}",
        MessageBody=json.dumps({"path": path, "headers": headers, "body": body}),
    )
    if headers.get("x-amz-sns-message-type") == "SubscriptionConfirmation":
        return {"statusCode": 200, "body": "confirmed"}
    if path.startswith("/fail"):
        return {"statusCode": 500, "body": "the sink could not take this notification"}
    return {"statusCode": 200, "body": "taken"}
"#;

/// One request the endpoint recorded.
#[derive(Debug, Clone)]
struct Recorded {
    /// The path the request addressed, which is the subscription it belongs to.
    path: String,
    /// The request's headers, as HTTP compares them: lowercased.
    headers: Value,
    /// The document POSTed.
    body: String,
}

impl Recorded {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(Value::as_str)
    }

    /// The POSTed document as JSON, or a failing case when it is not JSON at all.
    fn document(&self, case: &str) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|error| {
            panic!(
                "case {case:?}: the POSTed body must be a JSON document, got {error} for {:?}",
                self.body
            )
        })
    }
}

/// Every request the endpoint has recorded on the capture queue so far, drained: the queue is
/// read until a read reports nothing, so one call collects the whole batch.
async fn drain_recorded(emulator: &Emulator, capture_url: &str) -> Vec<Recorded> {
    let mut recorded = Vec::new();
    loop {
        let received = sqs(
            emulator,
            "ReceiveMessage",
            &json!({"QueueUrl": capture_url, "MaxNumberOfMessages": 10}),
        )
        .await;
        let messages = received["Messages"].as_array().cloned().unwrap_or_default();
        if messages.is_empty() {
            return recorded;
        }
        for message in messages {
            let body = message["Body"].as_str().unwrap_or_default();
            let captured: Value = serde_json::from_str(body).unwrap_or_else(|error| {
                panic!("the capture queue must hold JSON records, got {error} for {body:?}")
            });
            recorded.push(Recorded {
                path: captured["path"].as_str().unwrap_or_default().to_string(),
                headers: captured["headers"].clone(),
                body: captured["body"].as_str().unwrap_or_default().to_string(),
            });
        }
    }
}

/// The requests recorded for `path`, waiting until there are at least `expected` of them — or
/// until the deadline, which is how a case that expects a COUNT fails with what it did see.
///
/// Every POST this suite asserts on has already been made by the time the call that owed it
/// answered, so this waits only for the capture queue to have them all.
async fn recorded_for(
    emulator: &Emulator,
    capture_url: &str,
    path: &str,
    expected: usize,
) -> Vec<Recorded> {
    let deadline = Instant::now() + DEADLINE;
    let mut collected: Vec<Recorded> = Vec::new();
    loop {
        collected.extend(
            drain_recorded(emulator, capture_url)
                .await
                .into_iter()
                .filter(|recorded| recorded.path == path),
        );
        if collected.len() >= expected || Instant::now() >= deadline {
            return collected;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// The function URL the endpoint of every row addresses: an `AuthType=NONE` URL of `function`,
/// granted to every caller so an anonymous delivery reaches the handler.
async fn given_function_url(emulator: &Emulator, function: &str) -> String {
    let (status, url) = lambda(
        emulator,
        "POST",
        &format!("/2021-10-31/functions/{function}/url"),
        json!({"AuthType": "NONE"}),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateFunctionUrlConfig {function}: expected HTTP 201, got {status} with {url}"
    );
    let (status, granted) = lambda(
        emulator,
        "POST",
        &format!("/2015-03-31/functions/{function}/policy"),
        json!({
            "StatementId": "public-url",
            "Action": "lambda:InvokeFunctionUrl",
            "Principal": "*",
            "FunctionUrlAuthType": "NONE",
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "AddPermission {function}: expected HTTP 201, got {status} with {granted}"
    );
    url["FunctionUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateFunctionUrlConfig {function}: no FunctionUrl in {url}"))
        .trim_end_matches('/')
        .to_string()
}

/// Write one subscription attribute, through the call a client configures it with.
async fn set_attribute(emulator: &Emulator, subscription: &str, name: &str, value: &str) {
    sns(
        emulator,
        "SetSubscriptionAttributes",
        &[
            ("SubscriptionArn", subscription),
            ("AttributeName", name),
            ("AttributeValue", value),
        ],
    )
    .await;
}

/// The delivery policy that asks for `retries` retries of a failed delivery, with the shortest
/// delay a policy may name between them (`minDelayTarget` and `maxDelayTarget` are "delays in
/// seconds of at least 1", SNS Developer Guide, "Amazon SNS message delivery retries").
fn retry_policy(retries: i64) -> String {
    json!({
        "healthyRetryPolicy": {
            "numRetries": retries,
            "minDelayTarget": 1,
            "maxDelayTarget": 1,
            "numNoDelayRetries": 0,
            "numMinDelayRetries": 0,
            "numMaxDelayRetries": 0,
            "backoffFunction": "linear",
        },
    })
    .to_string()
}

/// One row: how the endpoint answers this subscription's notifications, the retry budget and
/// redrive the subscription is written, and what one publish must then cost.
struct Row {
    case: &'static str,
    /// The path on the sink this subscription's endpoint addresses, which is what tells the
    /// handler whether to take the notification — and what tells the recorded requests of one
    /// subscription from another's.
    path: &'static str,
    /// The `numRetries` written on the subscription, or `None` for one left on the account
    /// system defaults.
    retries: Option<i64>,
    /// Whether the subscription asks for a redrive.
    redrive: bool,
    /// Whether the subscription asks for raw delivery.
    raw: bool,
    /// The message published to this row's topic.
    message: &'static str,
    /// How many POSTs of that notification the endpoint must have recorded.
    posts: usize,
    /// How many dead letters the publish must leave in this subscription's queue.
    dead_letters: usize,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an endpoint that takes the notification",
            path: "/take/plain",
            retries: None,
            redrive: true,
            raw: false,
            message: "the plain body",
            posts: 1,
            dead_letters: 0,
        },
        Row {
            case: "a refusing endpoint and no retry budget at all",
            path: "/fail/none",
            retries: Some(0),
            redrive: true,
            raw: false,
            message: "the body nobody takes",
            posts: 1,
            dead_letters: 1,
        },
        Row {
            case: "a refusing endpoint and one retry",
            path: "/fail/one",
            retries: Some(1),
            redrive: true,
            raw: false,
            message: "the body attempted twice",
            posts: 2,
            dead_letters: 1,
        },
        Row {
            case: "a refusing endpoint and two retries",
            path: "/fail/two",
            retries: Some(2),
            redrive: true,
            raw: false,
            message: "the body attempted three times",
            posts: 3,
            dead_letters: 1,
        },
        Row {
            case: "a refusing endpoint and no redrive",
            path: "/fail/unheld",
            retries: Some(0),
            redrive: false,
            raw: false,
            message: "the body held nowhere",
            posts: 1,
            dead_letters: 0,
        },
        Row {
            case: "an endpoint taking a raw delivery",
            path: "/take/raw",
            retries: None,
            redrive: true,
            raw: true,
            message: "the raw body itself",
            posts: 1,
            dead_letters: 0,
        },
    ]
}

/// Every message waiting on a redrive's queue is read through the suite-shared reader of what
/// a redrive held ([`crate::lambda_subscription_dead_letter::held`]), which waits out its own
/// quiet budget for a row that expects none rather than a second copy of that rule here.

#[tokio::test]
async fn an_https_subscription_is_posted_to_under_its_own_delivery_policy() {
    let emulator = emulator();
    // The endpoint of every row: ONE function URL, whose handler records what it was sent and
    // answers from the path, so the rows differ in the subscription rather than in the sink.
    let (capture_url, _capture_arn) = given_queue(&emulator, "https-capture").await;
    let function = "https-subscription-sink";
    given_function_running(
        &emulator,
        function,
        &SINK_HANDLER.replace("{queue_url}", &capture_url),
    )
    .await;
    let sink = given_function_url(&emulator, function).await;

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let topic = element(
            &sns(
                &emulator,
                "CreateTopic",
                &[("Name", &format!("https-delivery-{index}"))],
            )
            .await,
            "TopicArn",
        );
        let endpoint = format!("{sink}{}", row.path);
        let subscription = subscribe(&emulator, &topic, HTTPS_PROTOCOL, &endpoint).await;

        // The confirmation request: it is POSTed by `Subscribe` itself, and it is what carries
        // the token no response of this service does.
        let confirmations = recorded_for(&emulator, &capture_url, row.path, 1).await;
        assert_eq!(
            confirmations.len(),
            1,
            "case {case:?}: expected exactly one SubscriptionConfirmation POST, got {}",
            confirmations.len()
        );
        let confirmation = &confirmations[0];
        assert_eq!(
            confirmation.header("x-amz-sns-message-type"),
            Some("SubscriptionConfirmation"),
            "case {case:?}: expected the confirmation's message-type header, got {:?}",
            confirmation.headers
        );
        assert_eq!(
            confirmation.header("x-amz-sns-topic-arn"),
            Some(topic.as_str()),
            "case {case:?}: expected the topic {topic:?} in the confirmation's headers, got {:?}",
            confirmation.headers
        );
        let posted_message_id = confirmation
            .header("x-amz-sns-message-id")
            .unwrap_or_default()
            .to_string();
        assert!(
            !posted_message_id.is_empty(),
            "case {case:?}: expected a message-id header on the confirmation, got {:?}",
            confirmation.headers
        );
        let document = confirmation.document(case);
        assert_eq!(
            (
                document["Type"].as_str(),
                document["TopicArn"].as_str(),
                document["MessageId"].as_str(),
            ),
            (
                Some("SubscriptionConfirmation"),
                Some(topic.as_str()),
                Some(posted_message_id.as_str()),
            ),
            "case {case:?}: expected the confirmation document of {topic:?}, got {document}"
        );
        let token = document["Token"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no Token in {document}"))
            .to_string();
        let subscribe_url = document["SubscribeURL"].as_str().unwrap_or_default();
        assert!(
            subscribe_url.contains("Action=ConfirmSubscription") && subscribe_url.contains(&token),
            "case {case:?}: expected a SubscribeURL presenting the token, got {subscribe_url:?}"
        );
        assert!(
            document["Message"]
                .as_str()
                .unwrap_or_default()
                .contains(&topic),
            "case {case:?}: expected the confirmation's Message to name the topic, got {document}"
        );
        assert!(
            document["Signature"].as_str().is_some()
                && document["SigningCertURL"].as_str().is_some(),
            "case {case:?}: expected the confirmation to be signed, got {document}"
        );

        // The token the endpoint was POSTed is the one that confirms the subscription.
        let confirmed = sns(
            &emulator,
            "ConfirmSubscription",
            &[("TopicArn", &topic), ("Token", &token)],
        )
        .await;
        assert_eq!(
            element(&confirmed, "SubscriptionArn"),
            subscription,
            "case {case:?}: expected the POSTed token to confirm {subscription:?}, got {confirmed}"
        );

        if let Some(retries) = row.retries {
            set_attribute(
                &emulator,
                &subscription,
                "DeliveryPolicy",
                &retry_policy(retries),
            )
            .await;
        }
        let dead_letter_url = if row.redrive {
            let (url, arn) = given_queue(&emulator, &format!("https-dlq-{index}")).await;
            set_attribute(
                &emulator,
                &subscription,
                "RedrivePolicy",
                &json!({"deadLetterTargetArn": arn}).to_string(),
            )
            .await;
            Some(url)
        } else {
            None
        };
        if row.raw {
            set_attribute(&emulator, &subscription, "RawMessageDelivery", "true").await;
        }

        let message_id = publish(&emulator, &topic, &[("Message", row.message)]).await;
        let notifications = recorded_for(&emulator, &capture_url, row.path, row.posts).await;
        assert_eq!(
            notifications.len(),
            row.posts,
            "case {case:?}: expected {} POST(s) of one publish, got {}",
            row.posts,
            notifications.len()
        );
        for notification in &notifications {
            assert_eq!(
                (
                    notification.header("x-amz-sns-message-type"),
                    notification.header("x-amz-sns-message-id"),
                    notification.header("x-amz-sns-topic-arn"),
                    notification.header("x-amz-sns-subscription-arn"),
                    notification.header("content-type"),
                ),
                (
                    Some("Notification"),
                    Some(message_id.as_str()),
                    Some(topic.as_str()),
                    Some(subscription.as_str()),
                    Some(DELIVERY_CONTENT_TYPE),
                ),
                "case {case:?}: expected the notification's headers of {message_id:?}, got {:?}",
                notification.headers
            );
            if row.raw {
                // A raw delivery POSTs the published body ITSELF, with nothing added, and
                // says so in the request so an endpoint can tell it from an envelope.
                assert_eq!(
                    notification.header(RAW_DELIVERY_HEADER),
                    Some("true"),
                    "case {case:?}: expected the raw-delivery header, got {:?}",
                    notification.headers
                );
                assert_eq!(
                    notification.body, row.message,
                    "case {case:?}: expected the raw body {:?}, got {:?}",
                    row.message, notification.body
                );
                continue;
            }
            assert_eq!(
                notification.header(RAW_DELIVERY_HEADER),
                None,
                "case {case:?}: an enveloped delivery carries no raw-delivery header, got {:?}",
                notification.headers
            );
            let document = notification.document(case);
            assert_eq!(
                (
                    document["Type"].as_str(),
                    document["MessageId"].as_str(),
                    document["TopicArn"].as_str(),
                    document["Message"].as_str(),
                ),
                (
                    Some("Notification"),
                    Some(message_id.as_str()),
                    Some(topic.as_str()),
                    Some(row.message),
                ),
                "case {case:?}: expected the notification of {message_id:?}, got {document}"
            );
            assert!(
                document["UnsubscribeURL"]
                    .as_str()
                    .unwrap_or_default()
                    .contains(&subscription),
                "case {case:?}: expected the notification's own unsubscribe link, got {document}"
            );
        }

        let Some(dead_letter_url) = dead_letter_url else {
            continue;
        };
        let letters = held(&emulator, &dead_letter_url, row.dead_letters).await;
        assert_eq!(
            letters.len(),
            row.dead_letters,
            "case {case:?}: expected {} dead letter(s), got {letters:?}",
            row.dead_letters
        );
        for letter in &letters {
            let document: Value = serde_json::from_str(letter).unwrap_or_else(|error| {
                panic!("case {case:?}: the dead letter must be the notification: {error}")
            });
            assert_eq!(
                (
                    document["MessageId"].as_str(),
                    document["Message"].as_str(),
                ),
                (Some(message_id.as_str()), Some(row.message)),
                "case {case:?}: expected the undeliverable notification of {message_id:?}, got \
                 {document}"
            );
        }
    }
}

/// A subscription still awaiting confirmation is sent the request again by a repeated
/// `Subscribe`, so a subscriber that did not receive the first one can ask for another — and
/// the token it carries is still the one that confirms that same subscription.
#[tokio::test]
async fn a_repeated_subscribe_posts_the_confirmation_again() {
    let emulator = emulator();
    let (capture_url, _capture_arn) = given_queue(&emulator, "https-recapture").await;
    let function = "https-reconfirmation-sink";
    given_function_running(
        &emulator,
        function,
        &SINK_HANDLER.replace("{queue_url}", &capture_url),
    )
    .await;
    let sink = given_function_url(&emulator, function).await;
    let topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "https-reconfirm")]).await,
        "TopicArn",
    );
    let path = "/take/again";
    let endpoint = format!("{sink}{path}");

    let subscription = subscribe(&emulator, &topic, HTTPS_PROTOCOL, &endpoint).await;
    let again = subscribe(&emulator, &topic, HTTPS_PROTOCOL, &endpoint).await;
    assert_eq!(
        again, subscription,
        "a repeated Subscribe must answer the same subscription, got {again:?}"
    );

    let requests = recorded_for(&emulator, &capture_url, path, 2).await;
    assert_eq!(
        requests.len(),
        2,
        "expected one SubscriptionConfirmation POST per Subscribe, got {}",
        requests.len()
    );
    let tokens: Vec<String> = requests
        .iter()
        .map(|request| {
            request.document("a repeated Subscribe")["Token"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        tokens[0], tokens[1],
        "expected the same token in both confirmation requests, got {tokens:?}"
    );
    let ids: Vec<Option<&str>> = requests
        .iter()
        .map(|request| request.header("x-amz-sns-message-id"))
        .collect();
    assert_ne!(
        ids[0], ids[1],
        "expected each confirmation request to be its own message, got {ids:?}"
    );

    let confirmed = sns(
        &emulator,
        "ConfirmSubscription",
        &[("TopicArn", &topic), ("Token", &tokens[1])],
    )
    .await;
    assert_eq!(
        element(&confirmed, "SubscriptionArn"),
        subscription,
        "expected the re-sent token to confirm {subscription:?}, got {confirmed}"
    );

    // And a CONFIRMED subscription is sent nothing further: there is nothing left to confirm.
    let third = subscribe(&emulator, &topic, HTTPS_PROTOCOL, &endpoint).await;
    assert_eq!(
        third, subscription,
        "a Subscribe on a confirmed subscription must answer it, got {third:?}"
    );
    let after = drain_recorded(&emulator, &capture_url).await;
    assert!(
        after.is_empty(),
        "expected no further confirmation request, got {after:?}"
    );
}

/// The endpoint a subscription names may be the address the emulator announces for a data
/// plane it serves ITSELF — its own authority followed by the reserved path that names the
/// receiver behind it — rather than the public host of that receiver: a subscription is POSTed
/// to the URL it was given, whichever of the two forms that URL is in.
#[tokio::test]
async fn an_endpoint_behind_the_emulators_own_authority_is_posted_to() {
    let emulator = emulator();
    let (capture_url, _capture_arn) = given_queue(&emulator, "https-local-capture").await;
    let function = "https-local-sink";
    given_function_running(
        &emulator,
        function,
        &SINK_HANDLER.replace("{queue_url}", &capture_url),
    )
    .await;
    let sink = given_function_url(&emulator, function).await;
    // The same receiver, addressed the other way: the url id the announced host names,
    // behind the reserved data-plane path of a loopback authority.
    let url_id = sink
        .strip_prefix("https://")
        .and_then(|rest| rest.split('.').next())
        .unwrap_or_else(|| panic!("the announced function URL must name a url id, got {sink:?}"));
    let path = format!("/_aws/lambda-url/{url_id}/take/local");
    let endpoint = format!("https://127.0.0.1:9297{path}");

    let topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "https-local")]).await,
        "TopicArn",
    );
    let subscription = subscribe(&emulator, &topic, HTTPS_PROTOCOL, &endpoint).await;
    let requests = recorded_for(&emulator, &capture_url, "/take/local", 1).await;
    assert_eq!(
        requests.len(),
        1,
        "expected the confirmation request at the emulator's own authority, got {}",
        requests.len()
    );
    let document = requests[0].document("an endpoint behind the emulator's own authority");
    let token = document["Token"]
        .as_str()
        .unwrap_or_else(|| panic!("no Token in {document}"))
        .to_string();
    let confirmed = sns(
        &emulator,
        "ConfirmSubscription",
        &[("TopicArn", &topic), ("Token", &token)],
    )
    .await;
    assert_eq!(
        element(&confirmed, "SubscriptionArn"),
        subscription,
        "expected that request's token to confirm {subscription:?}, got {confirmed}"
    );

    let message = "the body for a locally addressed endpoint";
    let message_id = publish(&emulator, &topic, &[("Message", message)]).await;
    let notifications = recorded_for(&emulator, &capture_url, "/take/local", 1).await;
    assert_eq!(
        notifications.len(),
        1,
        "expected one notification POST, got {}",
        notifications.len()
    );
    let document = notifications[0].document("an endpoint behind the emulator's own authority");
    assert_eq!(
        (
            notifications[0].header("x-amz-sns-message-id"),
            document["Message"].as_str(),
        ),
        (Some(message_id.as_str()), Some(message)),
        "expected the notification of {message_id:?}, got {document}"
    );
}
/// An endpoint that cannot take the request YET still receives it: the confirmation request is
/// attempted again along the phases the published delivery policy opens with, exactly as every
/// other delivery of this crate retries that answer.
///
/// The endpoint here is a function still inside the activation window its own `State` reports
/// — "During this time, you can't invoke or modify the function" (`CreateFunction`) — which is
/// the one condition a fresh receiver really meets: a client stands its webhook up and
/// subscribes it in the same breath.
#[tokio::test]
async fn an_endpoint_that_is_not_up_yet_still_receives_the_confirmation() {
    let emulator = emulator();
    let (capture_url, _capture_arn) = given_queue(&emulator, "https-pending-capture").await;
    let function = "https-pending-sink";
    given_function_running(
        &emulator,
        function,
        &SINK_HANDLER.replace("{queue_url}", &capture_url),
    )
    .await;
    let sink = given_function_url(&emulator, function).await;
    let topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "https-pending")]).await,
        "TopicArn",
    );
    let path = "/take/pending";

    // The function is put back inside its activation window, which is where a receiver
    // created moments before the subscription really is. The window is far shorter than the
    // retry schedule, so what the case turns on is that the request is attempted again at all.
    let window = Duration::from_millis(400);
    {
        let state = emulator
            .engine
            .state::<ComAmazonawsLambdaAwsgirapiserviceState>();
        let mut store = state.lock();
        for record in store.functions.values_mut() {
            record.state = "Pending".to_string();
            record.active_due = SystemTime::now() + window;
        }
    }

    let subscription = subscribe(&emulator, &topic, HTTPS_PROTOCOL, &format!("{sink}{path}")).await;
    let requests = recorded_for(&emulator, &capture_url, path, 1).await;
    assert_eq!(
        requests.len(),
        1,
        "expected the confirmation request to reach an endpoint that was not up yet, got {}",
        requests.len()
    );
    let document = requests[0].document("an endpoint that is not up yet");
    let token = document["Token"]
        .as_str()
        .unwrap_or_else(|| panic!("no Token in {document}"))
        .to_string();
    let confirmed = sns(
        &emulator,
        "ConfirmSubscription",
        &[("TopicArn", &topic), ("Token", &token)],
    )
    .await;
    assert_eq!(
        element(&confirmed, "SubscriptionArn"),
        subscription,
        "expected the retried request's token to confirm {subscription:?}, got {confirmed}"
    );
}
