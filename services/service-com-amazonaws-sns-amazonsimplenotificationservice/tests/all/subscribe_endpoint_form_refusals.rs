// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for WHICH sentence `Subscribe` refuses an unusable `Endpoint` with,
//! driven over the wire through the router the emulator serves in production.
//!
//! `Endpoint` holds a different kind of reference per protocol
//! (`smithy_query(query="doc",shape="SubscribeInput",member="Endpoint")`: a URL for
//! `http`/`https`, an address for `email`/`email-json`, a phone number for `sms`, and the
//! ARN of the named resource for `sqs`, `application`, `lambda` and `firehose`), and the
//! refusal says which of two things went wrong with the value:
//!
//! * SNS could not read it as the reference the protocol takes at all — for an
//!   ARN-addressed protocol, a value without an ARN's six `arn:`-led fields — so the
//!   sentence states the FORM required, `Invalid parameter: <Service> endpoint ARN`, where
//!   `<Service>` is the AWS service the protocol string names;
//! * SNS read it and did not accept it — an ARN of another service, or of the same service
//!   naming another kind of resource — so the sentence names the input MEMBER,
//!   `Invalid parameter: Endpoint`, and nothing else. A request carrying no endpoint at
//!   all takes the same sentence: that is a member with no value, not a reference SNS
//!   could not read.
//!
//! Both tables below span the rule rather than one value: every protocol the model lists,
//! and, for the ARN-addressed ones, each way a value can fail to be an ARN (a bare name, a
//! partial `<account>:<type>:<name>` reference, a URL, a six-field value that does not
//! begin with `arn:`) against each way an ARN can be the wrong one. Neither refusal
//! creates anything, and the form each protocol does take is subscribed and reads back.

use super::topics_subscriptions_publish::{Answer, Sns};

/// The code SNS names a refusal of a parameter by, as the modelled
/// `InvalidParameterException`'s `awsQueryError` declares it.
const INVALID_PARAMETER: &str = "InvalidParameter";

/// The sentence for a value SNS read as the protocol's kind of reference and did not
/// accept — the input member's name, with nothing of the request in it.
const MEMBER_REFUSAL: &str = "Invalid parameter: Endpoint";

/// `(protocol, a value that is not an ARN at all, the sentence it is refused with)`.
///
/// The protocols whose endpoint is the ARN of the service the protocol names, each with
/// the four ways a value can fail to be an ARN: a bare resource name, the partial
/// `<account>:<type>:<name>` form an AWS SDK may accept elsewhere, a URL (what a caller
/// reaches for when the resource's console or queue address is at hand), and a value with
/// an ARN's six fields that does not begin with `arn:`.
const NOT_AN_ARN: &[(&str, &str, &str)] = &[
    ("sqs", "a-queue", "Invalid parameter: SQS endpoint ARN"),
    (
        "sqs",
        "000000000000:a-queue",
        "Invalid parameter: SQS endpoint ARN",
    ),
    (
        "sqs",
        "https://sqs.us-east-1.amazonaws.com/000000000000/a-queue",
        "Invalid parameter: SQS endpoint ARN",
    ),
    (
        "sqs",
        "urn:aws:sqs:us-east-1:000000000000:a-queue",
        "Invalid parameter: SQS endpoint ARN",
    ),
    (
        "lambda",
        "a-function",
        "Invalid parameter: Lambda endpoint ARN",
    ),
    (
        "lambda",
        "000000000000:function:a-function",
        "Invalid parameter: Lambda endpoint ARN",
    ),
    (
        "lambda",
        "https://lambda.us-east-1.amazonaws.com/2015-03-31/functions/a-function",
        "Invalid parameter: Lambda endpoint ARN",
    ),
    (
        "lambda",
        "aws:lambda:us-east-1:000000000000:function:a-function",
        "Invalid parameter: Lambda endpoint ARN",
    ),
    (
        "firehose",
        "a-stream",
        "Invalid parameter: Firehose endpoint ARN",
    ),
    (
        "firehose",
        "000000000000:deliverystream/a-stream",
        "Invalid parameter: Firehose endpoint ARN",
    ),
    (
        "firehose",
        "https://firehose.us-east-1.amazonaws.com/a-stream",
        "Invalid parameter: Firehose endpoint ARN",
    ),
    (
        "firehose",
        "arn-aws:firehose:us-east-1:000000000000:deliverystream/a-stream",
        "Invalid parameter: Firehose endpoint ARN",
    ),
    // `application` addresses this service's OWN platform endpoint rather than another
    // service's resource, so the protocol names no service and every refusal of it stays
    // the member-named sentence — whether the value is an ARN or not.
    ("application", "an-endpoint-id", MEMBER_REFUSAL),
    (
        "application",
        "endpoint/GCM/an-app/22222222-2222-4222-8222-222222222222",
        MEMBER_REFUSAL,
    ),
    // The protocols whose endpoint is not an ARN at all: a value SNS cannot read as their
    // kind of reference has no ARN form to ask for, so it keeps the member's name.
    ("http", "example.test/hook", MEMBER_REFUSAL),
    ("https", "http://example.test/hook", MEMBER_REFUSAL),
    ("email", "ops@localhost", MEMBER_REFUSAL),
    ("email-json", "not-an-address", MEMBER_REFUSAL),
    ("sms", "not-a-number", MEMBER_REFUSAL),
];

