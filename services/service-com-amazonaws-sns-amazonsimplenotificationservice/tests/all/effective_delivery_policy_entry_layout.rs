// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the attribute map `GetSubscriptionAttributes` reports for an
//! HTTP/S subscription: the `EffectiveDeliveryPolicy` document, and the ENTRY LAYOUT the
//! map is reported in.
//!
//! The rules asserted here:
//!
//! 1. the `EffectiveDeliveryPolicy` entry carries "the JSON serialization of the
//!    effective delivery policy that takes into account the topic delivery policy and
//!    account system defaults" (`GetSubscriptionAttributesResponse$Attributes` doc) —
//!    the WHOLE document: the retry policy with every member, the `sicklyRetryPolicy`
//!    and `throttlePolicy` placeholders, the `requestPolicy` section that fixes the
//!    content type of the delivered notification, and the delivery guarantee. The
//!    numbers in it are the only variable part: they are the account defaults with the
//!    topic's and then the subscription's own `DeliveryPolicy` laid over them;
//! 2. the map is an entry LIST on the wire, and SNS reports its entries in its own
//!    stable sequence ([`REPORTED_SUBSCRIPTION_ATTRIBUTE_ORDER`]) rather than in an
//!    alphabetical one, so a client that walks the entries positionally reads the same
//!    attribute at the same position on every call.
//!
//! The rows differ along what the rules generalise over: the delivery protocol
//! (`http` and `https`, and `sqs` as the control that reports no effective policy at
//! all), whose `DeliveryPolicy` is laid over the defaults (nobody's, the topic's, the
//! subscription's, and both), and the retry numbers each of those sets.
//!
//! Every call goes over the wire through the router the emulator serves in production,
//! so nothing but persisted state carries information between calls.

use serde_json::{json, Value};

use service_com_amazonaws_sns_amazonsimplenotificationservice::state::REPORTED_SUBSCRIPTION_ATTRIBUTE_ORDER;

use super::topics_subscriptions_publish::{attribute_entries, Answer, Sns};

/// The content type SNS delivers an HTTP/S notification under by default, as the
/// `requestPolicy` section of a recorded effective delivery policy states it.
const DEFAULT_CONTENT_TYPE: &str = "text/plain; charset=UTF-8";

/// Subscribe `endpoint` over `protocol` with the attributes given, answering the ARN.
async fn subscribe(
    sns: &Sns,
    topic: &str,
    protocol: &str,
    endpoint: &str,
    attributes: &[(&str, &str)],
) -> String {
    let mut params: Vec<(String, String)> = vec![
        ("TopicArn".to_string(), topic.to_string()),
        ("Protocol".to_string(), protocol.to_string()),
        ("Endpoint".to_string(), endpoint.to_string()),
        ("ReturnSubscriptionArn".to_string(), "true".to_string()),
    ];
    for (at, (key, value)) in attributes.iter().enumerate() {
        params.push((format!("Attributes.entry.{}.key", at + 1), (*key).to_string()));
        params.push((
            format!("Attributes.entry.{}.value", at + 1),
            (*value).to_string(),
        ));
    }
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let answer = sns.call("Subscribe", &borrowed).await;
    assert_eq!(
        answer.status, 200,
        "Subscribe over {protocol} must be served, got {} / {}",
        answer.status, answer.body
    );
    answer
        .element("SubscriptionArn")
        .unwrap_or_else(|| panic!("no SubscriptionArn in {}", answer.body))
}

/// One `GetSubscriptionAttributes` call, asserting it was served.
async fn read(sns: &Sns, case: &str, arn: &str) -> Answer {
    let answer = sns
        .call("GetSubscriptionAttributes", &[("SubscriptionArn", arn)])
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200, got {} / {}",
        answer.status, answer.body
    );
    answer
}

/// The document a retry policy of the given numbers belongs to: the whole effective
/// policy, as SNS serializes it for a subscription.
fn expected_document(
    min_delay: i64,
    max_delay: i64,
    retries: i64,
    backoff: &str,
) -> Value {
    json!({
        "healthyRetryPolicy": {
            "minDelayTarget": min_delay,
            "maxDelayTarget": max_delay,
            "numRetries": retries,
            "numMaxDelayRetries": 0,
            "numNoDelayRetries": 0,
            "numMinDelayRetries": 0,
            "backoffFunction": backoff,
        },
        "sicklyRetryPolicy": Value::Null,
        "throttlePolicy": Value::Null,
        "requestPolicy": {"headerContentType": DEFAULT_CONTENT_TYPE},
        "guaranteed": false,
    })
}

