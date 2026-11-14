// SPDX-License-Identifier: CC0-1.0

//! What a client WATCHING the queue sees once a configuration set publishes a send's email
//! events to its SNS event destination — through the WHOLE mounted service set, the one
//! `core/router/src/lib.rs` assembles for the binary.
//!
//! The RULE under test: **each event an accepted send produces that an `Enabled` event
//! destination's `MatchingEventTypes` names becomes ONE message on every queue subscribed to
//! that destination's topic — so the queue's `ApproximateNumberOfMessages` advances by the
//! number of events the send caused, and `ReceiveMessage` answers them as `Message`s each
//! carrying `MessageId`, `ReceiptHandle`, `MD5OfBody` and a `Body` that is the SNS
//! notification whose `Message` is the SES event naming the message the send answered with.**
//! A destination that is off puts nothing on its queue, and the same destination switched on
//! publishes the next send's events.
//!
//! The sibling suite `ses_send_events_reach_a_subscribed_queue` pins what each event
//! DOCUMENT says; what is pinned here is what the QUEUE reports about the arrival — the
//! depth counter a consumer polls and the members of the message it then receives — which no
//! other suite asserts, and which three services have to agree on: SESv2 publishes, SNS fans
//! out to the subscribed queue, and SQS holds and reports what arrived.
//!
//! The rows differ along what the rule generalises over: which event types the destination
//! matches (one, a pair, every modelled type), and so how many messages one send puts on the
//! queue, and what the addressed recipient does with the message (takes it, refuses it).

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudwatch_alarm_action_reaches_its_subscribed_queue::{
    authorization, given_topic_with_subscriber, Grant,
};
use crate::ses_send_events_reach_a_subscribed_queue::{router, EVERY_EVENT_TYPE, REGION};

/// The sending domain these cases verify, and the address it covers: a verified domain
/// identity authorises every address in the domain.
const SENDER_DOMAIN: &str = "queue-watchers.example.com";
const SENDER: &str = "notices@queue-watchers.example.com";

/// The mailbox-simulator mailbox that TAKES a message, and the one that refuses it. Both are
/// the simulator's own mailboxes, whose reactions are the ones the simulator defines, so a
/// case does not depend on what any other mailbox would do with the message.
const TAKES_THE_MESSAGE: &str = "success@simulator.amazonses.com";
const REFUSES_THE_MESSAGE: &str = "bounce@simulator.amazonses.com";

/// The SQS attribute a consumer polls for the queue's depth: "the approximate number of
/// messages available for retrieval from the queue"
/// (`smithy_query(query="doc",shape="QueueAttributeName",member="ApproximateNumberOfMessages")`).
const DEPTH_ATTRIBUTE: &str = "ApproximateNumberOfMessages";

/// How many times a case asks the queue how deep it is before giving up, and how long it
/// waits between asks.
///
/// An email event is published on a task of its own — the message is accepted and answered
/// before any event of it exists — so a case waits for the ARRIVAL rather than for a span:
/// it stops as soon as the depth it expects is reported, and the budget below is far longer
/// than the publication needs so that a loaded machine delays a case instead of failing it.
const DEPTH_POLLS: usize = 600;
const DEPTH_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How many times a case asks the queue to hand over what arrived before giving up.
///
/// Each of those asks waits [`WAIT_TIME_SECONDS`] of its own for a message, so this bound is
/// its own rather than the depth loop's: the depth it polls for has already been reported by
/// the time a case receives, so the messages are there to be handed over.
const RECEIVE_POLLS: usize = 20;

/// How long one `ReceiveMessage` waits for a message to become available.
const WAIT_TIME_SECONDS: u32 = 1;

/// One Amazon SQS call (`awsJson1_0`, `AmazonSQS.<Operation>`), which must answer 200.
async fn sqs(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sqs.{REGION}.amazonaws.com"))
        .header("authorization", authorization("sqs", REGION))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from SQS {operation}, got {status} with {parsed}"
    );
    parsed
}

/// One SESv2 call: `restJson1` under the `ses` credential scope, at the SES endpoint host, in
/// SESv2's own `/v2/email/…` path space. Must answer 200.
async fn sesv2(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Value {
    let payload = match &body {
        Some(value) => bytes::Bytes::from(value.to_string()),
        None => bytes::Bytes::new(),
    };
    let host = format!("email.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", &host)
        .header("content-type", "application/json")
        .header("authorization", authorization("ses", REGION))
        .body(payload)
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from SESv2 {method} {path}, got {status} with {parsed}"
    );
    parsed
}

