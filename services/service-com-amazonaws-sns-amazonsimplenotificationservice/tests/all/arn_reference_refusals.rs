// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for how SNS judges the ARN REFERENCE a call carries, driven over the
//! wire through the router the emulator serves in production.
//!
//! An ARN names three things an SNS call is judged on before the resource it addresses is
//! looked up, and each one has its own refusal:
//!
//! 1. the ACCOUNT. An endpoint serves one account, so an ARN naming another names a
//!    resource it cannot resolve at all: the call is refused `403` /
//!    `InvalidClientTokenId` with the account-resolution layer's own fixed sentence,
//!    before any record of the account it does serve is read.
//! 2. the RESOURCE of a topic ARN, which is the topic's NAME: a resource that is not a
//!    name a topic could ever have is refused `400` / `InvalidParameter` /
//!    `Invalid parameter: Topic Name` — the part of the ARN at fault, with no value
//!    echoed.
//! 3. the SUBSCRIPTION ID of a subscription ARN: an ARN-shaped value that does not name a
//!    subscription is refused `400` / `InvalidParameter` /
//!    `Invalid parameter: SubscriptionId`, the same way.
//!
//! Beside them, the two refusals the framework answers for a request that never reached a
//! handler — an absent member the service resolves the resource from, which takes SNS's
//! own `InvalidParameter`, against every other declared-rule failure, which keeps the
//! validation layer's sentence under its `ValidationError` code — and the two reports a
//! topic's bookkeeping makes: the deleted-subscription count that settles after the
//! `Unsubscribe` rather than inside it, and the continuation token the account-wide
//! subscription listing hands back exactly while subscriptions remain.
//!
//! Every table's rows differ along what its rule generalises over: the ARN MEMBER and the
//! operation that resolves it, the account id named, the form of the unusable resource
//! part, the protocol an endpoint was refused for, and the number of subscriptions a
//! topic's counters are read over. The cases the wire corpus recorded are one row each.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_sns_amazonsimplenotificationservice::state::account_resolution::{
    ARN_MEMBERS, NO_ACCOUNT_CODE, NO_ACCOUNT_MESSAGE, NO_ACCOUNT_STATUS,
};
use service_com_amazonaws_sns_amazonsimplenotificationservice::state::{
    INVALID_SUBSCRIPTION_ID, INVALID_TOPIC_NAME, MAX_SETTLING_DELETIONS_PER_TOPIC,
    MAX_TOPIC_NAME_LEN, PAGE_SIZE, TOPIC_DOES_NOT_EXIST,
};

use super::topics_subscriptions_publish::{attribute_map, Answer, Sns};

/// The region every call below is addressed to, which is also the region the ARNs it
/// carries name: the account rule is judged on an ARN of the endpoint's OWN region, so
/// the region rule cannot answer in its place.
const REGION: &str = "us-east-1";

/// The code SNS names a refusal of a parameter by, as the modelled
/// `InvalidParameterException`'s `awsQueryError` declares it.
const INVALID_PARAMETER: &str = "InvalidParameter";

/// The code the query protocol's validation layer names a declared-rule failure by.
const VALIDATION_ERROR: &str = "ValidationError";

/// The account ids this endpoint does NOT serve. Three of them, so no row can pass by
/// matching one id.
const OTHER_ACCOUNTS: &[&str] = &["111111111111", "123456789012", "999988887777"];

/// An ARN of the KIND `member` carries, in `region` under `account`.
///
/// Each member takes the resource form SNS mints for it: a topic's name, a subscription's
/// topic and id, a platform application's `app/<platform>/<name>` and a device
/// endpoint's `endpoint/<platform>/<app>/<id>`. None of them needs to exist — the rules
/// under test are judged before the lookup, which is what the served-account control row
/// of each table shows.
fn arn_of(member: &str, region: &str, account: &str) -> String {
    let base = format!("arn:aws:sns:{region}:{account}");
    match member {
        "SubscriptionArn" => {
            format!("{base}:reference-topic:11111111-1111-4111-8111-111111111111")
        }
        "EndpointArn" => {
            format!("{base}:endpoint/GCM/reference-app/22222222-2222-4222-8222-222222222222")
        }
        "PlatformApplicationArn" => format!("{base}:app/GCM/reference-app"),
        _ => format!("{base}:reference-topic"),
    }
}

/// One operation of the ARN-member table: the case, the action, the ARN member it
/// addresses its resource through, and the other parameters it requires.
type Addressed = (
    &'static str,
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
);

