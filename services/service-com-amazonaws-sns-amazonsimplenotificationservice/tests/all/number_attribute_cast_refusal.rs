// SPDX-License-Identifier: CC0-1.0

//! What Amazon SNS answers a published `Number` message attribute whose value it cannot
//! read as a number, driven end-to-end over the awsQuery wire.
//!
//! RULE: a `Number` attribute carries a NUMBER, and a value SNS cannot cast to one is
//! refused for the cast it could not make — HTTP 400 under the `ParameterValueInvalid`
//! code the model binds `InvalidParameterValueException` to
//! (`smithy_query(query="operation",operation="Publish")`) — with the sentence
//! `Could not cast message attribute '<name>' value to number.`, whose only variable part
//! is the attribute's NAME, taken from the request. Nothing of the value is echoed back,
//! and nothing is published. A value that IS a number inside the type's documented
//! precision and scale is published as before.
//!
//! The rows differ along what the rule generalises over: the shape of the value SNS
//! cannot cast (plain text, a number with a second decimal point, a sign with no digits,
//! an exponent with no digits, whitespace around a number, a thousands separator, a
//! hexadecimal literal, a non-finite spelling, the empty-ish forms a numeric parser
//! accepts), the NAME the attribute carries (which is what the sentence quotes), the data
//! type's own LABEL, and the publishing call — `Publish` and `PublishBatch` hold the same
//! attribute to the same rule.

use super::topics_subscriptions_publish::{Answer, Sns};

/// The status and awsQuery code SNS answers a message attribute's value it will not take
/// with (`smithy_query(query="operation",operation="Publish")`:
/// `InvalidParameterValueException`, `httpError` 400, `awsQueryError` code
/// `ParameterValueInvalid`).
const REFUSAL_STATUS: u16 = 400;
const REFUSAL_CODE: &str = "ParameterValueInvalid";

/// The sentence SNS refuses an uncastable `Number` attribute `name` with.
fn cast_refusal(name: &str) -> String {
    format!("Could not cast message attribute '{name}' value to number.")
}

/// Publish one message carrying the single attribute `(name, data_type, value)`.
async fn publish(sns: &Sns, topic: &str, name: &str, data_type: &str, value: &str) -> Answer {
    sns.call(
        "Publish",
        &[
            ("TopicArn", topic),
            ("Message", "m"),
            ("MessageAttributes.entry.1.Name", name),
            ("MessageAttributes.entry.1.Value.DataType", data_type),
            ("MessageAttributes.entry.1.Value.StringValue", value),
        ],
    )
    .await
}

/// The same attribute through the other publishing call, which holds each entry to the
/// same attribute rules.
async fn publish_batch(sns: &Sns, topic: &str, name: &str, data_type: &str, value: &str) -> Answer {
    sns.call(
        "PublishBatch",
        &[
            ("TopicArn", topic),
            ("PublishBatchRequestEntries.member.1.Id", "one"),
            ("PublishBatchRequestEntries.member.1.Message", "m"),
            (
                "PublishBatchRequestEntries.member.1.MessageAttributes.entry.1.Name",
                name,
            ),
            (
                "PublishBatchRequestEntries.member.1.MessageAttributes.entry.1.Value.DataType",
                data_type,
            ),
            (
                "PublishBatchRequestEntries.member.1.MessageAttributes.entry.1.Value.StringValue",
                value,
            ),
        ],
    )
    .await
}