/// A topic delivery policy stating the retry members given, in the shape a TOPIC's
/// document carries.
fn topic_policy(min_delay: i64, max_delay: i64, retries: i64) -> String {
    json!({
        "http": {
            "defaultHealthyRetryPolicy": {
                "minDelayTarget": min_delay,
                "maxDelayTarget": max_delay,
                "numRetries": retries,
            }
        }
    })
    .to_string()
}

/// The `EffectiveDeliveryPolicy` of an HTTP/S subscription is the whole document — the
/// `requestPolicy` section included — with the account defaults, the topic's policy and
/// the subscription's own laid over each other in that order.
#[tokio::test]
async fn the_effective_delivery_policy_is_the_whole_document() {
    // (case, the topic's DeliveryPolicy, the subscription's own, the document expected)
    let rows: Vec<(&str, Option<String>, Option<String>, Value)> = vec![
        (
            "no policy anywhere: the account system defaults",
            None,
            None,
            expected_document(20, 20, 3, "linear"),
        ),
        (
            "the topic's policy laid over the defaults",
            Some(topic_policy(5, 60, 7)),
            None,
            expected_document(5, 60, 7, "linear"),
        ),
        (
            "the subscription's own policy laid over the defaults",
            None,
            Some(r#"{"healthyRetryPolicy":{"numRetries":1,"backoffFunction":"exponential"}}"#.to_string()),
            expected_document(20, 20, 1, "exponential"),
        ),
        (
            "the subscription's policy laid over the topic's",
            Some(topic_policy(5, 60, 7)),
            Some(
                r#"{"healthyRetryPolicy":{"minDelayTarget":1,"maxDelayTarget":10,"numRetries":2}}"#
                    .to_string(),
            ),
            expected_document(1, 10, 2, "linear"),
        ),
    ];

    for protocol in ["http", "https"] {
        for (case, topic_policy, own_policy, expected) in &rows {
            let case = format!("{protocol}: {case}");
            let sns = Sns::new();
            let topic = sns.topic("effective-delivery-policy").await;
            if let Some(policy) = topic_policy {
                let answer = sns
                    .call(
                        "SetTopicAttributes",
                        &[
                            ("TopicArn", topic.as_str()),
                            ("AttributeName", "DeliveryPolicy"),
                            ("AttributeValue", policy.as_str()),
                        ],
                    )
                    .await;
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: the topic's delivery policy must be accepted, got {} / {}",
                    answer.status, answer.body
                );
            }
            let attributes: Vec<(&str, &str)> = match own_policy {
                Some(policy) => vec![("DeliveryPolicy", policy.as_str())],
                None => Vec::new(),
            };
            let endpoint = format!("{protocol}://example.test/hook");
            let arn = subscribe(&sns, &topic, protocol, &endpoint, &attributes).await;

            let answer = read(&sns, &case, &arn).await;
            let entries = attribute_entries(&answer);
            let reported = entries
                .iter()
                .find(|(key, _)| key == "EffectiveDeliveryPolicy")
                .map(|(_, value)| value.clone())
                .unwrap_or_else(|| {
                    panic!("case {case:?}: no EffectiveDeliveryPolicy entry in {entries:?}")
                });
            let document: Value = serde_json::from_str(&reported).unwrap_or_else(|error| {
                panic!("case {case:?}: the policy must be a JSON document ({error}): {reported}")
            });
            assert_eq!(
                document, *expected,
                "case {case:?}: expected the whole effective document, got {document}"
            );

            // A read is a report: the second read answers the same document.
            let again = read(&sns, &case, &arn).await;
            assert_eq!(
                attribute_entries(&again),
                entries,
                "case {case:?}: a second read must answer the same map"
            );
        }
    }
}

