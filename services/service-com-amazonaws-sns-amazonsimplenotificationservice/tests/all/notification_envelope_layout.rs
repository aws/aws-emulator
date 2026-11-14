// SPDX-License-Identifier: CC0-1.0

//! The LAYOUT of the notification a non-raw subscriber receives: which members it carries,
//! in which order, and the text it is handed as.
//!
//! The RULE asserted here: a non-raw SNS notification is a JSON document whose members
//! stand in ONE fixed order — the message's metadata (`Type`, `MessageId`, `TopicArn`,
//! `Subject` when the publish carried one, `Message`, `Timestamp`), then the three members a
//! subscriber verifies it with (`SignatureVersion`, `Signature` and the signing
//! certificate's URL), then the link that cancels the receiving subscription, then the
//! published message attributes — and the three signature members are on EVERY notification,
//! because they are what "lets you verify the authenticity of the message" (the Amazon SNS
//! Developer Guide, "Verifying the signatures of Amazon SNS messages", and the envelope
//! samples of "Parsing message formats"). A queue subscriber receives that document as its
//! message BODY, in the layout those samples show: one member per line, two-space
//! indentation, and a space on each side of the colon.
//!
//! The rows differ along what the rule generalises over: whether the publish carried a
//! subject, whether it carried message attributes, how many it carried, and the endpoint
//! kind the notification is rendered for (a queue's message body and a function's event
//! record, which spell the two URL members differently).
//!
//! The `Timestamp` is part of that layout: UTC to the millisecond, with exactly three
//! fractional digits — the width every published sample writes, and the one a subscriber
//! parsing the member with a fixed-width millisecond format reads.

use super::sqs_subscription_delivery::{drain, element, emulator, given_queue, sns};

/// The members of a queue subscriber's notification, in the order the document carries
/// them. A member a notification does not carry is absent, and the order of the rest stands.
const ENVELOPE_LAYOUT: &[&str] = &[
    "Type",
    "MessageId",
    "TopicArn",
    "Subject",
    "Message",
    "Timestamp",
    "SignatureVersion",
    "Signature",
    "SigningCertURL",
    "UnsubscribeURL",
    "MessageAttributes",
];

/// The same layout as a FUNCTION's event record spells it: the two URL members carry `Url`
/// where the JSON envelope carries `URL`.
const EVENT_RECORD_LAYOUT: &[&str] = &[
    "Type",
    "MessageId",
    "TopicArn",
    "Subject",
    "Message",
    "Timestamp",
    "SignatureVersion",
    "Signature",
    "SigningCertUrl",
    "UnsubscribeUrl",
    "MessageAttributes",
];

/// The three members a subscriber verifies a notification with, which every notification
/// carries.
const SEALING_MEMBERS: &[&str] = &["SignatureVersion", "Signature"];

/// One publish: what it carries beside the body, and which members its notification must
/// then hold.
struct Published {
    case: &'static str,
    subject: Option<&'static str>,
    /// The message attributes it carries, as `(name, data type, value)`.
    attributes: &'static [(&'static str, &'static str, &'static str)],
}

/// The rows: a bare publish, one with a subject, one with attributes, one with both, and one
/// with several attributes.
const PUBLISHES: &[Published] = &[
    Published {
        case: "a bare publish",
        subject: None,
        attributes: &[],
    },
    Published {
        case: "a publish carrying a subject",
        subject: Some("the subject"),
        attributes: &[],
    },
    Published {
        case: "a publish carrying one message attribute",
        subject: None,
        attributes: &[("tier", "String", "gold")],
    },
    Published {
        case: "a publish carrying a subject and attributes",
        subject: Some("both"),
        attributes: &[("tier", "String", "gold"), ("count", "Number", "7")],
    },
];

