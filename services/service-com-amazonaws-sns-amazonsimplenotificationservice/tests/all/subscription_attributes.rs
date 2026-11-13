// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the subscription ATTRIBUTE slice: the map
//! `GetSubscriptionAttributes` projects out of one persisted subscription.
//!
//! Every call goes over the wire through the router the emulator serves in production,
//! so nothing but persisted state carries information between calls.
//!
//! The rule under test: `GetSubscriptionAttributes` reports the properties of the
//! subscription its `SubscriptionArn` names, computed from the persisted record — the
//! identity and routing `Subscribe` minted it with, its confirmation state, the
//! documented default of a flag nobody set, and every attribute its owner set —
//! refusing a value that is not a subscription ARN with HTTP 400 / `InvalidParameter`
//! and a well-formed ARN that names no subscription with HTTP 404 / `NotFound`.
//!
//! The rows differ in protocol, endpoint form, which attributes the subscription
//! carries and whether it is awaiting confirmation, which is what the rule generalises
//! over; a recorded SQS or Lambda subscription is one row of it.

use super::topics_subscriptions_publish::{attribute_entries, attribute_map, Answer, Sns};

/// The account id an ARN this emulator minted carries: the fifth field of
/// `arn:aws:sns:<region>:<account>:<name>`.
fn account_of(arn: &str) -> String {
    arn.split(':').nth(4).unwrap_or_default().to_string()
}

/// The JSON document an attribute value carries, or a panic naming the case.
fn json_of(case: &str, name: &str, value: &str) -> serde_json::Value {
    serde_json::from_str(value).unwrap_or_else(|e| {
        panic!("case {case:?}: {name} must be a JSON document, got {value:?}: {e}")
    })
}

/// Subscribe `endpoint` over `protocol` with `attributes`, asking for the ARN in every
/// case, and answer the subscription ARN the call reports.
///
/// `pub(crate)` so a sibling suite that needs a persisted subscription to act on reuses
/// this one rather than writing a second `Subscribe` caller.
pub(crate) async fn subscribe(
    sns: &Sns,
    case: &str,
    topic: &str,
    protocol: &str,
    endpoint: &str,
    attributes: &[(&str, &str)],
) -> String {
    let mut params: Vec<(&str, &str)> = vec![
        ("TopicArn", topic),
        ("Protocol", protocol),
        ("Endpoint", endpoint),
        ("ReturnSubscriptionArn", "true"),
    ];
    // The awsQuery form of an attribute map: one numbered `entry` per attribute. A
    // Firehose subscription carries the role SNS delivers as unless the caller named one
    // itself — "specifying a valid ARN for this attribute is required for Firehose
    // delivery stream subscriptions" (`SubscribeInput$Attributes` doc).
    let keys = [
        "Attributes.entry.1.key",
        "Attributes.entry.2.key",
        "Attributes.entry.3.key",
    ];
    let values = [
        "Attributes.entry.1.value",
        "Attributes.entry.2.value",
        "Attributes.entry.3.value",
    ];
    let mut attributes: Vec<(&str, &str)> = attributes.to_vec();
    if protocol == "firehose"
        && !attributes
            .iter()
            .any(|(name, _)| *name == "SubscriptionRoleArn")
    {
        attributes.push((
            "SubscriptionRoleArn",
            "arn:aws:iam::000000000000:role/sns-firehose",
        ));
    }
    for (index, (name, value)) in attributes.iter().enumerate() {
        params.push((keys[index], name));
        params.push((values[index], value));
    }
    let answer = sns.call("Subscribe", &params).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: Subscribe expected 200, got {} / {}",
        answer.status, answer.body
    );
    answer
        .element("SubscriptionArn")
        .unwrap_or_else(|| panic!("case {case:?}: no SubscriptionArn in {}", answer.body))
}

