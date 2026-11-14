// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for three rules a subscription and a published message are held to,
//! driven over the wire through the router the emulator serves in production.
//!
//! 1. A subscription's `DeliveryPolicy` is judged as a DOCUMENT and nothing else: the
//!    model lists it, and the `EffectiveDeliveryPolicy` computed from it, among the
//!    attributes a subscription's map carries with no protocol qualifying either
//!    (`GetSubscriptionAttributesResponse$Attributes` doc), so a subscription of any
//!    delivery protocol accepts one — and it reads back NORMALIZED: the retry members the
//!    caller did not name are filled from the account system defaults.
//! 2. A FLAG-valued attribute carries a boolean as a word, so `RawMessageDelivery` is
//!    read in any case and reported in the canonical lower case; and it may be ENABLED on
//!    a subscription whose endpoint takes the bare message bodies — a queue, an HTTP/S
//!    endpoint or a Firehose delivery stream — and on no other.
//! 3. A message published with `MessageStructure` of `json` carries one body per delivery
//!    protocol: "You can define other top-level keys that define the message you want to
//!    send to a specific transport protocol" (`PublishInput$MessageStructure` doc), so the
//!    member named for the subscriber's protocol is the body it receives — delivered as
//!    that value's own JSON text, so a branch naming a number arrives as its decimal text
//!    and one naming a boolean as `true` or `false`. A branch that names no message at all
//!    — an object, a list or `null` — is ignored ("Non-string values will cause the key to
//!    be ignored", `PublishInput$Message` doc), leaving that protocol the `default` body.
//!    The `default` member is not a branch but the message every protocol no branch
//!    names receives, read as the text of its value; a document naming no default body is
//!    refused. A structure value naming nothing SNS reads leaves the body one message for
//!    every protocol.
//!
//! The rows differ along what each rule generalises over: the delivery protocol, the
//! members a policy document names, the case a flag arrives in, and the JSON type of a
//! protocol branch. The recorded cases — a delivery policy on an `sqs` subscription, a
//! `RawMessageDelivery` of `True`, raw delivery on a `firehose` subscription, and a
//! structured message with a number-valued and an object-valued branch — are one row
//! each.

use serde_json::{json, Value};

use service_com_amazonaws_sns_amazonsimplenotificationservice::state::{
    NO_DEFAULT_ENTRY_REFUSAL, RAW_MESSAGE_DELIVERY_PROTOCOLS,
};

use super::sqs_subscription_delivery::{drain, element, emulator, given_queue, sns as sns_call};
use super::subscription_attributes::{read, subscribe};
use super::topics_subscriptions_publish::{attribute_map, Sns};

/// The members of a `healthyRetryPolicy` (SNS Developer Guide, "Amazon SNS message
/// delivery retries"), each paired with the account system default SNS fills it from.
const RETRY_DEFAULTS: &[(&str, i64)] = &[
    ("minDelayTarget", 20),
    ("maxDelayTarget", 20),
    ("numRetries", 3),
    ("numMaxDelayRetries", 0),
    ("numNoDelayRetries", 0),
    ("numMinDelayRetries", 0),
];

/// The default `backoffFunction`, which is a word rather than a number.
const DEFAULT_BACKOFF_FUNCTION: &str = "linear";

/// Every delivery protocol `Subscribe` accepts, with an endpoint of the documented form
/// for each (`SubscribeInput$Protocol` / `SubscribeInput$Endpoint` doc) — the catalog the
/// protocol rows below are drawn from.
const PROTOCOL_ENDPOINTS: &[(&str, &str)] = &[
    ("sqs", "arn:aws:sqs:us-east-1:000000000000:orders"),
    ("http", "http://example.test/hook"),
    ("https", "https://example.test/hook"),
    ("firehose", "arn:aws:firehose:us-east-1:000000000000:deliverystream/events"),
    ("lambda", "arn:aws:lambda:us-east-1:000000000000:function:notify"),
    ("email", "owner@example.test"),
    ("email-json", "owner@example.test"),
    ("sms", "+44 20 7946 0958"),
    ("application", "arn:aws:sns:us-east-1:000000000000:endpoint/GCM/app/device-1"),
];