/// `(protocol, an ARN-shaped value that is not the resource the protocol takes)`.
///
/// Every row has an ARN's six `arn:`-led fields, so SNS reads it: it is the wrong ARN,
/// not an unreadable value, and whichever protocol asked for it the refusal names the
/// input member. Two ways to be wrong per ARN-addressed protocol — another service's ARN,
/// and the same service's ARN naming another kind of resource — plus the ARN handed to a
/// protocol whose endpoint is never an ARN.
const WRONG_ARN: &[(&str, &str)] = &[
    ("sqs", "arn:aws:sns:us-east-1:000000000000:a-topic"),
    ("sqs", "arn:aws:sqs:us-east-1:000000000000:"),
    ("lambda", "arn:aws:sqs:us-east-1:000000000000:a-queue"),
    ("lambda", "arn:aws:lambda:us-east-1:000000000000:layer:a-layer"),
    ("firehose", "arn:aws:sqs:us-east-1:000000000000:a-queue"),
    (
        "firehose",
        "arn:aws:firehose:us-east-1:000000000000:stream/a-stream",
    ),
    ("application", "arn:aws:sqs:us-east-1:000000000000:a-queue"),
    (
        "application",
        "arn:aws:sns:us-east-1:000000000000:app/GCM/an-app",
    ),
    ("http", "arn:aws:sqs:us-east-1:000000000000:a-queue"),
    ("email", "arn:aws:sqs:us-east-1:000000000000:a-queue"),
    ("sms", "arn:aws:sqs:us-east-1:000000000000:a-queue"),
];

/// `(protocol, the endpoint form that protocol does take)` — the control that every
/// refusal above is about the VALUE and not about the protocol.
const ACCEPTED: &[(&str, &str)] = &[
    ("http", "http://example.test/hook"),
    ("https", "https://example.test/hook"),
    ("email", "ops@example.test"),
    ("email-json", "ops@example.test"),
    ("sms", "+15550100"),
    ("sqs", "arn:aws:sqs:us-east-1:000000000000:a-queue"),
    (
        "application",
        "arn:aws:sns:us-east-1:000000000000:endpoint/GCM/an-app/\
         22222222-2222-4222-8222-222222222222",
    ),
    (
        "lambda",
        "arn:aws:lambda:us-east-1:000000000000:function:a-function",
    ),
    (
        "firehose",
        "arn:aws:firehose:us-east-1:000000000000:deliverystream/a-stream",
    ),
];

/// Subscribe `endpoint` (when there is one) over `protocol` to `topic`.
async fn subscribe(sns: &Sns, topic: &str, protocol: &str, endpoint: Option<&str>) -> Answer {
    let mut params: Vec<(&str, &str)> = vec![
        ("TopicArn", topic),
        ("Protocol", protocol),
        ("ReturnSubscriptionArn", "true"),
    ];
    if let Some(endpoint) = endpoint {
        params.push(("Endpoint", endpoint));
    }
    sns.call("Subscribe", &params).await
}

/// Assert that `answer` is the refusal `want`, and that it says so as SNS does: HTTP 400
/// under the modelled `InvalidParameter` code.
fn assert_refused(case: &str, answer: &Answer, want: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected 400, got {} / {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.code(),
        INVALID_PARAMETER,
        "case {case:?}: expected the {INVALID_PARAMETER} code, got {}",
        answer.body
    );
    assert_eq!(
        answer.element("Message").as_deref(),
        Some(want),
        "case {case:?}: expected {want:?}, got {}",
        answer.body
    );
}