/// A read of one subscription's attributes, asserting the call was answered.
///
/// `pub(crate)` for the same reason as [`subscribe`].
pub(crate) async fn read(sns: &Sns, case: &str, arn: &str) -> Answer {
    let answer = sns
        .call("GetSubscriptionAttributes", &[("SubscriptionArn", arn)])
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: GetSubscriptionAttributes expected 200, got {} / {}",
        answer.status, answer.body
    );
    answer
}

/// Set one topic attribute, asserting the call was accepted.
async fn set_topic_attribute(sns: &Sns, case: &str, arn: &str, name: &str, value: &str) {
    let answer = sns
        .call(
            "SetTopicAttributes",
            &[
                ("TopicArn", arn),
                ("AttributeName", name),
                ("AttributeValue", value),
            ],
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: SetTopicAttributes {name} expected 200, got {} / {}",
        answer.status, answer.body
    );
}

/// One row of the projection table: the case, the topic's name, the protocol and the
/// endpoint it addresses, the attributes `Subscribe` carried, whether the subscription
/// is awaiting its subscriber's confirmation, and the attribute keys the read reports.
type ProjectionCase<'a> = (
    &'a str,
    &'a str,
    &'a str,
    &'a str,
    &'a [(&'a str, &'a str)],
    bool,
    &'a [&'a str],
);

/// A filter policy document, as the `FilterPolicy` attribute carries one.
const FILTER_POLICY: &str = r#"{"kind":["alert"]}"#;

/// A delivery policy that overrides one retry member, as a subscription's
/// `DeliveryPolicy` attribute carries one: the retry policy at the top level.
const SUBSCRIPTION_DELIVERY_POLICY: &str = r#"{"healthyRetryPolicy":{"numRetries":7}}"#;

/// A dead-letter queue, as the `RedrivePolicy` attribute names one.
const REDRIVE_POLICY: &str = r#"{"deadLetterTargetArn":"arn:aws:sqs:us-east-1:000000000000:dlq"}"#;

/// The members of a `healthyRetryPolicy`, as the retry policy of a delivery policy
/// document carries them (SNS Developer Guide, "Amazon SNS message delivery retries"):
/// SNS reports the policy WHOLE, however few of them a caller named.
const RETRY_POLICY_MEMBERS: &[&str] = &[
    "backoffFunction",
    "maxDelayTarget",
    "minDelayTarget",
    "numMaxDelayRetries",
    "numMinDelayRetries",
    "numNoDelayRetries",
    "numRetries",
];