/// The notification a queue subscriber receives carries the whole member set, in the layout
/// order, as the text those members are laid out in.
#[tokio::test]
async fn the_notification_a_queue_receives_is_laid_out_in_the_canonical_order() {
    let emulator = emulator();
    for (at, row) in PUBLISHES.iter().enumerate() {
        let case = row.case;
        let name = format!("layout-{at}");
        let topic = element(
            &sns(&emulator, "CreateTopic", &[("Name", &name)]).await,
            "TopicArn",
        );
        let (url, arn) = given_queue(&emulator, &format!("{name}-queue")).await;
        sns(
            &emulator,
            "Subscribe",
            &[
                ("TopicArn", &topic),
                ("Protocol", "sqs"),
                ("Endpoint", &arn),
                ("ReturnSubscriptionArn", "true"),
            ],
        )
        .await;

        let mut params: Vec<(String, String)> = vec![
            ("TopicArn".to_string(), topic.clone()),
            ("Message".to_string(), format!("body of {case}")),
        ];
        if let Some(subject) = row.subject {
            params.push(("Subject".to_string(), subject.to_string()));
        }
        for (entry, (attribute, data_type, value)) in row.attributes.iter().enumerate() {
            let at = entry + 1;
            params.push((
                format!("MessageAttributes.entry.{at}.Name"),
                (*attribute).to_string(),
            ));
            params.push((
                format!("MessageAttributes.entry.{at}.Value.DataType"),
                (*data_type).to_string(),
            ));
            params.push((
                format!("MessageAttributes.entry.{at}.Value.StringValue"),
                (*value).to_string(),
            ));
        }
        let borrowed: Vec<(&str, &str)> = params
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        sns(&emulator, "Publish", &borrowed).await;

        let delivered = drain(&emulator, &url).await;
        assert_eq!(
            delivered.len(),
            1,
            "case {case:?}: expected exactly one delivery, got {delivered:?}"
        );
        let body = delivered[0].0.clone();
        let envelope: serde_json::Value = serde_json::from_str(&body)
            .unwrap_or_else(|error| panic!("case {case:?}: {error} for {body}"));
        let document = envelope
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: the body must be a JSON object, got {body}"));

        // Every member the notification carries stands where the layout puts it, and the
        // ones the publish gave it are all there.
        let carried: Vec<&str> = document.keys().map(String::as_str).collect();
        let expected: Vec<&str> = ENVELOPE_LAYOUT
            .iter()
            .copied()
            .filter(|member| document.contains_key(*member))
            .collect();
        assert_eq!(
            carried, expected,
            "case {case:?}: the notification's members must stand in the canonical order, \
             got {carried:?} in {body}"
        );
        for member in SEALING_MEMBERS.iter().chain(["SigningCertURL"].iter()) {
            assert!(
                document
                    .get(*member)
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|value| !value.is_empty()),
                "case {case:?}: every notification carries {member}, got {body}"
            );
        }
        assert_eq!(
            document.contains_key("Subject"),
            row.subject.is_some(),
            "case {case:?}: Subject is carried exactly when the publish had one, got {body}"
        );
        assert_eq!(
            document.contains_key("MessageAttributes"),
            !row.attributes.is_empty(),
            "case {case:?}: MessageAttributes is carried exactly when the publish had any, \
             got {body}"
        );
        assert_millisecond_utc(
            case,
            document
                .get("Timestamp")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default(),
        );

        // And the TEXT is the layout a subscriber reads: one member per line, indented by
        // two spaces, with a space on each side of the colon.
        let mut lines = body.lines();
        assert_eq!(
            lines.next(),
            Some("{"),
            "case {case:?}: the body opens the document on its own line, got {body}"
        );
        assert_eq!(
            lines.next(),
            Some("  \"Type\" : \"Notification\","),
            "case {case:?}: each member stands on its own indented line, got {body}"
        );
        assert!(
            body.ends_with("\n}"),
            "case {case:?}: the body closes the document on its own line, got {body}"
        );
        for member in document.keys() {
            assert!(
                body.contains(&format!("\n  \"{member}\" : ")),
                "case {case:?}: {member} must stand on its own indented line, got {body}"
            );
        }
        // An attribute's own value pair stands on the attribute map's line, as the samples
        // write it: `"<name>" : {"Type":"<type>","Value":"<value>"}`.
        for (attribute, data_type, value) in row.attributes {
            assert!(
                body.contains(&format!(
                    "\n    \"{attribute}\" : {{\"Type\":\"{data_type}\",\"Value\":\"{value}\"}}"
                )),
                "case {case:?}: the attribute {attribute:?} must stand on one line of the \
                 attribute map, got {body}"
            );
        }
    }
}

/// The record a subscribed FUNCTION is handed carries the same members in the same order,
/// under the names a function's event spells the two URL members by.
#[tokio::test]
async fn the_record_a_function_receives_carries_the_same_layout() {
    use super::lambda_subscription_delivery::{
        given_function, grant, invoked, publish, subscribe, INVOKE_ACTION, LAMBDA_PROTOCOL,
        SNS_PRINCIPAL,
    };

    let emulator = super::lambda_subscription_delivery::emulator();
    let topic = element(
        &super::lambda_subscription_delivery::sns(
            &emulator,
            "CreateTopic",
            &[("Name", "record-layout")],
        )
        .await,
        "TopicArn",
    );
    let function = "record-layout-probe";
    let function_arn = given_function(&emulator, function).await;
    grant(
        &emulator,
        function,
        "sns-invoke",
        INVOKE_ACTION,
        SNS_PRINCIPAL,
        Some(&topic),
    )
    .await;
    subscribe(&emulator, &topic, LAMBDA_PROTOCOL, &function_arn).await;
    publish(
        &emulator,
        &topic,
        &[
            ("Message", "the body"),
            ("Subject", "the subject"),
            ("MessageAttributes.entry.1.Name", "tier"),
            ("MessageAttributes.entry.1.Value.DataType", "String"),
            ("MessageAttributes.entry.1.Value.StringValue", "gold"),
        ],
    )
    .await;

    let events = invoked(&emulator, function, 1).await;
    let notification = &events[0]["Records"][0]["Sns"];
    let document = notification
        .as_object()
        .unwrap_or_else(|| panic!("the record must carry an Sns object, got {}", events[0]));
    let carried: Vec<&str> = document.keys().map(String::as_str).collect();
    let expected: Vec<&str> = EVENT_RECORD_LAYOUT
        .iter()
        .copied()
        .filter(|member| document.contains_key(*member))
        .collect();
    assert_eq!(
        carried, expected,
        "the record's members must stand in the canonical order, got {carried:?}"
    );
    for member in SEALING_MEMBERS.iter().chain(["SigningCertUrl"].iter()) {
        assert!(
            document
                .get(*member)
                .and_then(serde_json::Value::as_str)
                .is_some_and(|value| !value.is_empty()),
            "every record carries {member}, got {notification}"
        );
    }
    assert_millisecond_utc(
        "a function's event record",
        document
            .get("Timestamp")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default(),
    );
}

/// Assert that `stamped` is an instant in UTC written to the millisecond:
/// `YYYY-MM-DDTHH:MM:SS.mmmZ`, with exactly three fractional digits and the `Z` suffix.
fn assert_millisecond_utc(case: &str, stamped: &str) {
    let shape: String = stamped
        .chars()
        .map(|c| if c.is_ascii_digit() { 'd' } else { c })
        .collect();
    assert_eq!(
        shape, "dddd-dd-ddTdd:dd:dd.dddZ",
        "case {case:?}: the Timestamp must be UTC to the millisecond, got {stamped:?}"
    );
}
