// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the subscription attribute WRITE path:
//! `SetSubscriptionAttributes`, read back through `GetSubscriptionAttributes`.
//!
//! Every call goes over the wire through the router the emulator serves in production,
//! so nothing but persisted state carries information between calls.
//!
//! The rule under test: `SetSubscriptionAttributes` stores the new value of ONE
//! documented attribute of the subscription its ARN names — accepting it when the value
//! satisfies the constraint SNS documents for that attribute and, for the attributes the
//! model scopes to a delivery protocol, the subscription delivers over one of those —
//! holding the canonical form of the value (a flag in lower case, a delivery policy with
//! its retry members filled in), clearing the attribute when the request carries no
//! value, and refusing anything else with HTTP 400 / `InvalidParameter` (a subscription
//! that does not exist: HTTP 404 / `NotFound`) without writing anything.
//!
//! The rows differ in attribute, value form (flag, enumerated word, policy document,
//! ARN), the case the value arrived in, delivery protocol and confirmation state, which
//! is what the rule generalises over; the recorded `RawMessageDelivery` write is one row
//! of it.

use std::collections::HashSet;

use service_com_amazonaws_sns_amazonsimplenotificationservice::state::SUBSCRIPTION_ATTRIBUTE_NAMES;

use super::subscription_attributes::{read, subscribe};
use super::topics_subscriptions_publish::{attribute_entries, attribute_map, Answer, Sns};

/// A filter policy, as the `FilterPolicy` attribute carries one: a JSON object of the
/// message properties to match.
const FILTER_POLICY: &str = r#"{"kind":["alert"]}"#;

/// A dead-letter queue, as the `RedrivePolicy` attribute names one.
const REDRIVE_POLICY: &str = r#"{"deadLetterTargetArn":"arn:aws:sqs:us-east-1:000000000000:dlq"}"#;

/// A delivery policy that overrides one retry member, in the shape a SUBSCRIPTION's
/// `DeliveryPolicy` carries: the retry policy at the top level, as the document the
/// `GetSubscriptionAttributesResponse$Attributes` doc calls "the JSON serialization of
/// the subscription's delivery policy".
const DELIVERY_POLICY: &str = r#"{"healthyRetryPolicy":{"numRetries":7}}"#;

/// The same policy as SNS reports it BACK: the whole shape of the document, with the
/// member the caller named kept and every member it did not supply stated as the `null` it
/// is — the account system defaults belong to the `EffectiveDeliveryPolicy`, not to what
/// the subscription holds.
const NORMALIZED_DELIVERY_POLICY: &str = r#"{"healthyRetryPolicy":{
    "minDelayTarget":null,"maxDelayTarget":null,"numRetries":7,"numMaxDelayRetries":null,
    "numNoDelayRetries":null,"numMinDelayRetries":null,"backoffFunction":null},
    "sicklyRetryPolicy":null,"throttlePolicy":null,"requestPolicy":null,
    "guaranteed":false}"#;

/// A delivery policy that names no retry member at all, and the document it reads back
/// as: the section it does name is kept and every other section is `null`.
const THROTTLE_DELIVERY_POLICY: &str = r#"{"throttlePolicy":{"maxReceivesPerSecond":2}}"#;
const NORMALIZED_THROTTLE_DELIVERY_POLICY: &str = r#"{"healthyRetryPolicy":null,
    "sicklyRetryPolicy":null,"throttlePolicy":{"maxReceivesPerSecond":2},
    "requestPolicy":null,"guaranteed":false}"#;

/// Whether the attribute value SNS reported is the one expected: the same DOCUMENT when
/// both are JSON, because the order a policy's members serialize in is not part of its
/// meaning, and the same text otherwise.
fn same_value(reported: &str, expected: &str) -> bool {
    match (
        serde_json::from_str::<serde_json::Value>(reported),
        serde_json::from_str::<serde_json::Value>(expected),
    ) {
        (Ok(left), Ok(right)) => left == right,
        _ => reported == expected,
    }
}