/// A subscription whose protocol no delivery policy governs reports no effective policy
/// at all — the control for the entry above.
#[tokio::test]
async fn a_subscription_no_delivery_policy_governs_reports_no_effective_policy() {
    let sns = Sns::new();
    let topic = sns.topic("effective-delivery-policy-control").await;
    let arn = subscribe(
        &sns,
        &topic,
        "sqs",
        "arn:aws:sqs:us-east-1:000000000000:orders",
        &[],
    )
    .await;

    let entries = attribute_entries(&read(&sns, "an sqs subscription", &arn).await);
    assert!(
        !entries
            .iter()
            .any(|(key, _)| key == "EffectiveDeliveryPolicy"),
        "an sqs subscription must report no effective delivery policy, got {entries:?}"
    );
}

/// The entries are reported in SNS's own sequence: every attribute the map carries sits
/// at the position that sequence gives it, and the sequence does not depend on the order
/// the attributes were set in.
#[tokio::test]
async fn the_attribute_entries_are_reported_in_the_services_own_sequence() {
    let sns = Sns::new();
    let topic = sns.topic("subscription-entry-layout").await;
    let arn = subscribe(
        &sns,
        &topic,
        "https",
        "https://example.test/hook",
        &[],
    )
    .await;

    let entries = attribute_entries(&read(&sns, "a pending https subscription", &arn).await);
    let keys: Vec<&str> = entries.iter().map(|(key, _)| key.as_str()).collect();
    assert_eq!(
        keys, REPORTED_SUBSCRIPTION_ATTRIBUTE_ORDER,
        "expected the reported sequence, got {keys:?}"
    );
    // The entry each position carries is the one the sequence names, with its own
    // value: the policy document sits where the sequence puts it rather than beside
    // another attribute's value.
    let at = |name: &str| {
        REPORTED_SUBSCRIPTION_ATTRIBUTE_ORDER
            .iter()
            .position(|known| *known == name)
            .unwrap_or_else(|| panic!("{name} is not in the reported sequence"))
    };
    assert_eq!(
        entries[at("Protocol")].1,
        "https",
        "the protocol entry must carry the protocol, got {entries:?}"
    );
    assert!(
        entries[at("EffectiveDeliveryPolicy")]
            .1
            .starts_with("{\"healthyRetryPolicy\""),
        "the effective-policy entry must carry the policy document, got {entries:?}"
    );
    assert_eq!(
        entries[at("Endpoint")].1,
        "https://example.test/hook",
        "the endpoint entry must carry the endpoint, got {entries:?}"
    );
    assert_eq!(
        entries[at("SubscriptionArn")].1, arn,
        "the ARN entry must carry the subscription's ARN, got {entries:?}"
    );

    // An attribute SNS's sequence does not name follows the ones it does, and the
    // sequence of those does not change for it — whichever order they were written in.
    for (name, value) in [
        ("RedrivePolicy", r#"{"deadLetterTargetArn":"arn:aws:sqs:us-east-1:000000000000:dlq"}"#),
        ("FilterPolicy", r#"{"kind":["alert"]}"#),
    ] {
        let answer = sns
            .call(
                "SetSubscriptionAttributes",
                &[
                    ("SubscriptionArn", arn.as_str()),
                    ("AttributeName", name),
                    ("AttributeValue", value),
                ],
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "setting {name} must be served, got {} / {}",
            answer.status, answer.body
        );
    }

    let entries = attribute_entries(&read(&sns, "with two policies set", &arn).await);
    let keys: Vec<&str> = entries.iter().map(|(key, _)| key.as_str()).collect();
    let (reported, extra) = keys.split_at(REPORTED_SUBSCRIPTION_ATTRIBUTE_ORDER.len());
    assert_eq!(
        reported, REPORTED_SUBSCRIPTION_ATTRIBUTE_ORDER,
        "the sequence must not change for the attributes it does not name, got {keys:?}"
    );
    assert_eq!(
        extra,
        // `FilterPolicyScope` is reported beside the policy it governs, which is why
        // setting the policy adds two entries rather than one.
        ["FilterPolicy", "FilterPolicyScope", "RedrivePolicy"],
        "the attributes the sequence does not name follow it, ordered by name, got {keys:?}"
    );
}