/// `GetSubscriptionAttributes` reports ONE entry per property of the subscription its
/// ARN names, each key paired with its own value: the ARN, the topic, the protocol and
/// endpoint it delivers to, the owning account, the confirmation state, the documented
/// default of a flag nobody set, and whatever its owner set — and the same document on
/// every read.
#[tokio::test]
async fn subscription_attributes_are_projected_from_the_persisted_subscription() {
    let sns = Sns::new();
    // The attribute names are the `GetSubscriptionAttributesResponse$Attributes` /
    // `SetSubscriptionAttributesInput$AttributeName` catalog; the keys come out in one
    // stable order, so each row names the order it expects.
    let cases: &[ProjectionCase<'_>] = &[
        (
            "an sqs subscription with nothing set",
            "sub-attrs-sqs-plain",
            "sqs",
            "arn:aws:sqs:us-east-1:000000000000:orders",
            &[],
            false,
            &[
                "ConfirmationWasAuthenticated",
                "Endpoint",
                "Owner",
                "PendingConfirmation",
                "Protocol",
                "RawMessageDelivery",
                "SubscriptionArn",
                "SubscriptionPrincipal",
                "TopicArn",
            ],
        ),
        (
            "an sqs subscription with raw delivery and a filter policy",
            "sub-attrs-sqs-filtered",
            "sqs",
            "arn:aws:sqs:eu-west-1:000000000000:filtered-queue",
            &[("RawMessageDelivery", "true"), ("FilterPolicy", FILTER_POLICY)],
            false,
            &[
                "ConfirmationWasAuthenticated",
                "Endpoint",
                "FilterPolicy",
                // A filter policy is applied under a scope, so the documented default
                // scope is reported with the policy.
                "FilterPolicyScope",
                "Owner",
                "PendingConfirmation",
                "Protocol",
                "RawMessageDelivery",
                "SubscriptionArn",
                "SubscriptionPrincipal",
                "TopicArn",
            ],
        ),
        (
            "a lambda subscription naming a function that does not exist",
            "sub-attrs-lambda-absent",
            "lambda",
            "arn:aws:lambda:us-east-1:000000000000:function:never-created",
            &[],
            false,
            &[
                "ConfirmationWasAuthenticated",
                "Endpoint",
                "Owner",
                "PendingConfirmation",
                "Protocol",
                "RawMessageDelivery",
                "SubscriptionArn",
                "SubscriptionPrincipal",
                "TopicArn",
            ],
        ),
        (
            "an http subscription awaiting confirmation",
            "sub-attrs-http-pending",
            "http",
            "http://example.test/hook",
            &[],
            true,
            &[
                "ConfirmationWasAuthenticated",
                // A delivery policy retries a failed delivery to an HTTP/S endpoint, so
                // such a subscription reports the one it is run under: the account
                // system defaults with the topic's and its own laid over them.
                "EffectiveDeliveryPolicy",
                "Endpoint",
                "Owner",
                "PendingConfirmation",
                "Protocol",
                "RawMessageDelivery",
                "SubscriptionArn",
                "SubscriptionPrincipal",
                "TopicArn",
            ],
        ),
        (
            "an https subscription with its own delivery policy",
            "sub-attrs-https-policy",
            "https",
            "https://deep.example.test/a/b/c",
            &[("DeliveryPolicy", SUBSCRIPTION_DELIVERY_POLICY)],
            true,
            &[
                "ConfirmationWasAuthenticated",
                "DeliveryPolicy",
                "EffectiveDeliveryPolicy",
                "Endpoint",
                "Owner",
                "PendingConfirmation",
                "Protocol",
                "RawMessageDelivery",
                "SubscriptionArn",
                "SubscriptionPrincipal",
                "TopicArn",
            ],
        ),
        (
            "an sqs subscription with its own delivery policy",
            "sub-attrs-sqs-policy",
            "sqs",
            "arn:aws:sqs:us-east-1:000000000000:retried",
            &[("DeliveryPolicy", SUBSCRIPTION_DELIVERY_POLICY)],
            false,
            &[
                "ConfirmationWasAuthenticated",
                // The policy its owner SET is reported because its owner set it; the
                // effective one is not, because no delivery to a queue is retried under
                // a delivery policy.
                "DeliveryPolicy",
                "Endpoint",
                "Owner",
                "PendingConfirmation",
                "Protocol",
                "RawMessageDelivery",
                "SubscriptionArn",
                "SubscriptionPrincipal",
                "TopicArn",
            ],
        ),
        (
            "an email subscription awaiting confirmation",
            "sub-attrs-email-pending",
            "email",
            "first.last+tag@mail.example.co.uk",
            &[],
            true,
            &[
                "ConfirmationWasAuthenticated",
                "Endpoint",
                "Owner",
                "PendingConfirmation",
                "Protocol",
                "RawMessageDelivery",
                "SubscriptionArn",
                "SubscriptionPrincipal",
                "TopicArn",
            ],
        ),
        (
            "a firehose subscription with the role it writes under and raw delivery",
            "sub-attrs-firehose-role",
            "firehose",
            "arn:aws:firehose:us-east-1:000000000000:deliverystream/events",
            &[
                (
                    "SubscriptionRoleArn",
                    "arn:aws:iam::000000000000:role/sns-firehose",
                ),
                ("RawMessageDelivery", "true"),
            ],
            false,
            &[
                "ConfirmationWasAuthenticated",
                "Endpoint",
                "Owner",
                "PendingConfirmation",
                "Protocol",
                "RawMessageDelivery",
                "SubscriptionArn",
                "SubscriptionPrincipal",
                "SubscriptionRoleArn",
                "TopicArn",
            ],
        ),
        (
            "an sms subscription with a dead-letter queue",
            "sub-attrs-sms-redrive",
            "sms",
            "+44 20 7946 0958",
            &[("RedrivePolicy", REDRIVE_POLICY)],
            false,
            &[
                "ConfirmationWasAuthenticated",
                "Endpoint",
                "Owner",
                "PendingConfirmation",
                "Protocol",
                "RawMessageDelivery",
                "RedrivePolicy",
                "SubscriptionArn",
                "SubscriptionPrincipal",
                "TopicArn",
            ],
        ),
    ];

    for (case, topic_name, protocol, endpoint, configured, pending, expected_keys) in cases {
        let topic = sns.topic(topic_name).await;
        let arn = subscribe(&sns, case, &topic, protocol, endpoint, configured).await;

        let answer = read(&sns, case, &arn).await;
        let entries = attribute_entries(&answer);
        // WHICH attributes the map carries, named here; the SEQUENCE the entries are
        // reported in is one rule for every subscription and is asserted by
        // `effective_delivery_policy_entry_layout.rs`, so the rows below list the names
        // alphabetically and compare the set.
        let mut keys: Vec<&str> = entries.iter().map(|(key, _)| key.as_str()).collect();
        keys.sort_unstable();
        assert_eq!(
            keys, *expected_keys,
            "case {case:?}: expected the attribute keys {expected_keys:?}, got {keys:?}"
        );

        // Each key carries its OWN value: the identity and routing the subscription was
        // minted with, read back out of the persisted record.
        let held = attribute_map(&answer);
        let principal = format!("arn:aws:iam::{}:user/emulator", account_of(&arn));
        for (name, expected) in [
            ("SubscriptionArn", arn.as_str()),
            ("TopicArn", topic.as_str()),
            ("Protocol", *protocol),
            ("Endpoint", *endpoint),
            ("Owner", account_of(&arn).as_str()),
            // The principal that created the subscription: the identity this emulator's
            // credentials present, which is what `sts:GetCallerIdentity` reports too.
            ("SubscriptionPrincipal", principal.as_str()),
        ] {
            assert_eq!(
                held.get(name).map(String::as_str),
                Some(expected),
                "case {case:?}: {name} must be {expected:?}, got {held:?}"
            );
        }

        // The confirmation state is the subscription's, not the protocol name's: a
        // subscription nothing has confirmed is pending and was not authenticated.
        assert_eq!(
            held.get("PendingConfirmation").map(String::as_str),
            Some(if *pending { "true" } else { "false" }),
            "case {case:?}: PendingConfirmation must say whether the subscriber has \
             confirmed, got {held:?}"
        );
        assert_eq!(
            held.get("ConfirmationWasAuthenticated").map(String::as_str),
            Some(if *pending { "false" } else { "true" }),
            "case {case:?}: ConfirmationWasAuthenticated must say whether a confirmation \
             has happened, got {held:?}"
        );

        // A flag nobody set is reported at its documented default.
        let raw_expected = configured
            .iter()
            .find(|(name, _)| *name == "RawMessageDelivery")
            .map(|(_, value)| *value)
            .unwrap_or("false");
        assert_eq!(
            held.get("RawMessageDelivery").map(String::as_str),
            Some(raw_expected),
            "case {case:?}: RawMessageDelivery must be {raw_expected:?}, got {held:?}"
        );

        // Every attribute the caller set round-trips in the form SNS reports it: a
        // document it normalizes reads back with the members it filled in, and
        // everything else verbatim.
        for (name, value) in *configured {
            let reported = held.get(*name).cloned().unwrap_or_default();
            if *name == "DeliveryPolicy" {
                let document = json_of(case, name, &reported);
                assert_eq!(
                    document.pointer("/healthyRetryPolicy/numRetries"),
                    json_of(case, name, value).pointer("/healthyRetryPolicy/numRetries"),
                    "case {case:?}: {name} must keep the retry member the subscription \
                     was given, got {document}"
                );
                for member in RETRY_POLICY_MEMBERS {
                    assert!(
                        document
                            .pointer(&format!("/healthyRetryPolicy/{member}"))
                            .is_some(),
                        "case {case:?}: {name} must read back with {member} filled in, \
                         got {document}"
                    );
                }
                continue;
            }
            assert_eq!(
                reported.as_str(),
                *value,
                "case {case:?}: {name} must report the value the subscription was given, \
                 got {held:?}"
            );
        }
        if configured.iter().any(|(name, _)| *name == "FilterPolicy") {
            assert_eq!(
                held.get("FilterPolicyScope").map(String::as_str),
                Some("MessageAttributes"),
                "case {case:?}: a filter policy with no explicit scope is applied under \
                 the documented default scope, got {held:?}"
            );
        }
        if let Some(effective) = held.get("EffectiveDeliveryPolicy") {
            let document = json_of(case, "EffectiveDeliveryPolicy", effective);
            for member in RETRY_POLICY_MEMBERS {
                assert!(
                    document
                        .pointer(&format!("/healthyRetryPolicy/{member}"))
                        .is_some(),
                    "case {case:?}: EffectiveDeliveryPolicy must state the whole retry \
                     policy; {member} is missing from {document}"
                );
            }
        }

        // A read must not change what a later read sees.
        let again = read(&sns, case, &arn).await;
        assert_eq!(
            attribute_entries(&again),
            entries,
            "case {case:?}: two reads of one subscription must report the same entries \
             in the same order"
        );
    }
}