/// Every ARN member of this service, with an operation that resolves it and the rest of
/// that operation's required input.
///
/// Source for the member list: `ARN_MEMBERS`, whose own doc cites the `smithy_query`
/// digest it came from; the assertion below holds this table to it, so a member added
/// there cannot go untested here.
const ADDRESSED: &[Addressed] = &[
    ("GetTopicAttributes", "GetTopicAttributes", "TopicArn", &[]),
    (
        "ListSubscriptionsByTopic",
        "ListSubscriptionsByTopic",
        "TopicArn",
        &[],
    ),
    ("DeleteTopic", "DeleteTopic", "TopicArn", &[]),
    (
        "SetTopicAttributes",
        "SetTopicAttributes",
        "TopicArn",
        &[("AttributeName", "DisplayName"), ("AttributeValue", "x")],
    ),
    (
        "Subscribe",
        "Subscribe",
        "TopicArn",
        &[
            ("Protocol", "sqs"),
            ("Endpoint", "arn:aws:sqs:us-east-1:000000000000:reference-queue"),
        ],
    ),
    (
        "AddPermission",
        "AddPermission",
        "TopicArn",
        &[
            ("Label", "reference-label"),
            ("AWSAccountId.member.1", "000000000000"),
            ("ActionName.member.1", "Publish"),
        ],
    ),
    (
        "ConfirmSubscription",
        "ConfirmSubscription",
        "TopicArn",
        &[("Token", "reference-token")],
    ),
    (
        "Publish on a topic",
        "Publish",
        "TopicArn",
        &[("Message", "m")],
    ),
    (
        "Publish on a target",
        "Publish",
        "TargetArn",
        &[("Message", "m")],
    ),
    (
        "GetSubscriptionAttributes",
        "GetSubscriptionAttributes",
        "SubscriptionArn",
        &[],
    ),
    ("Unsubscribe", "Unsubscribe", "SubscriptionArn", &[]),
    (
        "SetSubscriptionAttributes",
        "SetSubscriptionAttributes",
        "SubscriptionArn",
        &[("AttributeName", "RawMessageDelivery"), ("AttributeValue", "true")],
    ),
    (
        "ListTagsForResource",
        "ListTagsForResource",
        "ResourceArn",
        &[],
    ),
    (
        "TagResource",
        "TagResource",
        "ResourceArn",
        &[("Tags.member.1.Key", "team"), ("Tags.member.1.Value", "payments")],
    ),
    (
        "GetDataProtectionPolicy",
        "GetDataProtectionPolicy",
        "ResourceArn",
        &[],
    ),
    (
        "GetEndpointAttributes",
        "GetEndpointAttributes",
        "EndpointArn",
        &[],
    ),
    (
        "SetEndpointAttributes",
        "SetEndpointAttributes",
        "EndpointArn",
        &[
            ("Attributes.entry.1.key", "CustomUserData"),
            ("Attributes.entry.1.value", "user-data"),
        ],
    ),
    ("DeleteEndpoint", "DeleteEndpoint", "EndpointArn", &[]),
    (
        "GetPlatformApplicationAttributes",
        "GetPlatformApplicationAttributes",
        "PlatformApplicationArn",
        &[],
    ),
    (
        "SetPlatformApplicationAttributes",
        "SetPlatformApplicationAttributes",
        "PlatformApplicationArn",
        &[
            ("Attributes.entry.1.key", "PlatformCredential"),
            ("Attributes.entry.1.value", "AIzaSyA-credential-000000000000000000000"),
        ],
    ),
    (
        "DeletePlatformApplication",
        "DeletePlatformApplication",
        "PlatformApplicationArn",
        &[],
    ),
    (
        "CreatePlatformEndpoint",
        "CreatePlatformEndpoint",
        "PlatformApplicationArn",
        &[("Token", "device-token")],
    ),
    (
        "ListEndpointsByPlatformApplication",
        "ListEndpointsByPlatformApplication",
        "PlatformApplicationArn",
        &[],
    ),
];

/// Call `action` with `arn` under `member`, plus the row's other parameters.
async fn call_with(sns: &Sns, action: &str, member: &str, arn: &str, extra: &[(&str, &str)]) -> Answer {
    let mut params: Vec<(&str, &str)> = vec![(member, arn)];
    params.extend_from_slice(extra);
    sns.call(action, &params).await
}

