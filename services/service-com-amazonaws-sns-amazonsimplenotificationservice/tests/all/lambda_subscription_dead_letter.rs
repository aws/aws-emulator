// SPDX-License-Identifier: CC0-1.0

//! What a subscription's REDRIVE holds when the endpoint is a LAMBDA FUNCTION: end to end over
//! the wire through the real `MultiServiceRouter`, with SNS, Lambda and Amazon SQS on one
//! engine, so the held copy is read where a client reads it — on the dead-letter queue.
//!
//! The RULE asserted here: "RedrivePolicy – When specified, sends undeliverable messages to
//! the specified Amazon SQS dead-letter queue. Messages that can't be delivered due to client
//! errors (for example, when the subscribed endpoint is unreachable) […] are held in the
//! dead-letter queue for further analysis or reprocessing"
//! (`SetSubscriptionAttributesInput$AttributeName`) — what is held is the undeliverable
//! MESSAGE, which is the notification of the publish, the same document every enveloped
//! subscriber of the topic receives. The `Records` array is not part of that message: it is
//! the envelope the INVOKE of a function carries its event in, built by a delivery that in
//! these rows never happened, so it is never what the queue holds.
//!
//! The rows differ along what the rule generalises over: why the invocation could not be made
//! (no policy on the function at all, a grant scoped to another topic, a grant of another
//! action, an endpoint that no longer exists), the form of the published message (plain, with
//! a subject and an attribute, published as a structure of per-protocol branches), whether the
//! subscription asks for a redrive at all, and a delivery that SUCCEEDS and so is never
//! copied. One further test puts a function and a queue subscription — both undeliverable — on
//! one topic, because the rule is about the subscription rather than about the endpoint kind.

use std::time::Instant;

use serde_json::{json, Value};

use crate::lambda_subscription_delivery::{
    emulator, given_function, grant, invoked, lambda, publish, quiet, sns, sqs, subscribe,
    Emulator, DELIVERY_BUDGET, INVOKE_ACTION, LAMBDA_PROTOCOL, LOOK_INTERVAL, SNS_PRINCIPAL,
};
use crate::sqs_subscription_delivery::{element, grant_to_sns};

/// A queue called `name` this service may deliver to: its url and its ARN.
///
/// The policy is the suite-shared grant a queue subscribed to (or dead-lettered from) a topic
/// carries, not a second copy of that document.
///
/// `pub(crate)`: every suite over this harness that needs a queue SNS may write to stands one
/// up through this one definition.
pub(crate) async fn given_queue(emulator: &Emulator, name: &str) -> (String, String) {
    let (url, arn) = given_queue_without_policy(emulator, name).await;
    sqs(
        emulator,
        "SetQueueAttributes",
        &json!({"QueueUrl": url, "Attributes": {"Policy": grant_to_sns(&arn)}}),
    )
    .await;
    (url, arn)
}

/// A queue called `name` carrying NO policy, so no delivery to it can be made: its url and its
/// ARN.
async fn given_queue_without_policy(emulator: &Emulator, name: &str) -> (String, String) {
    let created = sqs(emulator, "CreateQueue", &json!({"QueueName": name})).await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateQueue {name}: no QueueUrl in {created}"))
        .to_string();
    let attributes = sqs(
        emulator,
        "GetQueueAttributes",
        &json!({"QueueUrl": url, "AttributeNames": ["QueueArn"]}),
    )
    .await;
    let arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("GetQueueAttributes {name}: no QueueArn in {attributes}"))
        .to_string();
    (url, arn)
}

/// Ask the subscription at `subscription_arn` to hold undeliverable messages in the queue at
/// `dead_letter_arn`, through the call a client configures a redrive with.
async fn set_redrive(emulator: &Emulator, subscription_arn: &str, dead_letter_arn: &str) {
    let policy = json!({"deadLetterTargetArn": dead_letter_arn}).to_string();
    sns(
        emulator,
        "SetSubscriptionAttributes",
        &[
            ("SubscriptionArn", subscription_arn),
            ("AttributeName", "RedrivePolicy"),
            ("AttributeValue", &policy),
        ],
    )
    .await;
}

