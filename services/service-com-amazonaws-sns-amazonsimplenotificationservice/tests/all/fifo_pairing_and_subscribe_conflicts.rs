// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what `Subscribe` refuses about the PAIRING it is asked for and
//! about a subscription that already exists.
//!
//! The rules asserted here:
//!
//! 1. an SQS queue whose name carries the FIFO suffix may be subscribed only to a FIFO
//!    topic — it is delivered to in the published order, which only a FIFO topic's
//!    messages carry — so the pairing with a standard topic is refused
//!    `InvalidParameter` (HTTP 400) before any subscription record or subscription
//!    counter is written, while the same queue on a FIFO topic, and a standard queue on
//!    either topic, are accepted;
//! 2. a FLAG-valued attribute takes the two words and nothing else, and the refusal names
//!    the attribute and echoes the value it rejected in brackets;
//! 3. `Subscribe` is idempotent only in a request that asks for what the subscription
//!    already holds: one asking for any other attribute set is refused, the stored
//!    subscription keeps its attributes and keeps delivering in the format they state,
//!    and the identical request answers the original subscription's ARN.
//!
//! The rows differ along what the rules generalise over: the kind of queue and the kind
//! of topic each pairing names, which value a flag is refused for, and which way a
//! re-subscribe differs from the subscription that already exists (a value of its own, an
//! attribute the subscription does not hold, the removal of one it does).
//!
//! Every call goes over the wire through the router the emulator serves in production, so
//! nothing but persisted state carries information between calls.

use super::topics_subscriptions_publish::{attribute_map, Answer, Sns};

/// The sentence SNS refuses a FIFO SQS queue subscribed to a standard topic with.
const FIFO_PAIRING_REFUSAL: &str = "Invalid parameter: Invalid parameter: Endpoint Reason: FIFO \
                                    SQS Queues can not be subscribed to standard SNS topics";

/// The sentence SNS refuses a re-subscribe that asks for other attributes with.
const CONFLICT_REFUSAL: &str =
    "Invalid parameter: Attributes Reason: Subscription already exists with different attributes";

/// The suffix the name of a FIFO queue — and of a FIFO topic — carries
/// (`CreateQueueInput$QueueName` / `CreateTopicInput$Name` docs).
const FIFO_SUFFIX: &str = ".fifo";

/// Subscribe `endpoint` over `protocol` to `topic` with the attributes given, answering
/// the call.
async fn subscribe(
    sns: &Sns,
    topic: &str,
    protocol: &str,
    endpoint: &str,
    attributes: &[(&str, &str)],
) -> Answer {
    let mut params: Vec<(String, String)> = vec![
        ("TopicArn".to_string(), topic.to_string()),
        ("Protocol".to_string(), protocol.to_string()),
        ("Endpoint".to_string(), endpoint.to_string()),
        ("ReturnSubscriptionArn".to_string(), "true".to_string()),
    ];
    for (index, (name, value)) in attributes.iter().enumerate() {
        params.push((
            format!("Attributes.entry.{}.key", index + 1),
            (*name).to_string(),
        ));
        params.push((
            format!("Attributes.entry.{}.value", index + 1),
            (*value).to_string(),
        ));
    }
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    sns.call("Subscribe", &borrowed).await
}

/// A topic of the given kind, with its ARN: a FIFO topic is created by the attribute SNS
/// documents for it, beside the name suffix it requires.
async fn topic(sns: &Sns, name: &str, fifo: bool) -> String {
    if !fifo {
        return sns.topic(name).await;
    }
    let fifo_name = format!("{name}{FIFO_SUFFIX}");
    let answer = sns
        .call(
            "CreateTopic",
            &[
                ("Name", &fifo_name),
                ("Attributes.entry.1.key", "FifoTopic"),
                ("Attributes.entry.1.value", "true"),
            ],
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "CreateTopic {fifo_name}: expected 200, got {} / {}",
        answer.status, answer.body
    );
    answer
        .element("TopicArn")
        .unwrap_or_else(|| panic!("CreateTopic {fifo_name}: no TopicArn in {}", answer.body))
}