/// An ARN naming an account this endpoint does not serve is refused by the
/// account-resolution layer — `403` / `InvalidClientTokenId` and its fixed sentence —
/// whichever ARN member carried it and whichever operation was called, and before any
/// record is read: the ARNs below name nothing that exists, and the SAME ARNs under the
/// served account are answered by the operation itself instead.
#[tokio::test]
async fn an_arn_naming_another_account_is_refused_before_the_lookup() {
    let sns = Sns::new();
    let served = engine::emulator_account_id().to_string();

    // The table covers every ARN member of the service, so the rule is asserted on the
    // member set itself rather than on the two members the corpus recorded.
    for member in ARN_MEMBERS {
        assert!(
            ADDRESSED.iter().any(|(_, _, named, _)| named == member),
            "the ARN member {member:?} has no row in this table"
        );
    }

    for (case, action, member, extra) in ADDRESSED {
        for account in OTHER_ACCOUNTS {
            let arn = arn_of(member, REGION, account);
            let answer = call_with(&sns, action, member, &arn, extra).await;
            assert_eq!(
                answer.status, NO_ACCOUNT_STATUS,
                "case {case:?} / account {account}: expected {NO_ACCOUNT_STATUS}, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                NO_ACCOUNT_CODE,
                "case {case:?} / account {account}: expected the {NO_ACCOUNT_CODE} code, got {}",
                answer.body
            );
            assert_eq!(
                answer.element("Message").as_deref(),
                Some(NO_ACCOUNT_MESSAGE),
                "case {case:?} / account {account}: expected {NO_ACCOUNT_MESSAGE:?}, got {}",
                answer.body
            );
            // The refusal reports nothing of the request: neither the ARN nor the
            // account it named is in the document.
            for echoed in [arn.as_str(), account] {
                assert!(
                    !answer.body.contains(echoed),
                    "case {case:?} / account {account}: the refusal must not reproduce \
                     {echoed:?}, got {}",
                    answer.body
                );
            }
        }

        // The control: the same ARN under the account this endpoint DOES serve is the
        // operation's own answer — a lookup miss, or a served call — and never the
        // account-resolution refusal.
        let own = arn_of(member, REGION, &served);
        let answer = call_with(&sns, action, member, &own, extra).await;
        assert_ne!(
            answer.code(),
            NO_ACCOUNT_CODE,
            "case {case:?}: an ARN of the served account must reach the operation, got {}",
            answer.body
        );
        assert_ne!(
            answer.status, NO_ACCOUNT_STATUS,
            "case {case:?}: an ARN of the served account must reach the operation, got {} / {}",
            answer.status, answer.body
        );
    }

    // An ARN of another account in a member that does NOT address a resource is not an
    // account reference at all: a published message carrying one is delivered as written.
    let topic = sns.topic("account-reference-topic").await;
    let message = format!("see {}", arn_of("TopicArn", REGION, OTHER_ACCOUNTS[0]));
    let published = sns
        .call(
            "Publish",
            &[("TopicArn", topic.as_str()), ("Message", message.as_str())],
        )
        .await;
    assert_eq!(
        published.status, 200,
        "case \"a message that quotes another account's ARN\": expected 200, got {} / {}",
        published.status, published.body
    );

    // And an ARN of another account in another REGION is answered by the region rule the
    // endpoint applies first: it serves no region but its own, whatever account is named.
    let elsewhere = arn_of("TopicArn", "eu-west-1", OTHER_ACCOUNTS[0]);
    let answer = sns
        .call("GetTopicAttributes", &[("TopicArn", elsewhere.as_str())])
        .await;
    assert_eq!(
        answer.code(),
        INVALID_PARAMETER,
        "case \"another account in another region\": expected the region refusal, got {}",
        answer.body
    );
}

/// The resource parts that are not names a topic could ever have: `(case, resource)`.
///
/// The name rule is the one `CreateTopic` holds a name to — ASCII letters, digits, `_`
/// and `-`, up to `MAX_TOPIC_NAME_LEN` characters, with a `.fifo` suffix the one
/// exception — so the rows are the ways a resource part breaks it: another kind of SNS
/// resource, a subscription's trailing id, a character the rule excludes, nothing at all,
/// and a name one past the documented maximum.
fn unusable_topic_resources() -> Vec<(String, String)> {
    let past_bound = "n".repeat(MAX_TOPIC_NAME_LEN + 1);
    vec![
        ("a subscription's id".to_string(), "a-topic:11111111-1111-4111-8111-111111111111".to_string()),
        (
            "a platform endpoint".to_string(),
            "endpoint/GCM/an-app/22222222-2222-4222-8222-222222222222".to_string(),
        ),
        ("a platform application".to_string(), "app/GCM/an-app".to_string()),
        ("a space in the name".to_string(), "two words".to_string()),
        ("a period that is no FIFO suffix".to_string(), "dotted.name".to_string()),
        ("a character the rule excludes".to_string(), "bang!name".to_string()),
        ("no resource at all".to_string(), String::new()),
        ("one past the documented maximum".to_string(), past_bound),
    ]
}