/// The endpoint form of `protocol`, from the catalog above.
fn endpoint_of(protocol: &str) -> &'static str {
    PROTOCOL_ENDPOINTS
        .iter()
        .find(|(name, _)| *name == protocol)
        .map(|(_, endpoint)| *endpoint)
        .unwrap_or_else(|| panic!("no documented endpoint form for protocol {protocol:?}"))
}

/// Write one subscription attribute.
async fn set(sns: &Sns, arn: &str, name: &str, value: &str) -> (u16, String, String) {
    let answer = sns
        .call(
            "SetSubscriptionAttributes",
            &[
                ("SubscriptionArn", arn),
                ("AttributeName", name),
                ("AttributeValue", value),
            ],
        )
        .await;
    (
        answer.status,
        answer.code(),
        answer.element("Message").unwrap_or_default(),
    )
}

/// The JSON document `value` carries, or a panic naming the case.
fn document(case: &str, name: &str, value: &str) -> Value {
    serde_json::from_str(value).unwrap_or_else(|error| {
        panic!("case {case:?}: {name} must be a JSON document, got {value:?}: {error}")
    })
}

/// Assert that `reported` states the whole retry policy with the account system defaults
/// filled in: every member the caller named with the value it named, and every other at the
/// default. That is the EFFECTIVE policy's rule — the one a delivery is actually run under.
fn assert_normalized(case: &str, name: &str, configured: &Value, reported: &Value) {
    for (member, default) in RETRY_DEFAULTS {
        let pointer = format!("/healthyRetryPolicy/{member}");
        let expected = configured
            .pointer(&pointer)
            .and_then(Value::as_i64)
            .unwrap_or(*default);
        let actual = reported.pointer(&pointer).and_then(Value::as_i64);
        assert_eq!(
            actual,
            Some(expected),
            "case {case:?}: {name} must report {member} as {expected}, got {reported}"
        );
    }
    let pointer = format!("/healthyRetryPolicy/{DEFAULT_BACKOFF_FUNCTION_MEMBER}");
    let expected = configured
        .pointer(&pointer)
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_BACKOFF_FUNCTION);
    let actual = reported.pointer(&pointer).and_then(Value::as_str);
    assert_eq!(
        actual,
        Some(expected),
        "case {case:?}: {name} must report the backoff function as {expected:?}, got \
         {reported}"
    );
}

/// Assert that `reported` is the document as STORED: every member of the shape present,
/// each one carrying what the caller supplied for it and `null` where it supplied none.
///
/// The stored document is what the SUBSCRIPTION holds, so it states no account default: a
/// member nobody set has no value on the subscription.
fn assert_stored(case: &str, name: &str, configured: &Value, reported: &Value) {
    let members: Vec<&str> = RETRY_DEFAULTS
        .iter()
        .map(|(member, _)| *member)
        .chain(std::iter::once(DEFAULT_BACKOFF_FUNCTION_MEMBER))
        .collect();
    if configured.get("healthyRetryPolicy").is_none() {
        assert_eq!(
            reported.get("healthyRetryPolicy"),
            Some(&Value::Null),
            "case {case:?}: {name} must state a retry section the caller did not supply as \
             null, got {reported}"
        );
        return;
    }
    for member in members {
        let pointer = format!("/healthyRetryPolicy/{member}");
        let expected = configured.pointer(&pointer).unwrap_or(&Value::Null);
        let actual = reported.pointer(&pointer);
        assert_eq!(
            actual,
            Some(expected),
            "case {case:?}: {name} must state {member} as {expected}, got {reported}"
        );
    }
}

/// The member of a retry policy that names its backoff function.
const DEFAULT_BACKOFF_FUNCTION_MEMBER: &str = "backoffFunction";