/// A value SNS cannot read as the reference its protocol takes is refused with the FORM
/// that protocol requires when the protocol names the service whose ARN it is, and with
/// the input member's name otherwise.
#[tokio::test]
async fn an_endpoint_that_is_not_the_protocols_kind_of_reference_states_the_form_required() {
    let sns = Sns::new();
    let topic = sns.topic("endpoint-form-refusals").await;

    for (protocol, endpoint, want) in NOT_AN_ARN {
        let case = format!("{protocol} endpoint {endpoint:?}");
        let answer = subscribe(&sns, &topic, protocol, Some(endpoint)).await;
        assert_refused(&case, &answer, want);
    }

    // Nothing was created by any of them: the refusal is judged before the store is
    // touched, so the topic holds no subscription at all.
    let listed = sns
        .call("ListSubscriptionsByTopic", &[("TopicArn", &topic)])
        .await;
    assert_eq!(
        listed.status, 200,
        "ListSubscriptionsByTopic: expected 200, got {} / {}",
        listed.status, listed.body
    );
    assert!(
        listed.elements("SubscriptionArn").is_empty(),
        "a refused Subscribe must create nothing, got {}",
        listed.body
    );
}

/// An `Endpoint` SNS DOES read as an ARN but does not accept — another service's, or the
/// same service's naming another kind of resource — is refused with the input member's
/// name, whichever protocol asked for it; so is a request that carries no endpoint at all.
#[tokio::test]
async fn an_endpoint_that_is_the_wrong_arn_names_the_member() {
    let sns = Sns::new();
    let topic = sns.topic("endpoint-wrong-arn-refusals").await;

    for (protocol, endpoint) in WRONG_ARN {
        let case = format!("{protocol} endpoint {endpoint:?}");
        let answer = subscribe(&sns, &topic, protocol, Some(endpoint)).await;
        assert_refused(&case, &answer, MEMBER_REFUSAL);
    }

    // A member with no value at all, for every protocol: there is no reference to read, so
    // no form to ask for either.
    for (protocol, _) in ACCEPTED {
        let case = format!("a {protocol} subscription with no endpoint");
        let answer = subscribe(&sns, &topic, protocol, None).await;
        assert_refused(&case, &answer, MEMBER_REFUSAL);
    }

    let listed = sns
        .call("ListSubscriptionsByTopic", &[("TopicArn", &topic)])
        .await;
    assert!(
        listed.elements("SubscriptionArn").is_empty(),
        "a refused Subscribe must create nothing, got {}",
        listed.body
    );
}

/// The form each protocol DOES take is subscribed and reads back through
/// `GetSubscriptionAttributes` — the control that the refusals above are about the value's
/// form and not about the protocol being asked for.
#[tokio::test]
async fn the_documented_endpoint_form_of_every_protocol_is_subscribed() {
    let sns = Sns::new();

    for (index, (protocol, endpoint)) in ACCEPTED.iter().enumerate() {
        let topic = sns
            .topic(&format!("endpoint-form-accepted-{index:02}"))
            .await;
        let case = format!("a {protocol} endpoint of the documented form");
        let mut params: Vec<(&str, &str)> = vec![
            ("TopicArn", topic.as_str()),
            ("Protocol", protocol),
            ("Endpoint", endpoint),
            ("ReturnSubscriptionArn", "true"),
        ];
        // "The following attribute applies only to Amazon Data Firehose delivery stream
        // subscriptions: SubscriptionRoleArn" (`SubscribeInput$Attributes` doc) — a rule
        // about the attributes rather than about the endpoint.
        if *protocol == "firehose" {
            params.push(("Attributes.entry.1.key", "SubscriptionRoleArn"));
            params.push((
                "Attributes.entry.1.value",
                "arn:aws:iam::000000000000:role/sns-firehose",
            ));
        }
        let answer = sns.call("Subscribe", &params).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} / {}",
            answer.status, answer.body
        );
        let arn = answer.element("SubscriptionArn").unwrap_or_else(|| {
            panic!("case {case:?}: expected a SubscriptionArn in {}", answer.body)
        });

        let read = sns
            .call("GetSubscriptionAttributes", &[("SubscriptionArn", &arn)])
            .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: GetSubscriptionAttributes expected 200, got {} / {}",
            read.status, read.body
        );
        assert!(
            read.elements("value").iter().any(|value| value == endpoint),
            "case {case:?}: expected the stored endpoint {endpoint:?}, got {}",
            read.body
        );
    }
}