/// One effective-policy row: the case, the `DeliveryPolicy` the topic holds, the one
/// the subscription holds, and the `numRetries` a delivery is actually run under.
type EffectivePolicyCase<'a> = (&'a str, Option<&'a str>, Option<&'a str>, i64);

/// The `EffectiveDeliveryPolicy` of a subscription is SNS's account system defaults
/// with the TOPIC's delivery policy and then the SUBSCRIPTION's own laid over them —
/// unless the topic's policy disables subscription overrides, which is what that member
/// means. It is reported in the subscription's own document shape, with the retry policy
/// at the top level.
///
/// The system default `numRetries` of 3 is SNS's documented default for a topic no
/// caller has given a delivery policy (SNS Developer Guide, "Amazon SNS message
/// delivery retries"), the same constant `GetTopicAttributes` reports.
#[tokio::test]
async fn the_effective_delivery_policy_composes_the_topic_and_subscription_policies() {
    let sns = Sns::new();
    let cases: &[EffectivePolicyCase<'_>] = &[
        ("neither sets one", None, None, 3),
        (
            "only the topic sets one",
            Some(r#"{"http":{"defaultHealthyRetryPolicy":{"numRetries":7}}}"#),
            None,
            7,
        ),
        (
            "only the subscription sets one",
            None,
            Some(r#"{"healthyRetryPolicy":{"numRetries":9}}"#),
            9,
        ),
        (
            "the subscription overrides the topic",
            Some(r#"{"http":{"defaultHealthyRetryPolicy":{"numRetries":7}}}"#),
            Some(r#"{"healthyRetryPolicy":{"numRetries":9}}"#),
            9,
        ),
        (
            "the topic disables subscription overrides",
            Some(
                r#"{"http":{"defaultHealthyRetryPolicy":{"numRetries":7},
                   "disableSubscriptionOverrides":true}}"#,
            ),
            Some(r#"{"healthyRetryPolicy":{"numRetries":9}}"#),
            7,
        ),
    ];

    for (index, (case, topic_policy, subscription_policy, expected)) in cases.iter().enumerate() {
        let topic = sns.topic(&format!("effective-policy-{index:02}")).await;
        if let Some(policy) = topic_policy {
            set_topic_attribute(&sns, case, &topic, "DeliveryPolicy", policy).await;
        }
        let configured: Vec<(&str, &str)> = subscription_policy
            .map(|policy| vec![("DeliveryPolicy", policy)])
            .unwrap_or_default();
        let arn = subscribe(
            &sns,
            case,
            &topic,
            "https",
            "https://example.test/hook",
            &configured,
        )
        .await;

        let answer = read(&sns, case, &arn).await;
        let held = attribute_map(&answer);
        let effective = held
            .get("EffectiveDeliveryPolicy")
            .cloned()
            .unwrap_or_default();
        let document = json_of(case, "EffectiveDeliveryPolicy", &effective);
        let retries = document
            .pointer("/healthyRetryPolicy/numRetries")
            .and_then(serde_json::Value::as_i64);
        assert_eq!(
            retries,
            Some(*expected),
            "case {case:?}: expected numRetries {expected}, got {document}"
        );
    }
}

/// `GetSubscriptionAttributes` answers 404 / `NotFound` for a well-formed subscription
/// ARN that names no subscription this emulator holds — one never minted, one
/// unsubscribed, one whose topic was deleted — and 400 / `InvalidParameter` for a value
/// that is not a subscription ARN at all, including the sentinels SNS publishes in
/// place of an unconfirmed subscription's ARN.
#[tokio::test]
async fn getting_the_attributes_of_a_subscription_that_is_not_there_is_refused() {
    let sns = Sns::new();
    let topic = sns.topic("absent-subscription").await;
    let unsubscribed = subscribe(
        &sns,
        "unsubscribed",
        &topic,
        "sqs",
        "arn:aws:sqs:us-east-1:000000000000:gone",
        &[],
    )
    .await;
    let unsubscribe = sns
        .call("Unsubscribe", &[("SubscriptionArn", &unsubscribed)])
        .await;
    assert_eq!(
        unsubscribe.status, 200,
        "Unsubscribe must succeed, got {} / {}",
        unsubscribe.status, unsubscribe.body
    );

    let doomed_topic = sns.topic("deleted-with-its-subscriptions").await;
    let doomed = subscribe(
        &sns,
        "deleted with its topic",
        &doomed_topic,
        "sqs",
        "arn:aws:sqs:us-east-1:000000000000:doomed",
        &[],
    )
    .await;
    let deleted = sns
        .call("DeleteTopic", &[("TopicArn", &doomed_topic)])
        .await;
    assert_eq!(
        deleted.status, 200,
        "DeleteTopic must succeed, got {} / {}",
        deleted.status, deleted.body
    );

    let absent = [
        ("the subscription was unsubscribed", unsubscribed.clone()),
        ("the topic was deleted under it", doomed.clone()),
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
        let answer = sns
            .call("GetSubscriptionAttributes", &[("SubscriptionArn", arn)])
            .await;
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

    // And the values that are not subscription ARNs this endpoint serves: too few
    // elements, a topic ARN (one element short of a subscription's), an empty element,
    // another service, a subscription of another region (which this endpoint does not
    // reach), and the two sentinels a `Subscribe` response or a listing carries for a
    // pending subscription.
    let malformed = [
        "not-an-arn-at-all",
        "arn:aws:sns",
        "arn:aws:sns:us-east-1:000000000000:absent-subscription",
        "arn:aws:sns:us-east-1:000000000000:absent-subscription:",
        "arn:aws:sns::000000000000:absent-subscription:11111111",
        "arn:aws:sqs:us-east-1:000000000000:q:11111111",
        "arn:aws:sns:us-east-1:000000000000:two words:11111111",
        "arn:aws:sns:us-east-1:000000000000:topic:id:extra",
        "arn:aws:sns:eu-west-1:123456789012:other-topic:22222222-2222-4222-8222-222222222222",
        "arn:aws-us-gov:sns:us-gov-west-1:000000000000:gov-topic:\
         33333333-3333-4333-8333-333333333333",
        "pending confirmation",
        "PendingConfirmation",
    ];
    for arn in malformed {
        let answer = sns
            .call("GetSubscriptionAttributes", &[("SubscriptionArn", arn)])
            .await;
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
