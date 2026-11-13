// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the SENTENCE a subscription filter policy that is not JSON is
//! refused with.
//!
//! The RULE asserted here: a `FilterPolicy` is "the simple JSON object that lets your
//! subscriber receive only a subset of messages"
//! (`SetSubscriptionAttributesInput$AttributeName`), and a document the service could not
//! parse is refused with `InvalidParameter` (HTTP 400) naming the attribute, the parse that
//! failed AND the parser's diagnostic: the bare word the parse stopped on and the line and
//! column that word begins at, both read off the document the request carried. Everything
//! else in the sentence is fixed text for this condition, and the document itself is never
//! echoed back.
//!
//! The rows differ along what the rule generalises over: which word the document stops the
//! parse on, where in the document it sits (the first value, a member's value, a later line),
//! and which operation carried the write — `Subscribe` and `SetSubscriptionAttributes` refuse
//! the same document with the same sentence.
//!
//! Every call goes over the wire through the router the emulator serves in production, so
//! nothing but persisted state carries information between calls.

use super::topics_subscriptions_publish::{attribute_map, Answer, Sns};

/// The clause every refusal of an unparseable filter policy opens with: the attribute
/// the document was written to, named DIRECTLY — the fault is in that document, so SNS
/// states no `<member> Reason: ` frame in front of it, whichever operation carried the
/// write.
const LEADING_CLAUSE: &str = "Invalid parameter: FilterPolicy: failed to parse JSON.";

/// The WHOLE sentence a document the parse stopped on `token` at `line`:`column` is refused
/// with.
fn expected_message(token: &str, line: usize, column: usize) -> String {
    format!(
        "{LEADING_CLAUSE} Unrecognized token '{token}': was expecting (JSON String, Number, \
         Array, Object or token 'null', 'true' or 'false')\n at [Source: REDACTED \
         (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: {line}, column: \
         {column}]"
    )
}