/// An endpoint of the documented form for `protocol`
/// (`SubscribeInput$Endpoint` doc), so a row names only the protocol it varies.
fn endpoint_for(protocol: &str) -> &'static str {
    match protocol {
        "sqs" => "arn:aws:sqs:us-east-1:000000000000:orders",
        "lambda" => "arn:aws:lambda:us-east-1:000000000000:function:notify",
        "http" => "http://example.test/hook",
        "https" => "https://example.test/hook",
        "email" => "owner@example.test",
        "email-json" => "owner@example.test",
        "sms" => "+44 20 7946 0958",
        "firehose" => "arn:aws:firehose:us-east-1:000000000000:deliverystream/events",
        "application" => "arn:aws:sns:us-east-1:000000000000:endpoint/GCM/app/device-1",
        other => panic!("no documented endpoint form for protocol {other:?}"),
    }
}

/// Write one subscription attribute: `value` of `None` is a request that carries no
/// `AttributeValue` at all, which clears the attribute.
async fn set(sns: &Sns, arn: &str, name: &str, value: Option<&str>) -> Answer {
    let mut params: Vec<(&str, &str)> = vec![("SubscriptionArn", arn), ("AttributeName", name)];
    if let Some(value) = value {
        params.push(("AttributeValue", value));
    }
    sns.call("SetSubscriptionAttributes", &params).await
}

/// One accepted write: the case, the subscription's protocol, the attribute, the value
/// set on it and the value the read reports back (the canonical form of what was
/// written: a flag in lower case, a delivery policy normalized).
type WriteCase<'a> = (&'a str, &'a str, &'a str, &'a str, &'a str);

/// The archive policy an archiving topic holds: a retention period of one day.
const ARCHIVE_POLICY: &str = r#"{"MessageRetentionPeriod":"1"}"#;

/// The topic a row writes its attribute against.
///
/// A plain topic, unless the attribute only applies to a topic that ARCHIVES its
/// messages: a `ReplayPolicy` names a window of the topic's message archive, and
/// "the following attributes apply only to FIFO topics: ArchivePolicy"
/// (`CreateTopicInput$Attributes` doc), so that row gets a FIFO topic with one.
async fn given_topic(sns: &Sns, name: &str, attribute: &str) -> String {
    if attribute != "ReplayPolicy" {
        return sns.topic(name).await;
    }
    let fifo_name = format!("{name}.fifo");
    sns.call(
        "CreateTopic",
        &[
            ("Name", fifo_name.as_str()),
            ("Attributes.entry.1.key", "FifoTopic"),
            ("Attributes.entry.1.value", "true"),
            ("Attributes.entry.2.key", "ArchivePolicy"),
            ("Attributes.entry.2.value", ARCHIVE_POLICY),
        ],
    )
    .await
    .element("TopicArn")
    .expect("the archiving topic ARN")
}

/// The attributes the subscription of a row is created with.
///
/// None, unless the attribute the row writes is only REPORTED beside another: a
/// `FilterPolicyScope` says which part of a message the filter policy is applied to, so
/// it is reported only while the subscription holds a policy to apply.
fn given_attributes(attribute: &str) -> &'static [(&'static str, &'static str)] {
    if attribute == "FilterPolicyScope" {
        &[("FilterPolicy", FILTER_POLICY)]
    } else {
        &[]
    }
}

/// One refused write: the case, the subscription's protocol, the attribute and the
/// value.
type RefusalCase<'a> = (&'a str, &'a str, &'a str, &'a str);