/// The three subscription counters a topic reports.
async fn counters(sns: &Sns, topic: &str) -> (String, String, String) {
    let answer = sns.call("GetTopicAttributes", &[("TopicArn", topic)]).await;
    assert_eq!(
        answer.status, 200,
        "GetTopicAttributes {topic}: expected 200, got {} / {}",
        answer.status, answer.body
    );
    let held = attribute_map(&answer);
    let counter = |name: &str| held.get(name).cloned().unwrap_or_default();
    (
        counter("SubscriptionsConfirmed"),
        counter("SubscriptionsPending"),
        counter("SubscriptionsDeleted"),
    )
}

/// The subscription ARNs a topic lists.
async fn listed(sns: &Sns, topic: &str) -> Vec<String> {
    let answer = sns
        .call("ListSubscriptionsByTopic", &[("TopicArn", topic)])
        .await;
    assert_eq!(
        answer.status, 200,
        "ListSubscriptionsByTopic {topic}: expected 200, got {} / {}",
        answer.status, answer.body
    );
    answer.elements("SubscriptionArn")
}

/// One pairing row: the case, whether the queue is a FIFO one, whether the topic is, and
/// whether SNS subscribes it.
type Pairing = (&'static str, bool, bool, bool);

/// A FIFO queue subscribes only to a FIFO topic; every other pairing of queue and topic
/// kind is accepted, and the refused one writes neither a subscription nor a counter.
#[tokio::test]
async fn a_fifo_queue_subscribes_only_to_a_fifo_topic() {
    let sns = Sns::new();
    let rows: &[Pairing] = &[
        ("a standard queue on a standard topic", false, false, true),
        ("a standard queue on a FIFO topic", false, true, true),
        ("a FIFO queue on a FIFO topic", true, true, true),
        ("a FIFO queue on a standard topic", true, false, false),
    ];

    for (index, (case, fifo_queue, fifo_topic, accepted)) in rows.iter().enumerate() {
        let topic = topic(&sns, &format!("pairing-{index:02}"), *fifo_topic).await;
        let queue = format!(
            "arn:aws:sqs:us-east-1:000000000000:pairing-{index:02}{}",
            if *fifo_queue { FIFO_SUFFIX } else { "" }
        );
        let before = counters(&sns, &topic).await;
        let answer = subscribe(&sns, &topic, "sqs", &queue, &[]).await;

        if *accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} / {}",
                answer.status, answer.body
            );
            let arn = answer.element("SubscriptionArn").unwrap_or_default();
            assert_eq!(
                listed(&sns, &topic).await,
                vec![arn.clone()],
                "case {case:?}: the topic must hold the subscription it accepted"
            );
            let after = counters(&sns, &topic).await;
            assert_eq!(
                after,
                ("1".to_string(), "0".to_string(), "0".to_string()),
                "case {case:?}: the accepted subscription must move the topic's confirmed \
                 count, got {after:?}"
            );
            continue;
        }

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
        assert_eq!(
            answer.element("Message").unwrap_or_default(),
            FIFO_PAIRING_REFUSAL,
            "case {case:?}: expected SNS's own sentence, got {}",
            answer.body
        );
        assert!(
            listed(&sns, &topic).await.is_empty(),
            "case {case:?}: the refused pairing must leave the topic's subscription list \
             empty"
        );
        assert_eq!(
            counters(&sns, &topic).await,
            before,
            "case {case:?}: the refused pairing must move no counter"
        );
    }
}