/// The account with [`SENDER_DOMAIN`] verified for sending, so every send below is
/// authorised.
///
/// The handler stores the instant the verification transition falls due rather than sleeping
/// for it, so this brings that instant into the past and settles the store instead of waiting
/// out the span.
async fn given_verified_sender(
    router: &MultiServiceRouter,
    engine: &aws_emulator_router::InMemoryEngine,
    case: &str,
) {
    sesv2(
        router,
        case,
        "POST",
        "/v2/email/identities",
        Some(json!({"EmailIdentity": SENDER_DOMAIN})),
    )
    .await;
    let state = engine.state::<service_com_amazonaws_sesv2_simpleemailservice_v2::state::ComAmazonawsSesv2SimpleemailserviceV2State>();
    let mut store = state.lock();
    let past = std::time::SystemTime::now()
        .checked_sub(Duration::from_secs(1))
        .expect("an instant before now");
    for record in store.identities.values_mut() {
        record.verification_due_at = past;
    }
    store.settle(std::time::SystemTime::now());
}

/// A configuration set of this account, holding nothing yet.
async fn given_configuration_set(router: &MultiServiceRouter, case: &str, set: &str) {
    sesv2(
        router,
        case,
        "POST",
        "/v2/email/configuration-sets",
        Some(json!({"ConfigurationSetName": set})),
    )
    .await;
}

/// An SNS event destination under `set`, publishing to `topic`.
async fn given_destination(
    router: &MultiServiceRouter,
    case: &str,
    set: &str,
    name: &str,
    topic: &str,
    enabled: bool,
    matching: &[&str],
) {
    sesv2(
        router,
        case,
        "POST",
        &format!("/v2/email/configuration-sets/{set}/event-destinations"),
        Some(json!({
            "EventDestinationName": name,
            "EventDestination": {
                "Enabled": enabled,
                "MatchingEventTypes": matching,
                "SnsDestination": {"TopicArn": topic},
            },
        })),
    )
    .await;
}

/// Switch the destination `name` of `set` on, leaving everything else about it as it was.
async fn switch_destination_on(
    router: &MultiServiceRouter,
    case: &str,
    set: &str,
    name: &str,
    topic: &str,
    matching: &[&str],
) {
    sesv2(
        router,
        case,
        "PUT",
        &format!("/v2/email/configuration-sets/{set}/event-destinations/{name}"),
        Some(json!({
            "EventDestination": {
                "Enabled": true,
                "MatchingEventTypes": matching,
                "SnsDestination": {"TopicArn": topic},
            },
        })),
    )
    .await;
}

/// `SendEmail` to `recipient` through `set`, answering the `MessageId` it minted.
async fn send(router: &MultiServiceRouter, case: &str, set: &str, recipient: &str) -> String {
    let answered = sesv2(
        router,
        case,
        "POST",
        "/v2/email/outbound-emails",
        Some(json!({
            "FromEmailAddress": SENDER,
            "ConfigurationSetName": set,
            "Destination": {"ToAddresses": [recipient]},
            "Content": {"Simple": {
                "Subject": {"Data": "a message under test"},
                "Body": {"Text": {"Data": "hello"}},
            }},
        })),
    )
    .await;
    let message_id = answered["MessageId"].as_str().unwrap_or_default().to_string();
    assert!(
        !message_id.is_empty(),
        "case {case:?}: an accepted send must answer a MessageId, got {answered}"
    );
    message_id
}