/// A write of a documented attribute with a documented value persists it, the read
/// reports the canonical form of it, a repeat is idempotent, and a request carrying no
/// value clears the attribute — leaving the subscription reporting what it reported
/// before the write.
///
/// The attribute names are the catalog of
/// `SetSubscriptionAttributesInput$AttributeName` (the crate's
/// `SUBSCRIPTION_ATTRIBUTE_NAMES`, asserted covered below); the values are the forms
/// that doc gives for each, in the cases SNS accepts them in — a flag in either case, a
/// delivery policy naming as few of its members as the caller likes.
#[tokio::test]
async fn setting_a_documented_subscription_attribute_persists_and_round_trips() {
    let sns = Sns::new();
    let cases: &[WriteCase<'_>] = &[
        (
            "raw delivery enabled on an sqs subscription",
            "sqs",
            "RawMessageDelivery",
            "true",
            "true",
        ),
        (
            "raw delivery enabled on an https subscription",
            "https",
            "RawMessageDelivery",
            "true",
            "true",
        ),
        (
            "raw delivery enabled on a firehose subscription",
            "firehose",
            "RawMessageDelivery",
            "true",
            "true",
        ),
        // A flag is a boolean carried as a word, so the word is read in any case and
        // reported in the canonical one.
        (
            "raw delivery enabled in capitals",
            "sqs",
            "RawMessageDelivery",
            "TRUE",
            "true",
        ),
        (
            "raw delivery enabled in mixed case",
            "https",
            "RawMessageDelivery",
            "True",
            "true",
        ),
        (
            "raw delivery left disabled on a lambda subscription",
            "lambda",
            "RawMessageDelivery",
            "false",
            "false",
        ),
        (
            "raw delivery disabled in capitals",
            "sqs",
            "RawMessageDelivery",
            "FALSE",
            "false",
        ),
        (
            "a filter policy on a lambda subscription",
            "lambda",
            "FilterPolicy",
            FILTER_POLICY,
            FILTER_POLICY,
        ),
        (
            "a filter policy matching two message attributes",
            "sqs",
            "FilterPolicy",
            r#"{"kind":["alert"],"severity":["high","critical"]}"#,
            r#"{"kind":["alert"],"severity":["high","critical"]}"#,
        ),
        (
            "a filter policy on an email subscription awaiting confirmation",
            "email",
            "FilterPolicy",
            FILTER_POLICY,
            FILTER_POLICY,
        ),
        (
            "a filter policy on an application subscription",
            "application",
            "FilterPolicy",
            FILTER_POLICY,
            FILTER_POLICY,
        ),
        (
            "the message-body filtering scope",
            "sqs",
            "FilterPolicyScope",
            "MessageBody",
            "MessageBody",
        ),
        (
            "the default filtering scope stated explicitly",
            "sqs",
            "FilterPolicyScope",
            "MessageAttributes",
            "MessageAttributes",
        ),
        // A delivery policy is a document every subscription carries, whatever it
        // delivers over, and it reads back as the whole shape with the members nobody
        // supplied nulled.
        (
            "a delivery policy on an https subscription",
            "https",
            "DeliveryPolicy",
            DELIVERY_POLICY,
            NORMALIZED_DELIVERY_POLICY,
        ),
        (
            "a delivery policy on an sqs subscription",
            "sqs",
            "DeliveryPolicy",
            DELIVERY_POLICY,
            NORMALIZED_DELIVERY_POLICY,
        ),
        (
            "a delivery policy on a lambda subscription",
            "lambda",
            "DeliveryPolicy",
            DELIVERY_POLICY,
            NORMALIZED_DELIVERY_POLICY,
        ),
        (
            "a delivery policy naming no retry member at all",
            "sqs",
            "DeliveryPolicy",
            THROTTLE_DELIVERY_POLICY,
            NORMALIZED_THROTTLE_DELIVERY_POLICY,
        ),
        (
            "a delivery policy on an http subscription",
            "http",
            "DeliveryPolicy",
            DELIVERY_POLICY,
            NORMALIZED_DELIVERY_POLICY,
        ),
        (
            "a dead-letter queue on a lambda subscription",
            "lambda",
            "RedrivePolicy",
            REDRIVE_POLICY,
            REDRIVE_POLICY,
        ),
        (
            "a dead-letter queue on an sms subscription",
            "sms",
            "RedrivePolicy",
            REDRIVE_POLICY,
            REDRIVE_POLICY,
        ),
        (
            "a replay policy on an sqs subscription",
            "sqs",
            "ReplayPolicy",
            r#"{"PointType":"Timestamp","StartingPoint":"2026-01-01T00:00:00Z"}"#,
            r#"{"PointType":"Timestamp","StartingPoint":"2026-01-01T00:00:00Z"}"#,
        ),
        (
            "the role a firehose delivery is written under",
            "firehose",
            "SubscriptionRoleArn",
            "arn:aws:iam::000000000000:role/sns-firehose",
            "arn:aws:iam::000000000000:role/sns-firehose",
        ),
    ];

    // Every settable attribute SNS defines is a row of the table above.
    let covered: HashSet<&str> = cases.iter().map(|(_, _, name, _, _)| *name).collect();
    for name in SUBSCRIPTION_ATTRIBUTE_NAMES {
        assert!(
            covered.contains(name),
            "the table must cover every settable subscription attribute; {name} is missing"
        );
    }

    for (index, (case, protocol, name, value, reads_back)) in cases.iter().enumerate() {
        let topic = given_topic(&sns, &format!("set-sub-attr-{index:02}"), name).await;
        let arn = subscribe(
            &sns,
            case,
            &topic,
            protocol,
            endpoint_for(protocol),
            given_attributes(name),
        )
        .await;
        let before = attribute_entries(&read(&sns, case, &arn).await);

        let answer = set(&sns, &arn, name, Some(value)).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: SetSubscriptionAttributes expected 200, got {} / {}",
            answer.status, answer.body
        );

        let written = read(&sns, case, &arn).await;
        let held = attribute_map(&written);
        let reported = held.get(*name).cloned().unwrap_or_default();
        assert!(
            same_value(&reported, reads_back),
            "case {case:?}: {name} must report {reads_back:?}, got {reported:?}"
        );
        // A filter policy is applied under a scope, so a policy set with no explicit
        // scope is reported with the documented default.
        if *name == "FilterPolicy" {
            assert_eq!(
                held.get("FilterPolicyScope").map(String::as_str),
                Some("MessageAttributes"),
                "case {case:?}: a filter policy with no explicit scope is applied under \
                 the documented default scope, got {held:?}"
            );
        }

        // The read is reporting only: a second one sees the same document.
        assert_eq!(
            attribute_entries(&read(&sns, case, &arn).await),
            attribute_entries(&written),
            "case {case:?}: two reads of one subscription must report the same entries"
        );

        // Writing the same value again changes nothing.
        let repeat = set(&sns, &arn, name, Some(value)).await;
        assert_eq!(
            repeat.status, 200,
            "case {case:?}: a repeated write expected 200, got {} / {}",
            repeat.status, repeat.body
        );
        assert_eq!(
            attribute_entries(&read(&sns, case, &arn).await),
            attribute_entries(&written),
            "case {case:?}: a repeated write of the same value must leave the \
             subscription as it was"
        );

        // A request carrying no value clears the attribute, so the subscription reports
        // what it reported before it was ever set — the documented default of a flag,
        // and no entry at all for a policy. The one exception is an attribute the
        // subscription's PROTOCOL requires: a Firehose subscription with no
        // `SubscriptionRoleArn` is one SNS has no identity to deliver as, so the clear
        // is refused and the attribute stays.
        let required_by_the_protocol = *name == "SubscriptionRoleArn";
        let cleared = set(&sns, &arn, name, None).await;
        if required_by_the_protocol {
            assert_eq!(
                cleared.status, 400,
                "case {case:?}: clearing the {name} its protocol requires expected 400, \
                 got {} / {}",
                cleared.status, cleared.body
            );
            let after = read(&sns, case, &arn).await;
            assert_eq!(
                attribute_map(&after).get(*name).cloned(),
                Some((*value).to_string()),
                "case {case:?}: a refused clear must leave {name} as it was, got {:?}",
                attribute_map(&after)
            );
            continue;
        }
        assert_eq!(
            cleared.status, 200,
            "case {case:?}: clearing {name} expected 200, got {} / {}",
            cleared.status, cleared.body
        );
        let after = read(&sns, case, &arn).await;
        assert_eq!(
            attribute_entries(&after),
            before,
            "case {case:?}: clearing {name} must leave the subscription reporting what \
             it reported before the write, got {:?}",
            attribute_map(&after)
        );
    }
}