/// A flag-valued attribute takes the two words in any case and nothing else, and the
/// refusal names the attribute and echoes the value it rejected — framed by the member the
/// value arrived in: the `Attributes` map of `Subscribe`, or the attribute name
/// `SetSubscriptionAttributes` was given.
#[tokio::test]
async fn a_flag_attribute_refusal_echoes_the_value_it_rejected() {
    let sns = Sns::new();
    // The values a flag does NOT take: a word of its own, a number, a quoted literal, a
    // word with punctuation, and the empty value (a flag is no policy document, so an
    // empty value removes nothing and names no word either).
    let refused = ["yes", "no", "1", "0", "\"true\"", "true.", "t r u e", ""];

    for (index, value) in refused.iter().enumerate() {
        let case = format!("RawMessageDelivery of {value:?}");
        let topic = sns.topic(&format!("flag-value-{index:02}")).await;
        let queue = format!("arn:aws:sqs:us-east-1:000000000000:flag-value-{index:02}");
        let reason = format!("RawMessageDelivery: Invalid value [{value}]. Must be true or false.");
        // `Subscribe` read the value out of its `Attributes` map, so the sentence names
        // that member before the attribute.
        let expected = format!("Invalid parameter: Attributes Reason: {reason}");
        // `SetSubscriptionAttributes` was given the attribute BY NAME, so the name is the
        // whole input and the sentence carries no member frame.
        let expected_by_name = format!("Invalid parameter: {reason}");

        let answer = subscribe(
            &sns,
            &topic,
            "sqs",
            &queue,
            &[("RawMessageDelivery", value)],
        )
        .await;
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
        assert_eq!(
            answer.element("Message").unwrap_or_default(),
            expected,
            "case {case:?}: expected {expected:?}, got {}",
            answer.body
        );
        assert!(
            listed(&sns, &topic).await.is_empty(),
            "case {case:?}: the refused Subscribe must leave the topic with no subscriber"
        );

        // `SetSubscriptionAttributes` refuses the same value, stating the same condition
        // under its own member.
        let accepted = subscribe(&sns, &topic, "sqs", &queue, &[("RawMessageDelivery", "true")])
            .await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: the control Subscribe expected 200, got {} / {}",
            accepted.status, accepted.body
        );
        let arn = accepted.element("SubscriptionArn").unwrap_or_default();
        let written = sns
            .call(
                "SetSubscriptionAttributes",
                &[
                    ("SubscriptionArn", &arn),
                    ("AttributeName", "RawMessageDelivery"),
                    ("AttributeValue", value),
                ],
            )
            .await;
        assert_eq!(
            written.status, 400,
            "case {case:?}: SetSubscriptionAttributes expected 400, got {} / {}",
            written.status, written.body
        );
        assert_eq!(
            written.element("Message").unwrap_or_default(),
            expected_by_name,
            "case {case:?}: SetSubscriptionAttributes must state the condition under the \
             attribute it was given, got {}",
            written.body
        );
        // And the accepted word reads back in its canonical form, so the refusal above is
        // the value's doing.
        let held = attribute_map(
            &sns.call("GetSubscriptionAttributes", &[("SubscriptionArn", &arn)])
                .await,
        );
        assert_eq!(
            held.get("RawMessageDelivery").map(String::as_str),
            Some("true"),
            "case {case:?}: the accepted word must read back, got {held:?}"
        );
    }
}

/// One re-subscribe row: the case, the attributes the second call carries, and whether it
/// asks for what the subscription already holds.
type ReSubscribe = (&'static str, &'static [(&'static str, &'static str)], bool);

/// The filter policy the original subscription is created with.
const FILTER_POLICY: &str = r#"{"kind":["alert"]}"#;