/// What the queue at `url` reports as its depth, as the decimal string the attribute map
/// carries.
async fn depth(router: &MultiServiceRouter, case: &str, url: &str) -> String {
    let answered = sqs(
        router,
        case,
        "GetQueueAttributes",
        &json!({"QueueUrl": url, "AttributeNames": [DEPTH_ATTRIBUTE]}),
    )
    .await;
    answered["Attributes"][DEPTH_ATTRIBUTE]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Ask the queue at `url` how deep it is until it reports `wanted`, answering what it last
/// reported. Nothing sleeps for the publication: this stops as soon as the depth arrives.
async fn depth_reaching(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    wanted: usize,
) -> String {
    let expected = wanted.to_string();
    let mut reported = String::new();
    for _ in 0..DEPTH_POLLS {
        reported = depth(router, case, url).await;
        if reported == expected {
            break;
        }
        tokio::time::sleep(DEPTH_POLL_INTERVAL).await;
    }
    reported
}

/// Receive from the queue at `url` until `wanted` messages have been handed over or the polls
/// run out, leaving every message where it is (nothing is deleted).
async fn received(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    wanted: usize,
) -> Vec<Value> {
    let mut messages: Vec<Value> = Vec::new();
    for _ in 0..RECEIVE_POLLS {
        if messages.len() >= wanted {
            break;
        }
        let answered = sqs(
            router,
            case,
            "ReceiveMessage",
            &json!({
                "QueueUrl": url,
                "MaxNumberOfMessages": 10,
                "WaitTimeSeconds": WAIT_TIME_SECONDS,
            }),
        )
        .await;
        match answered["Messages"].as_array() {
            Some(arrived) => messages.extend(arrived.iter().cloned()),
            None => tokio::time::sleep(DEPTH_POLL_INTERVAL).await,
        }
    }
    messages
}

/// The SES event one received `message` carries: its `Body` is the SNS notification, whose
/// `Message` is the published document.
fn published_event(case: &str, message: &Value, topic: &str) -> Value {
    let body = message["Body"].as_str().unwrap_or_default();
    let envelope: Value = serde_json::from_str(body)
        .unwrap_or_else(|error| panic!("case {case:?}: the body is no SNS envelope ({error}): {body}"));
    assert_eq!(
        envelope["Type"].as_str(),
        Some("Notification"),
        "case {case:?}: expected an SNS notification, got {envelope}"
    );
    assert_eq!(
        envelope["TopicArn"].as_str(),
        Some(topic),
        "case {case:?}: expected the notification to name the publishing topic, got {envelope}"
    );
    let inner = envelope["Message"].as_str().unwrap_or_default();
    serde_json::from_str(inner).unwrap_or_else(|error| {
        panic!("case {case:?}: the notification carries no SES event ({error}): {inner}")
    })
}

/// Assert the members `ReceiveMessage` answers for one arrived message: the queue's own id and
/// receipt handle, and the digest of the body it holds.
fn assert_message_members(case: &str, message: &Value) {
    let message_id = message["MessageId"].as_str().unwrap_or_default();
    assert!(
        !message_id.is_empty(),
        "case {case:?}: a received message must carry a MessageId, got {message}"
    );
    let handle = message["ReceiptHandle"].as_str().unwrap_or_default();
    assert!(
        !handle.is_empty(),
        "case {case:?}: a received message must carry a ReceiptHandle, got {message}"
    );
    let digest = message["MD5OfBody"].as_str().unwrap_or_default();
    assert!(
        digest.len() == 32 && digest.chars().all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()),
        "case {case:?}: MD5OfBody must be the body's digest in lower-case hexadecimal, got \
         {digest:?}"
    );
    assert!(
        !message["Body"].as_str().unwrap_or_default().is_empty(),
        "case {case:?}: a received message must carry a Body, got {message}"
    );
}