/// A value SNS does not accept for the attribute, or an attribute SNS does not define
/// for the subscription's delivery protocol, is refused with HTTP 400 /
/// `InvalidParameter` — and nothing is written.
///
/// The protocol rows come from the scoping the model states: raw message delivery is
/// asked for on the endpoints that take bare message bodies, and `SubscriptionRoleArn`
/// "applies only to Amazon Data Firehose delivery stream subscriptions"
/// (`SetSubscriptionAttributesInput$AttributeName` doc).
#[tokio::test]
async fn setting_a_subscription_attribute_sns_does_not_accept_is_refused() {
    let sns = Sns::new();
    let cases: &[RefusalCase<'_>] = &[
        // Names SNS does not define as a settable subscription attribute, including the
        // properties it COMPUTES and an attribute of the other resource kind.
        ("an attribute name SNS defines for nothing", "sqs", "Nonsense", "x"),
        ("a computed property", "sqs", "Protocol", "lambda"),
        (
            "the subscription's own ARN",
            "sqs",
            "SubscriptionArn",
            "arn:aws:sns:us-east-1:000000000000:t:11111111",
        ),
        ("a topic attribute", "sqs", "DisplayName", "orders"),
        ("the owning account", "sqs", "Owner", "000000000000"),
        // A flag takes the two words, in any case, and nothing else.
        ("raw delivery as a word of its own", "sqs", "RawMessageDelivery", "yes"),
        ("raw delivery as a number", "sqs", "RawMessageDelivery", "1"),
        // Raw delivery is asked for on the endpoints that take the bare bodies — a
        // queue, an HTTP/S endpoint, a delivery stream — and refused on the rest; the
        // recorded case is the lambda row.
        (
            "raw delivery on a lambda subscription",
            "lambda",
            "RawMessageDelivery",
            "true",
        ),
        ("raw delivery on an email subscription", "email", "RawMessageDelivery", "true"),
        ("raw delivery on an sms subscription", "sms", "RawMessageDelivery", "true"),
        (
            "raw delivery on an application subscription",
            "application",
            "RawMessageDelivery",
            "true",
        ),
        // A filter policy is a JSON OBJECT of properties to match.
        ("a filter policy that is a list", "sqs", "FilterPolicy", r#"["alert"]"#),
        ("a filter policy that is a string", "sqs", "FilterPolicy", r#""alert""#),
        ("a filter policy that is not JSON", "sqs", "FilterPolicy", "kind=alert"),
        // The filtering scope is one of two words.
        ("a filtering scope that is not one of the two", "sqs", "FilterPolicyScope", "MessageBodies"),
        ("a filtering scope in the wrong case", "sqs", "FilterPolicyScope", "messagebody"),
        // A delivery policy has to be a document the stored, normalized policy can be
        // computed from — whichever protocol the subscription delivers over.
        ("a delivery policy that is a bare number", "https", "DeliveryPolicy", "7"),
        ("a delivery policy that is a list", "sqs", "DeliveryPolicy", "[]"),
        (
            "a delivery policy whose member is of the wrong type",
            "https",
            "DeliveryPolicy",
            r#"{"healthyRetryPolicy":{"numRetries":"many"}}"#,
        ),
        (
            "a delivery policy whose retry policy is not a document",
            "sqs",
            "DeliveryPolicy",
            r#"{"healthyRetryPolicy":7}"#,
        ),
        // A redrive policy names an Amazon SQS dead-letter queue.
        ("a redrive policy naming no queue", "sqs", "RedrivePolicy", "{}"),
        (
            "a redrive policy whose target is not an ARN",
            "sqs",
            "RedrivePolicy",
            r#"{"deadLetterTargetArn":"dlq"}"#,
        ),
        (
            "a redrive policy whose target is not a queue",
            "sqs",
            "RedrivePolicy",
            r#"{"deadLetterTargetArn":"arn:aws:sns:us-east-1:000000000000:dlq"}"#,
        ),
        ("a redrive policy that is a list", "sqs", "RedrivePolicy", "[]"),
        // A replay policy is a document too.
        ("a replay policy that is a string", "sqs", "ReplayPolicy", r#""now""#),
        // A replay policy names a window of the TOPIC's message archive, so it cannot be
        // set on a subscription to a topic that archives nothing.
        (
            "a replay policy on a subscription to a topic with no archive policy",
            "sqs",
            "ReplayPolicy",
            r#"{"PointType":"Timestamp","StartingPoint":"2026-01-01T00:00:00Z"}"#,
        ),
        // A subscription role is an ARN, and applies to Firehose subscriptions alone.
        (
            "a subscription role that is not an ARN",
            "firehose",
            "SubscriptionRoleArn",
            "sns-firehose",
        ),
        (
            "a subscription role on an sqs subscription",
            "sqs",
            "SubscriptionRoleArn",
            "arn:aws:iam::000000000000:role/sns-firehose",
        ),
        (
            "a subscription role on a lambda subscription",
            "lambda",
            "SubscriptionRoleArn",
            "arn:aws:iam::000000000000:role/sns-firehose",
        ),
    ];

    for (index, (case, protocol, name, value)) in cases.iter().enumerate() {
        let topic = sns.topic(&format!("refused-sub-attr-{index:02}")).await;
        let arn = subscribe(
            &sns,
            case,
            &topic,
            protocol,
            endpoint_for(protocol),
            &[],
        )
        .await;
        let before = attribute_entries(&read(&sns, case, &arn).await);

        let answer = set(&sns, &arn, name, Some(value)).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            "InvalidParameter",
            "case {case:?}: expected the InvalidParameter code, got {}",
            answer.body
        );

        // A refused write persists nothing.
        let after = read(&sns, case, &arn).await;
        assert_eq!(
            attribute_entries(&after),
            before,
            "case {case:?}: a refused write must leave the subscription as it was, got \
             {:?}",
            attribute_map(&after)
        );
    }
}

/// An attribute refused because the SUBSCRIPTION's delivery protocol is not one SNS
/// documents it for is refused with SNS's own whole sentence — `Delivery protocol
/// [<protocol>] does not support <feature>.` — and not with the `<member> Reason:
/// <attribute>: ` frame this crate states a bad member VALUE in: the attribute's value
/// broke no constraint of its own, the subscription it was set on is what refused it.
/// What the sentence is reported UNDER is the member the attribute arrived in: given by
/// name it stands alone, and carried in a `Subscribe` request's `Attributes` map it is
/// reported under that member, which is the member the caller has to change.
///
/// The rows differ in the protocol refused, the attribute and so the feature, and the
/// writing operation, which is what the rule generalises over; the recorded
/// `RawMessageDelivery` write on a `lambda` subscription is one row of it. The feature
/// phrase per attribute comes from the scoping sentences quoted on the crate's
/// `RAW_MESSAGE_DELIVERY_PROTOCOLS` and `SUBSCRIPTION_ROLE_ARN_PROTOCOLS` — the two
/// attributes the model scopes to a protocol.
#[tokio::test]
async fn a_protocol_scoped_attribute_is_refused_with_the_whole_sentence() {
    let sns = Sns::new();
    // case, protocol, attribute, value, the feature the sentence names.
    let cases: &[(&str, &str, &str, &str, &str)] = &[
        (
            "raw delivery on a lambda subscription",
            "lambda",
            "RawMessageDelivery",
            "true",
            "raw message delivery",
        ),
        (
            "raw delivery on an email subscription",
            "email",
            "RawMessageDelivery",
            "true",
            "raw message delivery",
        ),
        (
            "raw delivery on an email-json subscription",
            "email-json",
            "RawMessageDelivery",
            "true",
            "raw message delivery",
        ),
        (
            "raw delivery on an sms subscription",
            "sms",
            "RawMessageDelivery",
            "true",
            "raw message delivery",
        ),
        (
            "raw delivery on an application subscription",
            "application",
            "RawMessageDelivery",
            "true",
            "raw message delivery",
        ),
        // The case the flag arrived in does not change the sentence: it is the
        // subscription that refuses the attribute, not its value.
        (
            "raw delivery in capitals on a lambda subscription",
            "lambda",
            "RawMessageDelivery",
            "TRUE",
            "raw message delivery",
        ),
        (
            "a subscription role on an https subscription",
            "https",
            "SubscriptionRoleArn",
            "arn:aws:iam::000000000000:role/sns-firehose",
            "a subscription role",
        ),
        (
            "a subscription role on a lambda subscription",
            "lambda",
            "SubscriptionRoleArn",
            "arn:aws:iam::000000000000:role/sns-firehose",
            "a subscription role",
        ),
    ];

    for (index, (case, protocol, name, value, feature)) in cases.iter().enumerate() {
        let sentence = format!("Delivery protocol [{protocol}] does not support {feature}.");
        // Given the attribute BY NAME, the sentence is the whole message.
        let want = format!("Invalid parameter: {sentence}");
        // Carried in the `Attributes` map, the same sentence is reported under that
        // member. Recorded evidence: a `Subscribe` enabling raw message delivery on a
        // `lambda` endpoint answers `Invalid parameter: Attributes Reason: ` and this
        // sentence.
        let want_on_subscribe = format!("Invalid parameter: Attributes Reason: {sentence}");
        let topic = sns.topic(&format!("protocol-scope-{index:02}")).await;

        // The write path the recorded check drove: SetSubscriptionAttributes.
        let arn = subscribe(&sns, case, &topic, protocol, endpoint_for(protocol), &[]).await;
        let answer = set(&sns, &arn, name, Some(value)).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            "InvalidParameter",
            "case {case:?}: expected the InvalidParameter code, got {}",
            answer.body
        );
        let message = answer.element("Message").unwrap_or_default();
        assert_eq!(
            message, want,
            "case {case:?}: expected the message {want:?}, got {message:?}"
        );

        // And the same sentence, under the member it arrived in, when Subscribe carries
        // the attribute instead.
        let on_subscribe = sns
            .call(
                "Subscribe",
                &[
                    ("TopicArn", &topic),
                    ("Protocol", protocol),
                    ("Endpoint", endpoint_for(protocol)),
                    ("ReturnSubscriptionArn", "true"),
                    ("Attributes.entry.1.key", name),
                    ("Attributes.entry.1.value", value),
                ],
            )
            .await;
        assert_eq!(
            on_subscribe.status, 400,
            "case {case:?}: Subscribe expected 400, got {} / {}",
            on_subscribe.status, on_subscribe.body
        );
        let subscribe_message = on_subscribe.element("Message").unwrap_or_default();
        assert_eq!(
            subscribe_message, want_on_subscribe,
            "case {case:?}: Subscribe expected the message {want_on_subscribe:?}, got \
             {subscribe_message:?}"
        );
    }
}

/// `SetSubscriptionAttributes` answers 404 / `NotFound` for a well-formed subscription
/// ARN that names no subscription this emulator holds, and 400 / `InvalidParameter` for
/// a value that is not a subscription ARN at all — including the sentinels SNS publishes
/// in place of an unconfirmed subscription's ARN.
#[tokio::test]
async fn setting_an_attribute_of_a_subscription_that_is_not_there_is_refused() {
    let sns = Sns::new();
    let topic = sns.topic("set-attrs-absent-subscription").await;
    let unsubscribed = subscribe(
        &sns,
        "unsubscribed",
        &topic,
        "sqs",
        "arn:aws:sqs:us-east-1:000000000000:gone",
        &[],
    )
    .await;
    let gone = sns
        .call("Unsubscribe", &[("SubscriptionArn", &unsubscribed)])
        .await;
    assert_eq!(
        gone.status, 200,
        "Unsubscribe must succeed, got {} / {}",
        gone.status, gone.body
    );

    let absent = [
        ("the subscription was unsubscribed", unsubscribed.clone()),
        (
            "an id no subscription of this topic carries",
            format!("{topic}:00000000-0000-4000-8000-000000000000"),
        ),
        (
            "a topic that does not exist",
            "arn:aws:sns:us-east-1:000000000000:absent-topic:\
             11111111-1111-4111-8111-111111111111"
                .to_string(),
        ),
    ];
    for (case, arn) in &absent {
        let answer = set(&sns, arn, "RawMessageDelivery", Some("true")).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            "NotFound",
            "case {case:?}: expected the NotFound code, got {}",
            answer.body
        );
    }

    let malformed = [
        "not-an-arn-at-all",
        "arn:aws:sns",
        "arn:aws:sns:us-east-1:000000000000:a-topic",
        "arn:aws:sns:us-east-1:000000000000:a-topic:",
        "arn:aws:sqs:us-east-1:000000000000:q:11111111",
        "arn:aws:sns:us-east-1:000000000000:two words:11111111",
        // A subscription of another region: this endpoint does not reach it, so the write
        // is refused rather than reported as a subscription that does not exist.
        "arn:aws:sns:eu-west-1:123456789012:other-topic:22222222-2222-4222-8222-222222222222",
        "pending confirmation",
        "PendingConfirmation",
    ];
    for arn in malformed {
        let answer = set(&sns, arn, "RawMessageDelivery", Some("true")).await;
        assert_eq!(
            answer.status, 400,
            "case {arn:?}: expected 400, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            "InvalidParameter",
            "case {arn:?}: expected the InvalidParameter code, got {}",
            answer.body
        );
    }
}