/// How long a wait lasts before concluding that NOTHING was held: a park happens on the
/// publishing request, so a copy that is owed is there long before this, and a row that expects
/// none does not pay the whole delivery budget to say so.
const QUIET_BUDGET: std::time::Duration = std::time::Duration::from_millis(1_500);

/// The message bodies waiting on the queue at `url`, waiting until there are `expected` of them
/// — or out the budget, which is how a row expecting NONE concludes there are none.
///
/// `pub(crate)`: every suite over this harness that reads what a redrive held reads it through
/// this one definition, so a row expecting none waits the same way whichever suite asks.
pub(crate) async fn held(emulator: &Emulator, url: &str, expected: usize) -> Vec<String> {
    let deadline = Instant::now()
        + if expected == 0 {
            QUIET_BUDGET
        } else {
            DELIVERY_BUDGET
        };
    let mut collected: Vec<String> = Vec::new();
    loop {
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
        for message in received["Messages"].as_array().cloned().unwrap_or_default() {
            collected.push(message["Body"].as_str().unwrap_or_default().to_string());
        }
        if collected.len() >= expected.max(1) || Instant::now() >= deadline {
            return collected;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// Which source a grant on the function is scoped to.
#[derive(Clone, Copy)]
enum Scope {
    /// The topic the message is published to: the invocation is allowed.
    SubscribedTopic,
    /// Another topic of the same account: this topic's notifications are not allowed.
    OtherTopic,
    /// No `SourceArn` at all.
    Any,
}

/// One row: the document the function carries, what is published, and what the subscription's
/// dead-letter queue must then hold.
struct Row {
    case: &'static str,
    /// The action and scope of the one statement the function carries, or `None` for a function
    /// carrying no policy at all.
    grant: Option<(&'static str, Scope)>,
    /// Whether the function is DELETED after the grant, leaving the endpoint naming nothing.
    delete_endpoint: bool,
    /// Whether the subscription asks for a redrive.
    redrive: bool,
    /// The `Publish` parameters beside the topic.
    params: Vec<(&'static str, &'static str)>,
    /// The `Message` the held notification must carry — the body this subscription's own
    /// protocol receives.
    body: &'static str,
    /// The `Subject` it must carry, if any.
    subject: Option<&'static str>,
    /// The attributes it must carry, as `(name, type, value)`.
    attributes: Vec<(&'static str, &'static str, &'static str)>,
    /// Whether the function is invoked.
    invoked: bool,
    /// Whether the dead-letter queue holds a copy.
    held: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a grant scoped to the topic published to, so the invocation is made",
            grant: Some((INVOKE_ACTION, Scope::SubscribedTopic)),
            delete_endpoint: false,
            redrive: true,
            params: vec![("Message", "delivered body")],
            body: "delivered body",
            subject: None,
            attributes: Vec::new(),
            invoked: true,
            held: false,
        },
        Row {
            case: "no policy on the function at all",
            grant: None,
            delete_endpoint: false,
            redrive: true,
            params: vec![
                ("Message", "ungranted body"),
                ("Subject", "a subject line"),
                ("MessageAttributes.entry.1.Name", "kind"),
                ("MessageAttributes.entry.1.Value.DataType", "String"),
                ("MessageAttributes.entry.1.Value.StringValue", "probe"),
            ],
            body: "ungranted body",
            subject: Some("a subject line"),
            attributes: vec![("kind", "String", "probe")],
            invoked: false,
            held: true,
        },
        Row {
            // "You can define other top-level keys that define the message you want to send to
            // a specific transport protocol" (`PublishInput$MessageStructure`): the body this
            // subscription would have received is the `lambda` branch, so that is the body the
            // undeliverable copy carries.
            case: "a grant scoped to another topic, on a message published as branches",
            grant: Some((INVOKE_ACTION, Scope::OtherTopic)),
            delete_endpoint: false,
            redrive: true,
            params: vec![
                (
                    "Message",
                    "{\"default\":\"the default branch\",\"lambda\":\"the lambda branch\"}",
                ),
                ("MessageStructure", "json"),
            ],
            body: "the lambda branch",
            subject: None,
            attributes: Vec::new(),
            invoked: false,
            held: true,
        },
        Row {
            case: "a grant of another action",
            grant: Some(("lambda:GetFunction", Scope::Any)),
            delete_endpoint: false,
            redrive: true,
            params: vec![("Message", "wrong-action body")],
            body: "wrong-action body",
            subject: None,
            attributes: Vec::new(),
            invoked: false,
            held: true,
        },
        Row {
            case: "an endpoint that no longer exists",
            grant: Some((INVOKE_ACTION, Scope::Any)),
            delete_endpoint: true,
            redrive: true,
            params: vec![("Message", "orphaned body")],
            body: "orphaned body",
            subject: None,
            attributes: Vec::new(),
            invoked: false,
            held: true,
        },
        Row {
            case: "an undeliverable subscription that asks for no redrive",
            grant: None,
            delete_endpoint: false,
            redrive: false,
            params: vec![("Message", "unheld body")],
            body: "unheld body",
            subject: None,
            attributes: Vec::new(),
            invoked: false,
            held: false,
        },
    ]
}

/// Assert that `document` is the notification of a publish of `body`, and NOT the `Records`
/// event envelope an invocation would have carried.
fn assert_notification(case: &str, document: &Value, row: &Row, topic: &str, message_id: &str) {
    assert!(
        document.get("Records").is_none(),
        "case {case:?}: the held copy is the undeliverable message, not the event envelope an \
         invocation carries, got {document}"
    );
    assert_eq!(
        document["Type"].as_str(),
        Some("Notification"),
        "case {case:?}: expected the held copy to be a Notification, got {document}"
    );
    assert_eq!(
        document["MessageId"].as_str(),
        Some(message_id),
        "case {case:?}: expected the publish's own MessageId {message_id:?}, got {document}"
    );
    assert_eq!(
        document["TopicArn"].as_str(),
        Some(topic),
        "case {case:?}: expected the topic published to {topic:?}, got {document}"
    );
    assert_eq!(
        document["Message"].as_str(),
        Some(row.body),
        "case {case:?}: expected the body {:?}, got {document}",
        row.body
    );
    assert_eq!(
        document["Subject"].as_str(),
        row.subject,
        "case {case:?}: expected the subject {:?}, got {document}",
        row.subject
    );
    for (name, data_type, value) in &row.attributes {
        assert_eq!(
            document["MessageAttributes"][*name],
            json!({"Type": data_type, "Value": value}),
            "case {case:?}: expected the attribute {name:?} as the envelope carries it, got \
             {document}"
        );
    }
}

/// Every row: an invocation that could not be made holds the undeliverable NOTIFICATION in the
/// subscription's dead-letter queue, and a delivery that was made holds nothing.
#[tokio::test]
async fn an_uninvokable_function_subscription_holds_the_notification_in_its_dead_letter_queue() {
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let emulator = emulator();
        let topic = element(
            &sns(
                &emulator,
                "CreateTopic",
                &[("Name", &format!("fn-redrive-{index}"))],
            )
            .await,
            "TopicArn",
        );
        let other_topic = element(
            &sns(
                &emulator,
                "CreateTopic",
                &[("Name", &format!("fn-redrive-other-{index}"))],
            )
            .await,
            "TopicArn",
        );
        let (dlq_url, dlq_arn) = given_queue(&emulator, &format!("fn-dead-letters-{index}")).await;

        let function = format!("redrive-probe-{index}");
        let function_arn = given_function(&emulator, &function).await;
        if let Some((action, scope)) = row.grant {
            let source = match scope {
                Scope::SubscribedTopic => Some(topic.as_str()),
                Scope::OtherTopic => Some(other_topic.as_str()),
                Scope::Any => None,
            };
            grant(
                &emulator,
                &function,
                "sns-invoke",
                action,
                SNS_PRINCIPAL,
                source,
            )
            .await;
        }
        let subscription = subscribe(&emulator, &topic, LAMBDA_PROTOCOL, &function_arn).await;
        if row.redrive {
            set_redrive(&emulator, &subscription, &dlq_arn).await;
        }
        if row.delete_endpoint {
            let (status, answer) = lambda(
                &emulator,
                "DELETE",
                &format!("/2015-03-31/functions/{function}"),
                Value::Null,
            )
            .await;
            assert_eq!(
                status, 204,
                "case {case:?}: DeleteFunction expected HTTP 204, got {status} with {answer}"
            );
        }

        let message_id = publish(&emulator, &topic, &row.params).await;

        if row.invoked {
            let events = invoked(&emulator, &function, 1).await;
            assert_eq!(
                events.len(),
                1,
                "case {case:?}: expected exactly one invocation, got {events:?}"
            );
            assert_eq!(
                events[0]["Records"][0]["Sns"]["Message"].as_str(),
                Some(row.body),
                "case {case:?}: expected the published body in the event, got {:?}",
                events[0]
            );
        } else if !row.delete_endpoint {
            let events = quiet(&emulator, &function).await;
            assert!(
                events.is_empty(),
                "case {case:?}: expected the function never to be invoked, got {events:?}"
            );
        }

        let parked = held(&emulator, &dlq_url, usize::from(row.held)).await;
        if !row.held {
            assert!(
                parked.is_empty(),
                "case {case:?}: expected nothing to be held in the dead-letter queue, got \
                 {parked:?}"
            );
            continue;
        }
        assert_eq!(
            parked.len(),
            1,
            "case {case:?}: expected exactly one held copy of the undeliverable message, got \
             {parked:?}"
        );
        let document: Value = serde_json::from_str(&parked[0]).unwrap_or_else(|error| {
            panic!(
                "case {case:?}: the held copy must be a JSON document, got {error} for {:?}",
                parked[0]
            )
        });
        assert_notification(case, &document, row, &topic, &message_id);
        let unsubscribe = document["UnsubscribeURL"].as_str().unwrap_or_default();
        assert!(
            unsubscribe.contains(&format!("SubscriptionArn={subscription}")),
            "case {case:?}: expected the unsubscribe link of the subscription that could not be \
             delivered to ({subscription}), got {unsubscribe:?}"
        );
    }
}

/// One publish, two undeliverable subscriptions of DIFFERENT endpoint kinds, one dead-letter
/// queue: each holds the same undeliverable notification, differing only in the unsubscribe
/// link of the subscription it was owed to — the redrive is about the subscription, not about
/// the endpoint kind.
#[tokio::test]
async fn a_function_and_a_queue_subscription_hold_the_same_undeliverable_notification() {
    let emulator = emulator();
    let topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "mixed-redrive")]).await,
        "TopicArn",
    );
    let (dlq_url, dlq_arn) = given_queue(&emulator, "mixed-dead-letters").await;
    // A queue granting this service nothing: the send to it cannot be made either.
    let (_denied_url, denied_arn) = given_queue_without_policy(&emulator, "mixed-denied").await;

    let function = "mixed-probe";
    let function_arn = given_function(&emulator, function).await;
    let function_subscription =
        subscribe(&emulator, &topic, LAMBDA_PROTOCOL, &function_arn).await;
    set_redrive(&emulator, &function_subscription, &dlq_arn).await;
    let queue_subscription = subscribe(&emulator, &topic, "sqs", &denied_arn).await;
    set_redrive(&emulator, &queue_subscription, &dlq_arn).await;

    let message = "a body nothing could take";
    let message_id = publish(&emulator, &topic, &[("Message", message)]).await;

    let parked = held(&emulator, &dlq_url, 2).await;
    assert_eq!(
        parked.len(),
        2,
        "case \"two undeliverable subscriptions\": expected one held copy per subscription, got \
         {parked:?}"
    );
    let row = Row {
        case: "two undeliverable subscriptions",
        grant: None,
        delete_endpoint: false,
        redrive: true,
        params: vec![("Message", message)],
        body: message,
        subject: None,
        attributes: Vec::new(),
        invoked: false,
        held: true,
    };
    let mut links: Vec<String> = Vec::new();
    for body in &parked {
        let document: Value = serde_json::from_str(body).unwrap_or_else(|error| {
            panic!("the held copy must be a JSON document, got {error} for {body:?}")
        });
        assert_notification(row.case, &document, &row, &topic, &message_id);
        links.push(
            document["UnsubscribeURL"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }
    for subscription in [&function_subscription, &queue_subscription] {
        assert!(
            links
                .iter()
                .any(|link| link.contains(&format!("SubscriptionArn={subscription}"))),
            "case \"two undeliverable subscriptions\": expected a held copy carrying the \
             unsubscribe link of {subscription}, got {links:?}"
        );
    }
}