/// A `DeliveryPolicy` is accepted on a subscription of ANY delivery protocol, written by
/// either call that writes a subscription attribute, and reads back as the whole document
/// with the members nobody supplied stated as null — while the `EffectiveDeliveryPolicy`
/// computed from it states the account system defaults in their place.
#[tokio::test]
async fn a_subscription_delivery_policy_is_accepted_on_every_protocol_and_reads_back_whole() {
    let sns = Sns::new();
    // (case, the policy the caller writes)
    let policies: &[(&str, &str)] = &[
        ("one retry member", r#"{"healthyRetryPolicy":{"numRetries":3}}"#),
        (
            "every retry member",
            r#"{"healthyRetryPolicy":{"minDelayTarget":5,"maxDelayTarget":600,
               "numRetries":10,"numMaxDelayRetries":2,"numNoDelayRetries":1,
               "numMinDelayRetries":3,"backoffFunction":"exponential"}}"#,
        ),
        (
            "a backoff function alone",
            r#"{"healthyRetryPolicy":{"backoffFunction":"geometric"}}"#,
        ),
        (
            "no retry member at all",
            r#"{"throttlePolicy":{"maxReceivesPerSecond":2}}"#,
        ),
        ("an empty document", "{}"),
    ];

    let mut index = 0;
    for (protocol, endpoint) in PROTOCOL_ENDPOINTS {
        for (policy_case, policy) in policies {
            let case = format!("{policy_case} on a {protocol} subscription");
            index += 1;
            let topic = sns.topic(&format!("sub-delivery-policy-{index:02}")).await;

            // The write path the recorded check drove: SetSubscriptionAttributes.
            let arn = subscribe(&sns, &case, &topic, protocol, endpoint, &[]).await;
            let (status, code, message) = set(&sns, &arn, "DeliveryPolicy", policy).await;
            assert_eq!(
                status, 200,
                "case {case:?}: SetSubscriptionAttributes expected 200, got {status} / \
                 {code} {message}"
            );

            let held = attribute_map(&read(&sns, &case, &arn).await);
            let configured = document(&case, "the policy written", policy);
            let stored = held
                .get("DeliveryPolicy")
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: no DeliveryPolicy in {held:?}"));
            let stored = document(&case, "DeliveryPolicy", &stored);
            assert_stored(&case, "DeliveryPolicy", &configured, &stored);
            // A member the document names that is not a retry policy is kept as written.
            if let Some(throttle) = configured.get("throttlePolicy") {
                assert_eq!(
                    stored.get("throttlePolicy"),
                    Some(throttle),
                    "case {case:?}: DeliveryPolicy must keep the members it was given, \
                     got {stored}"
                );
            }
            // The EFFECTIVE policy is the one a delivery is run under, so it is reported
            // for the endpoints a delivery policy retries a failed delivery to — the
            // HTTP/S ones — and for no other protocol, whichever policy its owner set.
            let effective = held.get("EffectiveDeliveryPolicy").cloned();
            if ["http", "https"].contains(protocol) {
                let effective = effective.unwrap_or_else(|| {
                    panic!("case {case:?}: no EffectiveDeliveryPolicy in {held:?}")
                });
                let effective = document(&case, "EffectiveDeliveryPolicy", &effective);
                assert_normalized(&case, "EffectiveDeliveryPolicy", &configured, &effective);
            } else {
                assert_eq!(
                    effective, None,
                    "case {case:?}: no delivery to a {protocol} endpoint is retried under \
                     a delivery policy, so none must be reported, got {held:?}"
                );
            }

            // And the same document when `Subscribe` carries the policy instead: a
            // second topic, so the write is on a subscription of its own.
            let other = sns
                .topic(&format!("sub-delivery-policy-{index:02}-on-subscribe"))
                .await;
            let on_subscribe = subscribe(
                &sns,
                &case,
                &other,
                protocol,
                endpoint,
                &[("DeliveryPolicy", policy)],
            )
            .await;
            let carried = attribute_map(&read(&sns, &case, &on_subscribe).await);
            assert_eq!(
                carried.get("DeliveryPolicy"),
                held.get("DeliveryPolicy"),
                "case {case:?}: Subscribe must hold the policy to the same rule as \
                 SetSubscriptionAttributes, got {carried:?}"
            );
        }
    }
}