/// A topic ARN whose resource is not a legal topic name is BAD INPUT, refused with the
/// fixed sentence naming that part of the ARN, before the store is consulted — and an
/// ARN whose resource IS a legal name, at the documented bound included, reaches the
/// lookup and is answered as a topic that does not exist.
#[tokio::test]
async fn a_topic_arn_whose_resource_is_no_topic_name_is_refused_as_bad_input() {
    let sns = Sns::new();
    let served = engine::emulator_account_id().to_string();
    // Every operation that addresses a topic by ARN and refuses on the spot. `DeleteTopic`
    // is among them: its documented idempotence covers a topic that does not exist, not a
    // value that names none.
    let actions: &[(&str, &[(&str, &str)])] = &[
        ("GetTopicAttributes", &[]),
        ("ListSubscriptionsByTopic", &[]),
        ("DeleteTopic", &[]),
        (
            "SetTopicAttributes",
            &[("AttributeName", "DisplayName"), ("AttributeValue", "x")],
        ),
        (
            "Subscribe",
            &[
                ("Protocol", "sqs"),
                ("Endpoint", "arn:aws:sqs:us-east-1:000000000000:reference-queue"),
            ],
        ),
        ("Publish", &[("Message", "m")]),
        (
            "AddPermission",
            &[
                ("Label", "reference-label"),
                ("AWSAccountId.member.1", "000000000000"),
                ("ActionName.member.1", "Publish"),
            ],
        ),
        ("ConfirmSubscription", &[("Token", "reference-token")]),
    ];

    for (case, resource) in unusable_topic_resources() {
        let arn = format!("arn:aws:sns:{REGION}:{served}:{resource}");
        for (action, extra) in actions {
            let answer = call_with(&sns, action, "TopicArn", &arn, extra).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?} on {action}: expected 400, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                INVALID_PARAMETER,
                "case {case:?} on {action}: expected the {INVALID_PARAMETER} code, got {}",
                answer.body
            );
            assert_eq!(
                answer.element("Message").as_deref(),
                Some(INVALID_TOPIC_NAME),
                "case {case:?} on {action}: expected {INVALID_TOPIC_NAME:?}, got {}",
                answer.body
            );
            if !resource.is_empty() {
                assert!(
                    !answer.body.contains(resource.as_str()),
                    "case {case:?} on {action}: the refusal must not reproduce the \
                     resource it rejected, got {}",
                    answer.body
                );
            }
        }
    }

    // The boundary: resources that ARE topic names — the characters the rule admits, a
    // FIFO topic's suffix, and a name exactly at the documented maximum — reach the
    // lookup, which reports a topic that does not exist.
    let at_bound = "b".repeat(MAX_TOPIC_NAME_LEN);
    let legal = [
        ("letters, digits, underscore and dash", "Topic_name-01".to_string()),
        ("a FIFO topic's suffix", "ordered-topic.fifo".to_string()),
        ("exactly the documented maximum", at_bound),
    ];
    for (case, resource) in &legal {
        let arn = format!("arn:aws:sns:{REGION}:{served}:{resource}");
        let answer = sns
            .call("GetTopicAttributes", &[("TopicArn", arn.as_str())])
            .await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: a legal name reaches the lookup, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.element("Message").as_deref(),
            Some(TOPIC_DOES_NOT_EXIST),
            "case {case:?}: expected the missing-topic sentence, got {}",
            answer.body
        );
    }
}

/// One row of the absent-ARN-member table: the case, the action, the ARN member left
/// out, and the other parameters sent.
type AbsentArn = (
    &'static str,
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
);

/// One row of the absent-declared-member table: the case, the action, the member as a
/// validation path spells it, and the parameters sent.
type AbsentDeclared<'a> = (&'a str, &'a str, &'a str, Vec<(&'a str, &'a str)>);