/// A re-subscribe of one (topic, protocol, endpoint) triple answers the original
/// subscription only for the attributes it already holds; any other attribute set is
/// refused and the stored subscription is untouched.
#[tokio::test]
async fn a_conflicting_re_subscribe_is_refused_and_the_original_is_unchanged() {
    let sns = Sns::new();
    let rows: &[ReSubscribe] = &[
        (
            "the identical request",
            &[
                ("RawMessageDelivery", "true"),
                ("FilterPolicy", FILTER_POLICY),
            ],
            true,
        ),
        (
            "the same flag written in another case",
            &[
                ("RawMessageDelivery", "TRUE"),
                ("FilterPolicy", FILTER_POLICY),
            ],
            true,
        ),
        ("no attributes at all", &[], true),
        (
            "one of the attributes it holds, at the value it holds",
            &[("RawMessageDelivery", "true")],
            true,
        ),
        (
            "the other value of the flag it holds",
            &[("RawMessageDelivery", "false")],
            false,
        ),
        (
            "another document for the policy it holds",
            &[("FilterPolicy", r#"{"kind":["warning"]}"#)],
            false,
        ),
        (
            "the removal of the policy it holds",
            &[("FilterPolicy", "")],
            false,
        ),
        (
            "an attribute the subscription does not hold",
            &[("FilterPolicyScope", "MessageBody")],
            false,
        ),
        (
            "the flag it holds beside an attribute it does not",
            &[
                ("RawMessageDelivery", "true"),
                ("RedrivePolicy", r#"{"deadLetterTargetArn":"arn:aws:sqs:us-east-1:000000000000:dlq"}"#),
            ],
            false,
        ),
    ];

    for (index, (case, attributes, idempotent)) in rows.iter().enumerate() {
        let topic = sns.topic(&format!("conflict-{index:02}")).await;
        let queue = format!("arn:aws:sqs:us-east-1:000000000000:conflict-{index:02}");
        let first = subscribe(
            &sns,
            &topic,
            "sqs",
            &queue,
            &[
                ("RawMessageDelivery", "true"),
                ("FilterPolicy", FILTER_POLICY),
            ],
        )
        .await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: the first Subscribe expected 200, got {} / {}",
            first.status, first.body
        );
        let arn = first.element("SubscriptionArn").unwrap_or_default();

        let again = subscribe(&sns, &topic, "sqs", &queue, attributes).await;
        if *idempotent {
            assert_eq!(
                again.status, 200,
                "case {case:?}: expected 200, got {} / {}",
                again.status, again.body
            );
            assert_eq!(
                again.element("SubscriptionArn").unwrap_or_default(),
                arn,
                "case {case:?}: an identical re-subscribe must answer the original ARN"
            );
        } else {
            assert_eq!(
                again.status, 400,
                "case {case:?}: expected 400, got {} / {}",
                again.status, again.body
            );
            assert_eq!(
                again.code(),
                "InvalidParameter",
                "case {case:?}: expected the InvalidParameter code, got {}",
                again.body
            );
            assert_eq!(
                again.element("Message").unwrap_or_default(),
                CONFLICT_REFUSAL,
                "case {case:?}: expected SNS's own sentence, got {}",
                again.body
            );
        }

        // Either way the topic holds the ONE subscription the first call created, with
        // the attributes that call gave it and no others.
        assert_eq!(
            listed(&sns, &topic).await,
            vec![arn.clone()],
            "case {case:?}: the topic must still hold exactly the original subscription"
        );
        let held = attribute_map(
            &sns.call("GetSubscriptionAttributes", &[("SubscriptionArn", &arn)])
                .await,
        );
        assert_eq!(
            held.get("RawMessageDelivery").map(String::as_str),
            Some("true"),
            "case {case:?}: the original subscription must keep its flag, got {held:?}"
        );
        assert_eq!(
            held.get("FilterPolicy").map(String::as_str),
            Some(FILTER_POLICY),
            "case {case:?}: the original subscription must keep its policy, got {held:?}"
        );
        assert_eq!(
            held.get("RedrivePolicy"),
            None,
            "case {case:?}: the refused re-subscribe must store no RedrivePolicy, got {held:?}"
        );
        // The filtering scope is the documented default the policy is matched under: a
        // refused re-subscribe asking for the other scope did not change it.
        assert_eq!(
            held.get("FilterPolicyScope").map(String::as_str),
            Some("MessageAttributes"),
            "case {case:?}: the policy must still be matched under the default scope, got \
             {held:?}"
        );
    }
}