/// The raw-delivery flag is a boolean carried as a word: it is read in any case, reported
/// in the canonical lower case, and refused for anything that is not one of the two
/// words.
#[tokio::test]
async fn the_raw_delivery_flag_is_read_in_any_case_and_reported_in_lower_case() {
    let sns = Sns::new();
    // (case, the value written, the value the read reports — or None when refused)
    let cases: &[(&str, &str, Option<&str>)] = &[
        ("the canonical word", "true", Some("true")),
        ("capitalised", "True", Some("true")),
        ("in capitals", "TRUE", Some("true")),
        ("in mixed case", "tRuE", Some("true")),
        ("the other word", "false", Some("false")),
        ("the other word capitalised", "False", Some("false")),
        ("the other word in capitals", "FALSE", Some("false")),
        ("a word of its own", "yes", None),
        ("a number", "1", None),
        ("the other number", "0", None),
        ("a JSON literal in quotes", "\"true\"", None),
        ("a word with a trailing stop", "true.", None),
    ];

    for (index, (case, value, reports)) in cases.iter().enumerate() {
        let topic = sns.topic(&format!("raw-flag-case-{index:02}")).await;
        let arn = subscribe(&sns, case, &topic, "sqs", endpoint_of("sqs"), &[]).await;
        let (status, code, message) = set(&sns, &arn, "RawMessageDelivery", value).await;
        let held = attribute_map(&read(&sns, case, &arn).await);
        match reports {
            Some(expected) => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200, got {status} / {code} {message}"
                );
                assert_eq!(
                    held.get("RawMessageDelivery").map(String::as_str),
                    Some(*expected),
                    "case {case:?}: RawMessageDelivery must report {expected:?}, got \
                     {held:?}"
                );
            }
            None => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected 400, got {status} / {message}"
                );
                assert_eq!(
                    code, "InvalidParameter",
                    "case {case:?}: expected the InvalidParameter code, got {code}"
                );
                assert_eq!(
                    held.get("RawMessageDelivery").map(String::as_str),
                    Some("false"),
                    "case {case:?}: a refused write must leave the flag at its documented \
                     default, got {held:?}"
                );
            }
        }
    }
}

/// Raw message delivery may be ENABLED on a subscription whose endpoint takes the bare
/// bodies and on no other, and the subscription of any other protocol is refused with
/// SNS's own whole sentence.
///
/// The protocols are the crate's `RAW_MESSAGE_DELIVERY_PROTOCOLS` catalog, driven over
/// every protocol `Subscribe` accepts, so a protocol added to or removed from the catalog
/// moves its row with it.
#[tokio::test]
async fn raw_delivery_is_enabled_for_the_endpoints_that_take_bare_bodies() {
    let sns = Sns::new();
    for (index, (protocol, endpoint)) in PROTOCOL_ENDPOINTS.iter().enumerate() {
        let accepted = RAW_MESSAGE_DELIVERY_PROTOCOLS.contains(protocol);
        let case = format!("raw delivery on a {protocol} subscription");
        let topic = sns.topic(&format!("raw-protocol-{index:02}")).await;
        let arn = subscribe(&sns, &case, &topic, protocol, endpoint, &[]).await;
        let (status, code, message) = set(&sns, &arn, "RawMessageDelivery", "true").await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} / {code} {message}"
            );
            let held = attribute_map(&read(&sns, &case, &arn).await);
            assert_eq!(
                held.get("RawMessageDelivery").map(String::as_str),
                Some("true"),
                "case {case:?}: raw delivery must be enabled, got {held:?}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} / {message}"
        );
        assert_eq!(
            code, "InvalidParameter",
            "case {case:?}: expected the InvalidParameter code, got {code}"
        );
        assert_eq!(
            message,
            format!(
                "Invalid parameter: Delivery protocol [{protocol}] does not support raw \
                 message delivery."
            ),
            "case {case:?}: expected SNS's own sentence, got {message:?}"
        );
        // Switching the flag OFF is never scoped: every protocol carries the envelope.
        let (off, off_code, off_message) = set(&sns, &arn, "RawMessageDelivery", "FALSE").await;
        assert_eq!(
            off, 200,
            "case {case:?}: disabling raw delivery expected 200, got {off} / {off_code} \
             {off_message}"
        );
    }
}

