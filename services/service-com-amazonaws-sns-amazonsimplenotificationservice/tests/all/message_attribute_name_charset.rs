// SPDX-License-Identifier: CC0-1.0

//! The CHARACTER SET a published message attribute's NAME is held to.
//!
//! The RULE asserted here: a message attribute name may carry alphanumeric characters,
//! hyphens, underscores and periods and nothing else (the Amazon SNS Developer Guide,
//! "Amazon SNS message attributes"), so a name holding any other character is REFUSED —
//! `InvalidParameterValueException` (awsQueryError `ParameterValueInvalid`, httpError 400,
//! per `Publish`'s modelled errors) naming the offending character as the character
//! reference SNS states it by — and nothing is published or delivered; while a name made of
//! those characters is published and delivered under the name it was given.
//!
//! The rows differ along what the rule generalises over: which character outside the set the
//! name carries (a space, a control character, punctuation on either side of the digits and
//! letters in code-point order, and one outside ASCII), where in the name it stands, and —
//! on the accepted side — whether the name uses each piece of the permitted punctuation,
//! letters outside ASCII, or digits alone.

use super::sqs_subscription_delivery::{drain, element, emulator, given_queue, sns};
use super::topics_subscriptions_publish::Sns;

/// The error code SNS states a refused parameter VALUE with.
const PARAMETER_VALUE_INVALID: &str = "ParameterValueInvalid";

/// The sentence SNS states for a name carrying `reference`, which is that character's code
/// point in uppercase hex.
fn refusal(reference: &str) -> String {
    format!(
        "Invalid non-alphanumeric character '#x{reference}' was found in the message \
         attribute name. Can only include alphanumeric characters, hyphens, underscores, \
         or dots."
    )
}

/// One refused name: the case, the name, and the character reference the refusal names.
const REFUSED: &[(&str, &str, &str)] = &[
    ("a space inside the name", "a b", "20"),
    ("a tab inside the name", "a\tb", "9"),
    ("a leading exclamation mark", "!ab", "21"),
    ("a hash inside the name", "a#b", "23"),
    ("a slash inside the name", "a/b", "2F"),
    ("a colon inside the name", "a:b", "3A"),
    ("a trailing tilde", "ab~", "7E"),
    (
        "a character outside ASCII that is no letter",
        "a\u{20AC}b",
        "20AC",
    ),
];

/// One accepted name: the case and the name.
const ACCEPTED: &[(&str, &str)] = &[
    ("every piece of the permitted punctuation", "A-b_c.d9"),
    ("letters outside ASCII", "café"),
    ("periods between the parts", "a.b.c"),
    ("digits alone", "90210"),
    ("one letter", "n"),
];

/// A name carrying a character outside the set is refused, with the code, the status and the
/// sentence SNS states it with.
#[tokio::test]
async fn a_name_outside_the_character_set_is_refused() {
    let sns = Sns::new();
    let topic = sns.topic("attribute-name-charset").await;
    for (case, name, reference) in REFUSED {
        let answer = sns
            .call(
                "Publish",
                &[
                    ("TopicArn", &topic),
                    ("Message", "m"),
                    ("MessageAttributes.entry.1.Name", name),
                    ("MessageAttributes.entry.1.Value.DataType", "String"),
                    ("MessageAttributes.entry.1.Value.StringValue", "v"),
                ],
            )
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            PARAMETER_VALUE_INVALID,
            "case {case:?}: expected the {PARAMETER_VALUE_INVALID} code, got {}",
            answer.body
        );
        assert_eq!(
            answer.element("Message").as_deref(),
            Some(refusal(reference).as_str()),
            "case {case:?}: expected the service's own sentence, got {}",
            answer.body
        );
        assert!(
            answer.element("MessageId").is_none(),
            "case {case:?}: a refused publish mints no message id, got {}",
            answer.body
        );
    }
}

/// A name made of the characters the set admits is published, and the notification carries
/// that attribute under the name it was given.
#[tokio::test]
async fn a_name_inside_the_character_set_is_published_and_delivered() {
    let emulator = emulator();
    let topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "attribute-names")]).await,
        "TopicArn",
    );
    let (url, arn) = given_queue(&emulator, "attribute-names-queue").await;
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

    for (case, name) in ACCEPTED {
        sns(
            &emulator,
            "Publish",
            &[
                ("TopicArn", &topic),
                ("Message", case),
                ("MessageAttributes.entry.1.Name", name),
                ("MessageAttributes.entry.1.Value.DataType", "String"),
                ("MessageAttributes.entry.1.Value.StringValue", "v"),
            ],
        )
        .await;
        let delivered = drain(&emulator, &url).await;
        assert_eq!(
            delivered.len(),
            1,
            "case {case:?}: the publish must be delivered, got {delivered:?}"
        );
        let body = &delivered[0].0;
        let envelope: serde_json::Value = serde_json::from_str(body)
            .unwrap_or_else(|error| panic!("case {case:?}: {error} for {body}"));
        assert_eq!(
            envelope["MessageAttributes"][*name]["Value"].as_str(),
            Some("v"),
            "case {case:?}: the attribute must be carried under {name:?}, got {body}"
        );
    }
}