/// RULE: a `Number` attribute whose value cannot be cast is refused with SNS's own cast
/// sentence, by both publishing calls, and nothing is published.
#[tokio::test]
async fn an_uncastable_number_attribute_is_refused_for_the_cast_it_could_not_make() {
    let sns = Sns::new();
    let topic = sns.topic("cast-refusals").await;

    // (case, the attribute's name, the data type as the request spells it, the value)
    let rows: &[(&str, &str, &str, &str)] = &[
        ("plain text", "count", "Number", "not-a-number"),
        ("a second decimal point", "spent", "Number", "1.2.3"),
        ("a sign with no digits", "delta", "Number", "-"),
        ("an exponent with no digits", "scale", "Number", "1e"),
        ("whitespace around a number", "padded", "Number", " 17 "),
        ("a thousands separator", "total", "Number", "1,000"),
        ("a hexadecimal literal", "mask", "Number", "0x1f"),
        // Spellings a floating-point parser takes and a decimal number has none of.
        ("a non-finite spelling", "ratio", "Number", "NaN"),
        ("an infinity", "limit", "Number", "inf"),
        ("a trailing unit", "size", "Number", "17kb"),
        // The name is the sentence's only variable part, so a row names the attribute
        // with every character class a name may carry.
        ("a name carrying a period", "order.id", "Number", "abc"),
        ("a name carrying a hyphen", "retry-after", "Number", "abc"),
        ("a name carrying an underscore", "queue_depth", "Number", "abc"),
        // The type may carry a custom label after it, which is still the Number type.
        ("the type with a custom label", "count", "Number.cents", "abc"),
    ];

    for (case, name, data_type, value) in rows {
        let expected = cast_refusal(name);
        for (call, answer) in [
            ("Publish", publish(&sns, &topic, name, data_type, value).await),
            (
                "PublishBatch",
                publish_batch(&sns, &topic, name, data_type, value).await,
            ),
        ] {
            assert_eq!(
                answer.status, REFUSAL_STATUS,
                "case {case:?} through {call}: expected HTTP {REFUSAL_STATUS}, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                REFUSAL_CODE,
                "case {case:?} through {call}: expected the {REFUSAL_CODE} code, got {}",
                answer.body
            );
            assert_eq!(
                answer.element("Message").as_deref(),
                Some(expected.as_str()),
                "case {case:?} through {call}: expected {expected:?}, got {}",
                answer.body
            );
            // The value the caller sent is its own: the sentence names the attribute and
            // does not echo what was in it.
            let message = answer.element("Message").unwrap_or_default();
            assert!(
                !message.contains(value),
                "case {case:?} through {call}: expected the sentence not to echo the value \
                 {value:?}, got {message:?}"
            );
            // Nothing was published: a refusal carries no id of a message.
            assert_eq!(
                answer.element("MessageId"),
                None,
                "case {case:?} through {call}: expected a refused publish to report no \
                 message, got {}",
                answer.body
            );
        }
    }
}

/// The other side of the rule: a value that IS a number of the documented precision and
/// scale is published, so the refusal above is about the cast and nothing else.
#[tokio::test]
async fn a_number_attribute_whose_value_is_a_number_is_published() {
    let sns = Sns::new();
    let topic = sns.topic("cast-accepted").await;

    // (case, the value) — the forms a decimal number is written in: a whole number, a
    // signed one, a fraction, a leading-point fraction and both exponent spellings, plus
    // the documented bounds' own edges ("a number can have up to 38 digits of precision,
    // and it can be between 10^-128 to 10^+126", the Amazon SNS Developer Guide).
    let rows: &[(&str, &str)] = &[
        ("a whole number", "17"),
        ("a signed number", "-17"),
        ("an explicitly positive number", "+17"),
        ("a fraction", "1.5"),
        ("a leading-point fraction", ".5"),
        ("a lowercase exponent", "1e3"),
        ("an uppercase signed exponent", "1E-3"),
        ("the largest power the type holds", "1e126"),
        ("the smallest power the type holds", "1e-128"),
        ("zero", "0"),
    ];

    for (case, value) in rows {
        let answer = publish(&sns, &topic, "count", "Number", value).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected the publish to be served, got {} / {}",
            answer.status, answer.body
        );
        let message_id = answer.element("MessageId").unwrap_or_default();
        assert!(
            !message_id.is_empty(),
            "case {case:?}: expected the publish to report a message, got {}",
            answer.body
        );
    }
}