/// Subscribe an SQS endpoint to `topic` with the attributes given, answering the call.
///
/// `endpoint` is the queue's name: one topic holds at most one subscription per
/// (protocol, endpoint) pair, so a row that asks for a configuration of its own
/// subscribes a queue of its own.
async fn subscribe_queue(
    sns: &Sns,
    topic: &str,
    endpoint: &str,
    attributes: &[(&str, &str)],
) -> Answer {
    let mut params: Vec<(String, String)> = vec![
        ("TopicArn".to_string(), topic.to_string()),
        ("Protocol".to_string(), "sqs".to_string()),
        (
            "Endpoint".to_string(),
            format!("arn:aws:sqs:us-east-1:000000000000:{endpoint}"),
        ),
        ("ReturnSubscriptionArn".to_string(), "true".to_string()),
    ];
    for (index, (key, value)) in attributes.iter().enumerate() {
        params.push((
            format!("Attributes.entry.{}.key", index + 1),
            (*key).to_string(),
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

/// [`subscribe_queue`] on the suite's own queue, for the rows that need no second one.
async fn subscribe_with(sns: &Sns, topic: &str, attributes: &[(&str, &str)]) -> Answer {
    subscribe_queue(sns, topic, "filter-policy-queue", attributes).await
}

/// The message of a refused answer, with the case named when it is not a refusal at all.
fn refusal_message(case: &str, answer: &Answer) -> String {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {} / {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.code(),
        "InvalidParameter",
        "case {case:?}: expected the code InvalidParameter, got {} / {}",
        answer.code(),
        answer.body
    );
    answer
        .element("Message")
        .unwrap_or_else(|| panic!("case {case:?}: no Message in {}", answer.body))
}

/// One row of the diagnostic table: the case, the document sent, and the word the parse
/// stops on with the line and the column it begins at.
type DiagnosticCase<'a> = (&'a str, &'a str, &'a str, usize, usize);

/// A filter policy the service cannot parse is refused with the parser's diagnostic, by
/// whichever operation carried it.
#[tokio::test]async fn an_unparseable_filter_policy_names_the_token_and_where_it_is() {
    let sns = Sns::new();
    let topic = sns.topic("filter-policy-diagnostic").await;
    // One confirmed subscription to write the same documents to through the other path.
    let control = subscribe_with(
        &sns,
        &topic,
        &[("FilterPolicy", r#"{"event":["created"]}"#)],
    )
    .await;
    assert_eq!(
        control.status, 200,
        "a well-formed filter policy must be accepted, got {} / {}",
        control.status, control.body
    );
    let subscription = control
        .element("SubscriptionArn")
        .unwrap_or_else(|| panic!("no SubscriptionArn in {}", control.body));

    // (case, the document sent, the word the parse stops on, its line, its column)
    let rows: &[DiagnosticCase<'_>] = &[
        (
            "a bare word with punctuation after it",
            "this-is-not-json",
            "this",
            1,
            1,
        ),
        ("a sentence of bare words", "this is not json", "this", 1, 1),
        ("one bare word", "nope", "nope", 1, 1),
        (
            "a bare word where a member's value belongs",
            r#"{"event": created}"#,
            "created",
            1,
            11,
        ),
        (
            "a bare word on the second line",
            "{\n  \"event\": oops\n}",
            "oops",
            2,
            12,
        ),
        (
            "a word a template left unsubstituted",
            r#"{"event": $event}"#,
            "$event",
            1,
            11,
        ),
        (
            "a truncated keyword",
            "{\"event\": tru}",
            "tru",
            1,
            11,
        ),
    ];

    for (case, document, token, line, column) in rows {
        let expected = expected_message(token, *line, *column);

        let subscribed = subscribe_with(&sns, &topic, &[("FilterPolicy", document)]).await;
        let message = refusal_message(case, &subscribed);
        assert_eq!(
            message, expected,
            "case {case:?}: Subscribe must refuse the document with the parser's diagnostic: \
             expected {expected:?}, got {message:?}"
        );

        let written = sns
            .call(
                "SetSubscriptionAttributes",
                &[
                    ("SubscriptionArn", subscription.as_str()),
                    ("AttributeName", "FilterPolicy"),
                    ("AttributeValue", document),
                ],
            )
            .await;
        let message = refusal_message(case, &written);
        assert_eq!(
            message, expected,
            "case {case:?}: SetSubscriptionAttributes must refuse the same document with the \
             same sentence: expected {expected:?}, got {message:?}"
        );
        assert!(
            message.contains("REDACTED"),
            "case {case:?}: the refusal must report the document's source as redacted rather \
             than echoing it, got {message:?}"
        );
    }
}

/// A document the parse stops on no word at all is still refused for the parse, and a
/// well-formed object is still accepted.
#[tokio::test]
async fn a_filter_policy_with_no_offending_word_is_refused_for_the_parse_alone() {
    let sns = Sns::new();
    let topic = sns.topic("filter-policy-no-token").await;

    // (case, the document sent, whether it is refused for the parse)
    let rows: &[(&str, &str, bool)] = &[
        ("an object that ends early", "{", true),
        ("an array that ends early", "[\"created\"", true),
        ("a well-formed object", r#"{"event":["created"]}"#, false),
    ];

    for (case, document, refused) in rows {
        let answer = subscribe_with(&sns, &topic, &[("FilterPolicy", document)]).await;
        if !refused {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: a filter policy the service can parse must be accepted, got {} / \
                 {}",
                answer.status, answer.body
            );
            continue;
        }
        let message = refusal_message(case, &answer);
        assert!(
            message.starts_with(LEADING_CLAUSE),
            "case {case:?}: expected a refusal opening with {LEADING_CLAUSE:?}, got {message:?}"
        );
    }

    // An EMPTY value is no document at all: it asks for a subscription that holds no such
    // policy. The subscription above HOLDS the well-formed policy, so asking the same
    // endpoint for none of it is a different attribute set, and the re-subscribe is
    // refused rather than failing to parse…
    let refused = subscribe_with(&sns, &topic, &[("FilterPolicy", "")]).await;
    assert_eq!(
        refused.status, 400,
        "a re-subscribe asking for none of the policy the subscription holds must be \
         refused, got {} / {}",
        refused.status, refused.body
    );
    // …while on a subscription of its own the empty value is accepted, and that
    // subscription holds no filter policy at all.
    let fresh = sns.topic("filter-policy-empty").await;
    let answer = subscribe_with(&sns, &fresh, &[("FilterPolicy", "")]).await;
    assert_eq!(
        answer.status, 200,
        "an empty filter policy removes the policy rather than failing to parse, got {} / {}",
        answer.status, answer.body
    );
    let arn = answer
        .element("SubscriptionArn")
        .unwrap_or_else(|| panic!("no SubscriptionArn in {}", answer.body));
    let held = attribute_map(
        &sns.call("GetSubscriptionAttributes", &[("SubscriptionArn", &arn)])
            .await,
    );
    assert_eq!(
        held.get("FilterPolicy"),
        None,
        "a removed filter policy must not be reported, got {held:?}"
    );
}