/// An absent required member is refused as the layer that judged it words it: the member
/// SNS resolves the addressed resource from takes the service's own `InvalidParameter`
/// and names itself as the request spells it, and every other declared-rule failure keeps
/// the validation layer's own sentence under its `ValidationError` code.
///
/// The rows differ in the member absent, the operation, and which of the two layers
/// judges it; the recorded `ListSubscriptionsByTopic` call carrying no `TopicArn` is one
/// row of the first group.
#[tokio::test]
async fn an_absent_required_member_is_refused_by_the_layer_that_judges_it() {
    let sns = Sns::new();
    let topic = sns.topic("absent-member-topic").await;

    // (case, action, the ARN member left out, the other parameters sent)
    let addressed: &[AbsentArn] = &[
        ("GetTopicAttributes with no TopicArn", "GetTopicAttributes", "TopicArn", &[]),
        (
            "ListSubscriptionsByTopic with no TopicArn",
            "ListSubscriptionsByTopic",
            "TopicArn",
            &[],
        ),
        ("DeleteTopic with no TopicArn", "DeleteTopic", "TopicArn", &[]),
        (
            "Subscribe with no TopicArn",
            "Subscribe",
            "TopicArn",
            &[
                ("Protocol", "sqs"),
                ("Endpoint", "arn:aws:sqs:us-east-1:000000000000:reference-queue"),
            ],
        ),
        (
            "GetSubscriptionAttributes with no SubscriptionArn",
            "GetSubscriptionAttributes",
            "SubscriptionArn",
            &[],
        ),
        ("Unsubscribe with no SubscriptionArn", "Unsubscribe", "SubscriptionArn", &[]),
        (
            "ListTagsForResource with no ResourceArn",
            "ListTagsForResource",
            "ResourceArn",
            &[],
        ),
        (
            "GetEndpointAttributes with no EndpointArn",
            "GetEndpointAttributes",
            "EndpointArn",
            &[],
        ),
        (
            "GetPlatformApplicationAttributes with no PlatformApplicationArn",
            "GetPlatformApplicationAttributes",
            "PlatformApplicationArn",
            &[],
        ),
    ];

    for (case, action, member, extra) in addressed {
        let answer = sns.call(action, extra).await;
        let want = format!("Invalid parameter: {member} Reason: no value for required parameter");
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
            Some(want.as_str()),
            "case {case:?}: expected {want:?}, got {}",
            answer.body
        );
    }

    // (case, action, the member left out as a validation path spells it, the parameters
    // sent) — members that carry no resource reference, so the validation layer judges
    // them.
    let declared: Vec<AbsentDeclared> = vec![
        (
            "Subscribe with no Protocol",
            "Subscribe",
            "protocol",
            vec![
                ("TopicArn", topic.as_str()),
                ("Endpoint", "arn:aws:sqs:us-east-1:000000000000:reference-queue"),
            ],
        ),
        (
            "Publish with no Message",
            "Publish",
            "message",
            vec![("TopicArn", topic.as_str())],
        ),
        (
            "SetTopicAttributes with no AttributeName",
            "SetTopicAttributes",
            "attributeName",
            vec![("TopicArn", topic.as_str()), ("AttributeValue", "x")],
        ),
        (
            "ConfirmSubscription with no Token",
            "ConfirmSubscription",
            "token",
            vec![("TopicArn", topic.as_str())],
        ),
        (
            "PublishBatch with no entries",
            "PublishBatch",
            "publishBatchRequestEntries",
            vec![("TopicArn", topic.as_str())],
        ),
        (
            "AddPermission with no Label",
            "AddPermission",
            "label",
            vec![
                ("TopicArn", topic.as_str()),
                ("AWSAccountId.member.1", "000000000000"),
                ("ActionName.member.1", "Publish"),
            ],
        ),
    ];

    for (case, action, path, params) in &declared {
        let answer = sns.call(action, params).await;
        let message = answer.element("Message").unwrap_or_default();
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            VALIDATION_ERROR,
            "case {case:?}: expected the {VALIDATION_ERROR} code, got {}",
            answer.body
        );
        assert!(
            message.starts_with("1 validation error detected:"),
            "case {case:?}: expected the validation layer's sentence, got {message:?}"
        );
        assert!(
            message.contains(&format!("at '{path}'")),
            "case {case:?}: the sentence must name the member {path:?}, got {message:?}"
        );
    }
}

/// The endpoint forms each protocol takes, from the `SubscribeInput$Endpoint` doc, with a
/// value that is NOT in that form: `(protocol, a good endpoint, a bad one)`.
///
/// Each bad value is one SNS can still READ as the kind of reference its protocol takes —
/// an ARN, for the protocols whose endpoint is one — so every row is refused by the
/// member-named sentence. A value SNS cannot read as an ARN at all is the other half of
/// the rule, and lives with it in `subscribe_endpoint_form_refusals`.
const ENDPOINTS: &[(&str, &str, &str)] = &[
    ("http", "http://example.test/hook", "ftp://example.test/hook"),
    ("https", "https://example.test/hook", "http://example.test/hook"),
    ("email", "ops@example.test", "ops@localhost"),
    ("email-json", "ops@example.test", "not-an-address"),
    ("sms", "+15550100", "not-a-number"),
    (
        "sqs",
        "arn:aws:sqs:us-east-1:000000000000:a-queue",
        "arn:aws:sns:us-east-1:000000000000:a-topic",
    ),
    (
        "application",
        "arn:aws:sns:us-east-1:000000000000:endpoint/GCM/an-app/22222222-2222-4222-8222-222222222222",
        "arn:aws:sqs:us-east-1:000000000000:a-queue",
    ),
    (
        "lambda",
        "arn:aws:lambda:us-east-1:000000000000:function:a-function",
        "arn:aws:lambda:us-east-1:000000000000:layer:a-layer",
    ),
    (
        "firehose",
        "arn:aws:firehose:us-east-1:000000000000:deliverystream/a-stream",
        "arn:aws:firehose:us-east-1:000000000000:stream/a-stream",
    ),
];