/// `Subscribe` holds the attributes it carries to the SAME two rules, so an attribute
/// one call accepts is not one the other refuses.
#[tokio::test]
async fn subscribe_holds_its_attributes_to_the_same_rules() {
    let sns = Sns::new();
    let cases: &[(&str, &str, &str, &str, bool)] = &[
        (
            "raw delivery on an sqs subscription",
            "sqs",
            "RawMessageDelivery",
            "true",
            true,
        ),
        (
            "raw delivery on a lambda subscription",
            "lambda",
            "RawMessageDelivery",
            "true",
            false,
        ),
        (
            "raw delivery as a number",
            "sqs",
            "RawMessageDelivery",
            "1",
            false,
        ),
        (
            "a filter policy that is a list",
            "sqs",
            "FilterPolicy",
            r#"["alert"]"#,
            false,
        ),
        (
            "a filter policy that is an object",
            "sqs",
            "FilterPolicy",
            FILTER_POLICY,
            true,
        ),
        (
            "a subscription role on a lambda subscription",
            "lambda",
            "SubscriptionRoleArn",
            "arn:aws:iam::000000000000:role/sns-firehose",
            false,
        ),
        (
            "raw delivery in capitals on an sqs subscription",
            "sqs",
            "RawMessageDelivery",
            "TRUE",
            true,
        ),
        (
            "raw delivery on a firehose subscription",
            "firehose",
            "RawMessageDelivery",
            "true",
            true,
        ),
        (
            "a delivery policy on an sqs subscription",
            "sqs",
            "DeliveryPolicy",
            DELIVERY_POLICY,
            true,
        ),
        (
            "a delivery policy that is a bare number",
            "sqs",
            "DeliveryPolicy",
            "7",
            false,
        ),
    ];

    // Each row subscribes its own TOPIC: `Subscribe` is idempotent only in a request
    // that asks for what the subscription already holds, so one row's attributes must
    // not be asked of the subscription another row created.
    for (index, (case, protocol, name, value, accepted)) in cases.iter().enumerate() {
        let topic = sns
            .topic(&format!("subscribe-attribute-rules-{index:02}"))
            .await;
        let mut params: Vec<(&str, &str)> = vec![
            ("TopicArn", &topic),
            ("Protocol", protocol),
            ("Endpoint", endpoint_for(protocol)),
            ("ReturnSubscriptionArn", "true"),
            ("Attributes.entry.1.key", name),
            ("Attributes.entry.1.value", value),
        ];
        // The role a Firehose subscription is required to carry, where the row does not
        // write it itself, so these rows turn on the attribute under test alone.
        if *protocol == "firehose" && *name != "SubscriptionRoleArn" {
            params.push(("Attributes.entry.2.key", "SubscriptionRoleArn"));
            params.push((
                "Attributes.entry.2.value",
                "arn:aws:iam::000000000000:role/sns-firehose",
            ));
        }
        let answer = sns.call("Subscribe", &params).await;
        if *accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} / {}",
                answer.status, answer.body
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected 400, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                "InvalidParameter",
                "case {case:?}: expected the InvalidParameter code, got {}",
                answer.body
            );
        }
    }
}