#[tokio::test]
async fn an_accepted_sends_events_advance_the_subscribed_queue_and_are_received_as_messages() {
    // (case, the event types the destination matches, the recipient addressed, the events
    // that reach the queue in the order they happen)
    let rows: &[(&str, &[&str], &str, &[&str])] = &[
        (
            "the delivery event alone",
            &["DELIVERY"],
            TAKES_THE_MESSAGE,
            &["Delivery"],
        ),
        (
            "the acceptance alone",
            &["SEND"],
            TAKES_THE_MESSAGE,
            &["Send"],
        ),
        (
            "the acceptance and the delivery together",
            &["SEND", "DELIVERY"],
            TAKES_THE_MESSAGE,
            &["Send", "Delivery"],
        ),
        (
            "every modelled event type, for a recipient that refuses the message",
            EVERY_EVENT_TYPE,
            REFUSES_THE_MESSAGE,
            &["Send", "Bounce"],
        ),
        (
            "a recipient that refuses the message, watched for the failure alone",
            &["BOUNCE"],
            REFUSES_THE_MESSAGE,
            &["Bounce"],
        ),
    ];

    for (case, matching, recipient, expected) in rows {
        let (router, engine) = router();
        let (topic, url) =
            given_topic_with_subscriber(&router, case, REGION, "ses-delivery", Grant::Unscoped)
                .await;
        given_verified_sender(&router, &engine, case).await;
        let set = "watched-events";
        given_configuration_set(&router, case, set).await;
        given_destination(&router, case, set, "events", &topic, true, matching).await;

        // Nothing has been sent, so the queue is empty — the depth a consumer polls starts at
        // zero rather than at nothing.
        assert_eq!(
            depth(&router, case, &url).await,
            "0",
            "case {case:?}: expected an empty queue before the send"
        );

        let message_id = send(&router, case, set, recipient).await;

        // The depth advances by the number of events this send caused, and by no more: an
        // event the destination does not match puts nothing on the queue.
        let reported = depth_reaching(&router, case, &url, expected.len()).await;
        assert_eq!(
            reported,
            expected.len().to_string(),
            "case {case:?}: expected the queue to report {} message(s) for the events {expected:?}, \
             got {reported:?}",
            expected.len()
        );

        // Each of them is received as one message, carrying the members a consumer reads it
        // by and the event that was published.
        let messages = received(&router, case, &url, expected.len()).await;
        assert_eq!(
            messages.len(),
            expected.len(),
            "case {case:?}: expected {} message(s) on the queue, got {messages:?}",
            expected.len()
        );
        let mut arrived: Vec<String> = Vec::new();
        let mut ids: Vec<String> = Vec::new();
        for message in &messages {
            assert_message_members(case, message);
            ids.push(message["MessageId"].as_str().unwrap_or_default().to_string());
            let event = published_event(case, message, &topic);
            assert_eq!(
                event["mail"]["messageId"].as_str(),
                Some(message_id.as_str()),
                "case {case:?}: the published event must name the message the send answered, \
                 got {event}"
            );
            assert_eq!(
                event["mail"]["source"].as_str(),
                Some(SENDER),
                "case {case:?}: the published event must name the sender, got {event}"
            );
            arrived.push(
                event["eventType"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            );
        }
        arrived.sort();
        let mut wanted: Vec<String> = expected.iter().map(|name| name.to_string()).collect();
        wanted.sort();
        assert_eq!(
            arrived, wanted,
            "case {case:?}: expected the events {wanted:?} on the queue, got {arrived:?}"
        );
        ids.sort();
        let distinct = ids.len();
        ids.dedup();
        assert_eq!(
            ids.len(),
            distinct,
            "case {case:?}: every message on the queue gets its own MessageId, got {ids:?}"
        );

        // The queue reported its depth without consuming anything: the receive above is what
        // took the messages out of sight, so the depth is now what is left behind them.
        assert_eq!(
            depth(&router, case, &url).await,
            "0",
            "case {case:?}: expected the received messages to be out of sight"
        );
    }
}

#[tokio::test]
async fn a_destination_that_is_off_leaves_its_queue_empty_while_another_one_delivers() {
    let case = "one set, one destination off and one on";
    let (router, engine) = router();
    // Two topics, each with a queue of its own subscribed to it: the one the destination
    // under test publishes to, and the control that says when this send's publication pass
    // is over.
    let (watched_topic, watched) =
        given_topic_with_subscriber(&router, case, REGION, "ses-switched", Grant::Unscoped).await;
    let (control_topic, control) =
        given_topic_with_subscriber(&router, case, REGION, "ses-control", Grant::Unscoped).await;
    given_verified_sender(&router, &engine, case).await;
    let set = "switched-events";
    let matching: &[&str] = &["DELIVERY"];
    given_configuration_set(&router, case, set).await;
    given_destination(&router, case, set, "off", &watched_topic, false, matching).await;
    given_destination(&router, case, set, "on", &control_topic, true, matching).await;

    let first = send(&router, case, set, TAKES_THE_MESSAGE).await;
    // The enabled destination's queue holds this send's delivery event, so the publication
    // pass for it has run — and the disabled destination's queue is still empty.
    let reported = depth_reaching(&router, case, &control, 1).await;
    assert_eq!(
        reported, "1",
        "case {case:?}: expected the enabled destination's queue to hold the delivery event, \
         got {reported:?}"
    );
    assert_eq!(
        depth(&router, case, &watched).await,
        "0",
        "case {case:?}: a destination that is off must put nothing on its queue"
    );
    let nothing = sqs(
        &router,
        case,
        "ReceiveMessage",
        &json!({"QueueUrl": watched, "MaxNumberOfMessages": 10}),
    )
    .await;
    assert!(
        nothing["Messages"]
            .as_array()
            .is_none_or(|messages| messages.is_empty()),
        "case {case:?}: expected no message on the disabled destination's queue, got {nothing}"
    );

    // Switched on, the SAME destination publishes the NEXT send's matching event — and not
    // the one produced while it was off.
    switch_destination_on(&router, case, set, "off", &watched_topic, matching).await;
    let second = send(&router, case, set, TAKES_THE_MESSAGE).await;
    assert_ne!(
        second, first,
        "case {case:?}: every accepted message gets its own MessageId"
    );
    let reported = depth_reaching(&router, case, &watched, 1).await;
    assert_eq!(
        reported, "1",
        "case {case:?}: expected the switched-on destination's queue to hold one event, got \
         {reported:?}"
    );
    let messages = received(&router, case, &watched, 1).await;
    assert_eq!(
        messages.len(),
        1,
        "case {case:?}: expected exactly the second send's delivery event, got {messages:?}"
    );
    assert_message_members(case, &messages[0]);
    let event = published_event(case, &messages[0], &watched_topic);
    assert_eq!(
        event["eventType"].as_str(),
        Some("Delivery"),
        "case {case:?}: expected the delivery event the destination matches, got {event}"
    );
    assert_eq!(
        event["mail"]["messageId"].as_str(),
        Some(second.as_str()),
        "case {case:?}: expected the event of the send made while the destination was on, got \
         {event}"
    );
}