/// An `Endpoint` SNS reads as the kind of reference the requested `Protocol` takes and
/// does not accept is refused with the sentence naming the input MEMBER and nothing else —
/// not the protocol, not the form it wanted, and no part of the value — whatever protocol
/// was asked for; the endpoint each protocol does take is subscribed.
#[tokio::test]
async fn a_subscribe_endpoint_that_does_not_match_its_protocol_names_the_member() {
    let sns = Sns::new();
    let want = "Invalid parameter: Endpoint";

    for (index, (protocol, good, bad)) in ENDPOINTS.iter().enumerate() {
        let topic = sns.topic(&format!("endpoint-form-{index:02}")).await;
        for (case, endpoint) in [
            (format!("a {protocol} endpoint of the wrong form"), Some(*bad)),
            (format!("a {protocol} subscription with no endpoint"), None),
        ] {
            let mut params: Vec<(&str, &str)> =
                vec![("TopicArn", topic.as_str()), ("Protocol", protocol)];
            if let Some(endpoint) = endpoint {
                params.push(("Endpoint", endpoint));
            }
            let answer = sns.call("Subscribe", &params).await;
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

        // The control: the form that protocol does take is subscribed, so the refusal is
        // about the endpoint's form and not about the protocol. A `firehose`
        // subscription also needs the role SNS delivers through — "The following
        // attribute applies only to Amazon Data Firehose delivery stream subscriptions:
        // SubscriptionRoleArn" (`SubscribeInput$Attributes` doc) — which is a rule about
        // the attributes rather than the endpoint.
        let mut accepted_params: Vec<(&str, &str)> = vec![
            ("TopicArn", topic.as_str()),
            ("Protocol", protocol),
            ("Endpoint", good),
            ("ReturnSubscriptionArn", "true"),
        ];
        if *protocol == "firehose" {
            accepted_params.push(("Attributes.entry.1.key", "SubscriptionRoleArn"));
            accepted_params.push((
                "Attributes.entry.1.value",
                "arn:aws:iam::000000000000:role/sns-firehose",
            ));
        }
        let accepted = sns.call("Subscribe", &accepted_params).await;
        assert_eq!(
            accepted.status, 200,
            "case \"a {protocol} endpoint of the documented form\": expected 200, got {} / {}",
            accepted.status, accepted.body
        );
    }
}

/// An ARN-shaped `SubscriptionArn` that does not name a subscription is refused with the
/// sentence naming the part of the ARN at fault and nothing of the value, whichever call
/// took it — while a value that is not an ARN at all keeps the generic ARN-shape clause
/// every ARN member of this service shares.
#[tokio::test]
async fn a_subscription_arn_that_names_no_subscription_names_the_id() {
    let sns = Sns::new();
    let served = engine::emulator_account_id().to_string();
    let topic = sns.topic("subscription-reference-topic").await;
    let held = sns
        .call(
            "Subscribe",
            &[
                ("TopicArn", topic.as_str()),
                ("Protocol", "sqs"),
                ("Endpoint", "arn:aws:sqs:us-east-1:000000000000:reference-queue"),
                ("ReturnSubscriptionArn", "true"),
            ],
        )
        .await
        .element("SubscriptionArn")
        .expect("Subscribe answers the subscription's ARN");

    // ARN-shaped values whose subscription id is not one SNS could have minted.
    let not_subscriptions = [
        ("a topic ARN".to_string(), topic.clone()),
        (
            "a topic ARN with an empty id".to_string(),
            format!("{topic}:"),
        ),
        (
            "an id too many".to_string(),
            format!("{topic}:11111111:22222222"),
        ),
        (
            "a topic name that is no topic name".to_string(),
            format!("arn:aws:sns:{REGION}:{served}:two words:11111111"),
        ),
        (
            "another service".to_string(),
            format!("arn:aws:sqs:{REGION}:{served}:a-queue:11111111"),
        ),
        (
            "an empty region".to_string(),
            format!("arn:aws:sns::{served}:a-topic:11111111"),
        ),
    ];
    // Every operation that takes a `SubscriptionArn`.
    let actions: &[(&str, &[(&str, &str)])] = &[
        ("GetSubscriptionAttributes", &[]),
        ("Unsubscribe", &[]),
        (
            "SetSubscriptionAttributes",
            &[("AttributeName", "RawMessageDelivery"), ("AttributeValue", "true")],
        ),
    ];

    for (case, value) in &not_subscriptions {
        for (action, extra) in actions {
            let answer = call_with(&sns, action, "SubscriptionArn", value, extra).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?} on {action}: expected 400, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                INVALID_PARAMETER,
                "case {case:?} on {action}: expected the {INVALID_PARAMETER} code, got {}",
                answer.body
            );
            assert_eq!(
                answer.element("Message").as_deref(),
                Some(INVALID_SUBSCRIPTION_ID),
                "case {case:?} on {action}: expected {INVALID_SUBSCRIPTION_ID:?}, got {}",
                answer.body
            );
            assert!(
                !answer.body.contains(value.as_str()),
                "case {case:?} on {action}: the refusal must not reproduce the value it \
                 rejected, got {}",
                answer.body
            );
        }
    }

    // A value that is not an ARN keeps the shared ARN-shape clause, which names the
    // MEMBER and the constraint: the fault is the shape, not the subscription id.
    let answer = sns
        .call("GetSubscriptionAttributes", &[("SubscriptionArn", "not-an-arn")])
        .await;
    assert_eq!(
        answer.element("Message").as_deref(),
        Some("Invalid parameter: SubscriptionArn Reason: An ARN must have at least 6 elements, not 1"),
        "case \"not an ARN at all\": expected the ARN-shape clause, got {}",
        answer.body
    );

    // And the subscription that WAS minted still reads back, so the rule refuses only
    // what could not be a subscription ARN.
    let read = sns
        .call("GetSubscriptionAttributes", &[("SubscriptionArn", held.as_str())])
        .await;
    assert_eq!(
        read.status, 200,
        "case \"the subscription Subscribe minted\": expected 200, got {} / {}",
        read.status, read.body
    );
}

