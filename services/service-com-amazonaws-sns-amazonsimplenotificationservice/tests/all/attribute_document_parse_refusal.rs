// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the SENTENCE an attribute whose value is not JSON at all is
//! refused with.
//!
//! The RULE asserted here: when SNS attributes a refusal to one NAMED attribute's own
//! document — a value the JSON parser could not read — it names that attribute
//! DIRECTLY: `Invalid parameter: <AttributeName>: failed to parse JSON. ` followed by
//! the parser's own diagnostic, with NO `<member> Reason: ` frame in front of the
//! attribute. (The frame belongs to the refusals whose offending parameter is the
//! `Attributes` member itself — a flag's bad value states it, and that sentence is
//! asserted by `validation_rules.rs` and `fifo_pairing_and_subscribe_conflicts.rs`.)
//! The attribute name in the sentence is the one the CALLER spelled, and nothing in the
//! sentence echoes the document back.
//!
//! The rows differ along what the rule generalises over: WHICH attribute carries the
//! document (three of the subscription attributes whose value is a policy document,
//! from `SUBSCRIPTION_ATTRIBUTE_NAMES`), and the FORM of the value that is not JSON (a
//! bare word, a sentence of words, a template left unsubstituted, a document that ends
//! early). Which word the diagnostic names and where it sits is
//! `filter_policy_parse_diagnostic.rs`.
//!
//! Every call goes over the wire through the router the emulator serves in production,
//! so nothing but persisted state carries information between calls.

use service_com_amazonaws_sns_amazonsimplenotificationservice::state::attribute_parse_refusal_opening;

use super::topics_subscriptions_publish::{attribute_map, Answer, Sns};

/// One `SetSubscriptionAttributes` call.
async fn set(sns: &Sns, arn: &str, name: &str, value: &str) -> Answer {
    sns.call(
        "SetSubscriptionAttributes",
        &[
            ("SubscriptionArn", arn),
            ("AttributeName", name),
            ("AttributeValue", value),
        ],
    )
    .await
}

/// An http subscription of its own topic, confirmed by this service, with its ARN.
async fn subscribed(sns: &Sns, topic: &str) -> String {
    let answer = sns
        .call(
            "Subscribe",
            &[
                ("TopicArn", topic),
                ("Protocol", "https"),
                ("Endpoint", "https://example.test/hook"),
                ("ReturnSubscriptionArn", "true"),
            ],
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "Subscribe must be served, got {} / {}",
        answer.status, answer.body
    );
    answer
        .element("SubscriptionArn")
        .unwrap_or_else(|| panic!("no SubscriptionArn in {}", answer.body))
}

/// A value that is not JSON is refused for the PARSE, naming the attribute it was
/// written to and no member in front of it — whichever policy-valued attribute carried
/// it.
#[tokio::test]
async fn a_value_that_is_not_json_is_refused_naming_the_attribute_alone() {
    let sns = Sns::new();
    let topic = sns.topic("attribute-parse-refusal").await;
    let subscription = subscribed(&sns, &topic).await;

    // The subscription attributes whose value is a policy DOCUMENT, as
    // `SetSubscriptionAttributesInput$AttributeName` documents them.
    let attributes = ["FilterPolicy", "DeliveryPolicy", "ReplayPolicy"];
    // (case, a value that is not JSON at all)
    let values: &[(&str, &str)] = &[
        ("one bare word", "not"),
        ("a sentence of bare words", "this is not json"),
        ("a word a template left unsubstituted", r#"{"event": $event}"#),
        ("an object that ends early", "{"),
        ("a bare word where a member's value belongs", r#"{"event": created}"#),
    ];

    for name in attributes {
        let opening = attribute_parse_refusal_opening(name);
        for (case, value) in values {
            let answer = set(&sns, &subscription, name, value).await;
            assert_eq!(
                answer.status, 400,
                "{name} case {case:?}: expected HTTP 400, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                "InvalidParameter",
                "{name} case {case:?}: expected the InvalidParameter code, got {}",
                answer.body
            );
            let message = answer
                .element("Message")
                .unwrap_or_else(|| panic!("{name} case {case:?}: no Message in {}", answer.body));
            assert!(
                message.starts_with(&opening),
                "{name} case {case:?}: expected a refusal opening with {opening:?}, got \
                 {message:?}"
            );
            assert!(
                !message.contains("Reason:"),
                "{name} case {case:?}: the parse refusal must carry no member frame, got \
                 {message:?}"
            );
            assert!(
                message.contains("REDACTED"),
                "{name} case {case:?}: the refusal must report the document's source as \
                 redacted rather than echoing it, got {message:?}"
            );
        }

        // Nothing the parse refused was stored: the attribute is still absent.
        let held = attribute_map(
            &sns.call(
                "GetSubscriptionAttributes",
                &[("SubscriptionArn", subscription.as_str())],
            )
            .await,
        );
        assert_eq!(
            held.get(name),
            None,
            "{name}: a refused document must not be stored, got {held:?}"
        );
    }
}