/// A message published under the json structure must carry a `default` entry that names
/// a message: the entry is what every protocol no other member names receives. A member
/// that names a scalar names that body; one that is absent, or names a composite value no
/// protocol could receive, leaves the document with no default entry and the publish is
/// refused with SNS's own sentence.
///
/// The rows differ in the JSON type the `default` member carries, which is what the rule
/// generalises over; the recorded object-valued default is one row of it.
#[tokio::test]
async fn a_structured_message_must_name_a_default_entry() {
    let sns = Sns::new();
    let topic = sns.topic("default-entry").await;
    // (case, the published body, whether the publish is served)
    let cases: &[(&str, &str, bool)] = &[
        ("a string default", r#"{"default":"d"}"#, true),
        ("a whole-number default", r#"{"default":7}"#, true),
        ("a fractional default", r#"{"default":1.5}"#, true),
        ("a flag default", r#"{"default":true}"#, true),
        ("a null default", r#"{"default":null}"#, true),
        (
            "a default beside branches of every other form",
            r#"{"default":"d","sqs":{"k":"v"},"email":1,"http":null}"#,
            true,
        ),
        ("no default member at all", r#"{"sqs":"q"}"#, false),
        ("an object default", r#"{"default":{"x":1}}"#, false),
        ("a list default", r#"{"default":["d"]}"#, false),
    ];

    for (case, message, served) in cases {
        let answer = sns
            .call(
                "Publish",
                &[
                    ("TopicArn", &topic),
                    ("Message", message),
                    ("MessageStructure", "json"),
                ],
            )
            .await;
        if *served {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} / {}",
                answer.status, answer.body
            );
            assert!(
                answer
                    .element("MessageId")
                    .is_some_and(|id| !id.is_empty()),
                "case {case:?}: a served publish reports the id of the message, got {}",
                answer.body
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
        let message = answer.element("Message").unwrap_or_default();
        assert_eq!(
            message, NO_DEFAULT_ENTRY_REFUSAL,
            "case {case:?}: expected the sentence {NO_DEFAULT_ENTRY_REFUSAL:?}, got \
             {message:?}"
        );
    }
}

/// A message published with `MessageStructure` of `json` is delivered as the branch its
/// subscriber's protocol selects: the member named for the protocol when it names a
/// body, and the `default` member otherwise.
///
/// Observed where a client observes it — on the subscribed queue — for both delivery
/// forms: the raw body and the `Message` of the notification envelope.
#[tokio::test]
async fn a_structured_message_delivers_the_branch_its_protocol_selects() {
    let emulator = emulator();
    let topic = element(
        &sns_call(&emulator, "CreateTopic", &[("Name", "structured")]).await,
        "TopicArn",
    );
    let (raw_url, raw_arn) = given_queue(&emulator, "structured-raw").await;
    let (enveloped_url, enveloped_arn) = given_queue(&emulator, "structured-enveloped").await;
    for (arn, raw) in [(&raw_arn, true), (&enveloped_arn, false)] {
        let subscription = element(
            &sns_call(
                &emulator,
                "Subscribe",
                &[
                    ("TopicArn", &topic),
                    ("Protocol", "sqs"),
                    ("Endpoint", arn),
                ],
            )
            .await,
            "SubscriptionArn",
        );
        if raw {
            sns_call(
                &emulator,
                "SetSubscriptionAttributes",
                &[
                    ("SubscriptionArn", &subscription),
                    ("AttributeName", "RawMessageDelivery"),
                    ("AttributeValue", "true"),
                ],
            )
            .await;
        }
    }

    // (case, the `MessageStructure` published under, the published body, the body an
    //  `sqs` subscriber receives)
    let cases: &[(&str, &str, &str, &str)] = &[
        ("only a default branch", "json", r#"{"default":"d"}"#, "d"),
        // The `default` member is not a protocol branch: it is the message every
        // protocol no branch names receives, and a scalar names that body as the text
        // SNS writes it.
        ("a number as the default branch", "json", r#"{"default":7}"#, "7"),
        (
            "a boolean as the default branch",
            "json",
            r#"{"default":false}"#,
            "false",
        ),
        (
            "a branch for this protocol",
            "json",
            r#"{"default":"d","sqs":"q"}"#,
            "q",
        ),
        (
            "a branch for another protocol only",
            "json",
            r#"{"default":"d","email":"e"}"#,
            "d",
        ),
        // A protocol's branch is delivered as its own JSON TEXT: a scalar that is not a
        // string names the body SNS writes that value out as, which is what the recorded
        // service delivers for a number-valued branch.
        (
            "a whole-number branch",
            "json",
            r#"{"default":"d","sqs":7}"#,
            "7",
        ),
        (
            "a negative-number branch",
            "json",
            r#"{"default":"d","sqs":-12}"#,
            "-12",
        ),
        (
            "a fractional branch",
            "json",
            r#"{"default":"d","sqs":1.5}"#,
            "1.5",
        ),
        (
            "a boolean branch",
            "json",
            r#"{"default":"d","sqs":true}"#,
            "true",
        ),
        // A branch that names NO message is ignored, so its protocol receives what it
        // would have received with no branch at all.
        (
            "an object branch",
            "json",
            r#"{"default":"d","sqs":{"k":"v"}}"#,
            "d",
        ),
        (
            "a list branch",
            "json",
            r#"{"default":"d","sqs":["a"]}"#,
            "d",
        ),
        (
            "a null branch",
            "json",
            r#"{"default":"d","sqs":null}"#,
            "d",
        ),
        (
            "an ignored branch for this protocol beside a used one for another",
            "json",
            r#"{"default":"d","email":"e","sqs":{"k":"v"}}"#,
            "d",
        ),
        (
            "branches of every form at once",
            "json",
            r#"{"default":"d","email":1,"lambda":{"k":"v"},"sqs":"q"}"#,
            "q",
        ),
        // And a value naming no structure SNS reads leaves the body ONE message for
        // every protocol: nothing selects a branch out of it.
        (
            "a structure SNS defines nothing for",
            "xml",
            r#"{"default":"d","sqs":"q"}"#,
            r#"{"default":"d","sqs":"q"}"#,
        ),
    ];

    for (case, structure, message, expected) in cases {
        sns_call(
            &emulator,
            "Publish",
            &[
                ("TopicArn", &topic),
                ("Message", message),
                ("MessageStructure", structure),
            ],
        )
        .await;

        let raw_delivered = drain(&emulator, &raw_url).await;
        let bodies: Vec<&str> = raw_delivered
            .iter()
            .map(|(body, _)| body.as_str())
            .collect();
        assert_eq!(
            bodies,
            vec![*expected],
            "case {case:?}: a raw delivery must carry the branch body, got {bodies:?}"
        );

        let enveloped = drain(&emulator, &enveloped_url).await;
        let Some((body, _)) = enveloped.first() else {
            panic!("case {case:?}: the enveloped subscriber received nothing");
        };
        let envelope: Value = serde_json::from_str(body)
            .unwrap_or_else(|error| panic!("case {case:?}: body is not JSON ({error}): {body}"));
        assert_eq!(
            envelope["Message"],
            json!(expected),
            "case {case:?}: the envelope must carry the branch body, got {envelope}"
        );
    }
}