/// The three subscription counters a topic reports.
fn counters(answer: &Answer) -> (String, String, String) {
    let held = attribute_map(answer);
    let counter = |name: &str| held.get(name).cloned().unwrap_or_default();
    (
        counter("SubscriptionsConfirmed"),
        counter("SubscriptionsPending"),
        counter("SubscriptionsDeleted"),
    )
}

/// `Unsubscribe` removes the subscription at once — it stops being counted as confirmed
/// and stops being listed — while the topic's DELETED count is taken up later: a read
/// taken straight after the call reports the count from before it, and the deletions
/// become part of it when their instant arrives.
///
/// The rows differ in how many subscriptions the topic held and how many were removed,
/// which is what the rule generalises over; the recorded single removal is one row.
#[tokio::test]
async fn the_deleted_subscription_count_settles_after_the_unsubscribe() {
    // (case, subscriptions made, subscriptions removed)
    let rows: &[(&str, usize, usize)] = &[
        ("one subscription, removed", 1, 1),
        ("three subscriptions, two removed", 3, 2),
        ("two subscriptions, none removed", 2, 0),
    ];

    for (index, (case, made, removed)) in rows.iter().enumerate() {
        let sns = Sns::new();
        let topic = sns.topic(&format!("deleted-counter-{index:02}")).await;
        let mut arns: Vec<String> = Vec::new();
        for at in 0..*made {
            let endpoint = format!("arn:aws:sqs:us-east-1:000000000000:counted-{at:02}");
            let answer = sns
                .call(
                    "Subscribe",
                    &[
                        ("TopicArn", topic.as_str()),
                        ("Protocol", "sqs"),
                        ("Endpoint", endpoint.as_str()),
                        ("ReturnSubscriptionArn", "true"),
                    ],
                )
                .await;
            arns.push(
                answer
                    .element("SubscriptionArn")
                    .unwrap_or_else(|| panic!("case {case:?}: no SubscriptionArn in {}", answer.body)),
            );
        }
        for arn in arns.iter().take(*removed) {
            let answer = sns.call("Unsubscribe", &[("SubscriptionArn", arn.as_str())]).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: Unsubscribe expected 200, got {} / {}",
                answer.status, answer.body
            );
        }

        // Straight after the removals: the confirmed count has already dropped, and the
        // deleted count has not taken them up.
        let read = sns.call("GetTopicAttributes", &[("TopicArn", topic.as_str())]).await;
        assert_eq!(
            counters(&read),
            ((made - removed).to_string(), "0".to_string(), "0".to_string()),
            "case {case:?}: got {}",
            read.body
        );
        // Reading it again reports the same document: a read moves no counter.
        let again = sns.call("GetTopicAttributes", &[("TopicArn", topic.as_str())]).await;
        assert_eq!(
            counters(&again),
            counters(&read),
            "case {case:?}: a read must not change what the next read sees, got {}",
            again.body
        );

        // Once the instant each deletion is due at has passed, the count reports them —
        // the clock owes the change, so the due instants are moved into the past rather
        // than waited for.
        {
            let store = sns.state();
            let mut store = store.lock();
            let held = store
                .topics
                .get_mut(&topic)
                .unwrap_or_else(|| panic!("case {case:?}: the topic must be held"));
            let due = SystemTime::now() - Duration::from_secs(1);
            held.subscriptions_deleting = held.subscriptions_deleting.iter().map(|_| due).collect();
        }
        let settled = sns.call("GetTopicAttributes", &[("TopicArn", topic.as_str())]).await;
        assert_eq!(
            counters(&settled),
            (
                (made - removed).to_string(),
                "0".to_string(),
                removed.to_string()
            ),
            "case {case:?}: the deleted count must report the settled removals, got {}",
            settled.body
        );
    }
}

/// One topic holds the instants of at most [`MAX_SETTLING_DELETIONS_PER_TOPIC`] deletions
/// that have not been counted yet: this emulator's own ceiling on a store a caller grows
/// by unsubscribing. Past it the OLDEST deletions are taken up early, so the count a topic
/// reports is never lost — the ceiling costs it only the delay.
#[tokio::test]
async fn a_topics_uncounted_deletions_are_bounded() {
    let sns = Sns::new();
    let topic = sns.topic("deleted-counter-ceiling").await;
    let cycles = MAX_SETTLING_DELETIONS_PER_TOPIC + 8;
    for at in 0..cycles {
        let endpoint = format!("arn:aws:sqs:us-east-1:000000000000:bounded-{at:04}");
        let arn = sns
            .call(
                "Subscribe",
                &[
                    ("TopicArn", topic.as_str()),
                    ("Protocol", "sqs"),
                    ("Endpoint", endpoint.as_str()),
                    ("ReturnSubscriptionArn", "true"),
                ],
            )
            .await
            .element("SubscriptionArn")
            .unwrap_or_else(|| panic!("cycle {at}: Subscribe answers the subscription's ARN"));
        let removed = sns.call("Unsubscribe", &[("SubscriptionArn", arn.as_str())]).await;
        assert_eq!(
            removed.status, 200,
            "cycle {at}: Unsubscribe expected 200, got {} / {}",
            removed.status, removed.body
        );
    }

    let held = {
        let store = sns.state();
        let store = store.lock();
        store
            .topics
            .get(&topic)
            .map(|topic| topic.subscriptions_deleting.len())
            .unwrap_or_default()
    };
    assert!(
        held <= MAX_SETTLING_DELETIONS_PER_TOPIC,
        "a topic holds at most {MAX_SETTLING_DELETIONS_PER_TOPIC} uncounted deletions, got {held}"
    );
    // The deletions past the ceiling were taken up rather than dropped: the reported
    // count is the number of removals the ceiling displaced.
    let read = sns.call("GetTopicAttributes", &[("TopicArn", topic.as_str())]).await;
    assert_eq!(
        counters(&read),
        (
            "0".to_string(),
            "0".to_string(),
            (cycles - MAX_SETTLING_DELETIONS_PER_TOPIC).to_string()
        ),
        "the displaced deletions must be counted, got {}",
        read.body
    );
}

/// The account-wide subscription listing reports a `NextToken` exactly while
/// subscriptions remain to report: "Token to pass along to the next ListSubscriptions
/// request. This element is returned if there are more subscriptions to retrieve"
/// (`smithy_query(query="doc", shape="ListSubscriptionsResponse", member="NextToken")`).
///
/// The rows differ in how many subscriptions the account holds either side of the page
/// size, which is what the rule generalises over.
#[tokio::test]
async fn the_account_wide_listing_reports_a_token_exactly_while_subscriptions_remain() {
    // (case, subscriptions held, whether the first page carries a token)
    let rows: &[(&str, usize, bool)] = &[
        ("no subscriptions", 0, false),
        ("fewer than one page", 1, false),
        ("one page and one more", PAGE_SIZE + 1, true),
    ];

    for (index, (case, held, token_expected)) in rows.iter().enumerate() {
        let sns = Sns::new();
        let topic = sns.topic(&format!("token-rule-{index:02}")).await;
        for at in 0..*held {
            let endpoint = format!("arn:aws:sqs:us-east-1:000000000000:token-{at:04}");
            let answer = sns
                .call(
                    "Subscribe",
                    &[
                        ("TopicArn", topic.as_str()),
                        ("Protocol", "sqs"),
                        ("Endpoint", endpoint.as_str()),
                    ],
                )
                .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: Subscribe expected 200, got {} / {}",
                answer.status, answer.body
            );
        }

        let first = sns.call("ListSubscriptions", &[]).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: expected 200, got {} / {}",
            first.status, first.body
        );
        let reported = first.elements("SubscriptionArn").len();
        assert_eq!(
            reported,
            (*held).min(PAGE_SIZE),
            "case {case:?}: expected a page of {}, got {reported}",
            (*held).min(PAGE_SIZE)
        );
        let token = first.element("NextToken");
        assert_eq!(
            token.is_some(),
            *token_expected,
            "case {case:?}: a token comes back exactly while subscriptions remain, got {}",
            first.body
        );

        // The token it does mint reports the remainder, and that page carries none.
        if let Some(token) = token {
            let second = sns
                .call("ListSubscriptions", &[("NextToken", token.as_str())])
                .await;
            assert_eq!(
                second.elements("SubscriptionArn").len(),
                held - PAGE_SIZE,
                "case {case:?}: the second page reports the remainder, got {}",
                second.body
            );
            assert!(
                second.element("NextToken").is_none(),
                "case {case:?}: the last page carries no token, got {}",
                second.body
            );
        }
    }
}
