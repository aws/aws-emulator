// SPDX-License-Identifier: CC0-1.0

//! A topic-policy statement decides a call only where its `Condition` holds for that call, end
//! to end over the wire through the real `MultiServiceRouter` with IAM and STS mounted on the
//! same engine — so the grant and the refusal are observed where a client observes them: in the
//! answer to the signed call.
//!
//! The RULE asserted here: every statement is weighed under the condition keys the request
//! ANSWERS FOR — the principal that signed it (`aws:PrincipalArn`, which for a role session is
//! "the ARN of the role, not the ARN of the user that assumed the role", and
//! `aws:PrincipalAccount`) and the subscription the request itself names (`sns:Protocol` and
//! `sns:Endpoint`, "the protocol value … from a Subscribe request or a previously confirmed
//! subscription"). A conditioned statement therefore applies to exactly the requests its
//! condition covers: a grant admits those and no others, and a `Deny` refuses those and no
//! others, each with the modelled `AuthorizationErrorException` (HTTP 403, `awsQueryError` code
//! `AuthorizationError`). A key the request carries NO value for is decided as absent rather
//! than left undecided, which is what IAM's `IfExists` suffix, its negated operators and its
//! `Null` operator are defined over.
//!
//! The rows differ along what the rule generalises over: WHICH key the condition is written on,
//! which OPERATOR compares it, whether the request's value matches, which principal the
//! statement names (the caller by name, or every principal), which effect the statement states,
//! and whether the operation the call performs carries a value for the key at all.
//!
//! The refusal SENTENCES are the sibling suite's, so nothing of IAM's wording is spelled twice
//! ([`crate::topic_policy_authorization`]).

use serde_json::{json, Value};

use crate::publish_authorization::{
    emulator, given_assumed_role, policy, sns_as, Emulator, AMBIENT_KEY,
};
use crate::topic_policy_authorization::{
    no_identity_policy_allows, not_authorized, AUTHORIZATION_ERROR, RESOURCE_POLICY_DENY,
};
use crate::topics_subscriptions_publish::text_of;

/// The IAM action `Subscribe` performs, as a policy and a refusal spell it.
const SUBSCRIBE_ACTION: &str = "SNS:Subscribe";

/// The IAM action `Publish` performs.
const PUBLISH_ACTION: &str = "SNS:Publish";

/// An account that is not this one, for the rows whose condition names somebody else's.
const ANOTHER_ACCOUNT: &str = "210987654321";

/// The two endpoints the `sns:Endpoint` rows tell apart, in the form the `https` protocol
/// addresses its subscriber in.
const ONE_ENDPOINT: &str = "https://example.com/one";
const OTHER_ENDPOINT: &str = "https://example.com/two";

/// Whether a row's call is served or refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    Served,
    Refused,
}

/// One statement of a topic's access policy: `effect` for `principal` on `action` of the topic,
/// under `condition`.
fn conditioned(
    effect: &str,
    principal: Value,
    action: &str,
    topic: &str,
    condition: Option<Value>,
) -> String {
    let mut statement = json!({
        "Sid": "TheRowsStatement",
        "Effect": effect,
        "Principal": principal,
        "Action": action,
        "Resource": topic,
    });
    if let Some(condition) = condition {
        statement
            .as_object_mut()
            .expect("the statement being built is a JSON object")
            .insert("Condition".to_string(), condition);
    }
    policy(statement)
}

/// A fresh topic carrying `document` as its access policy: its ARN.
///
/// One topic per row, written by the account's own credential: a row's document decides that
/// row's topic alone.
async fn given_topic(emulator: &Emulator, name: &str) -> String {
    let (status, body) = sns_as(emulator, AMBIENT_KEY, "CreateTopic", &[("Name", name)]).await;
    assert_eq!(
        status, 200,
        "the fixture CreateTopic {name} must succeed, got {status} / {body}"
    );
    text_of(&body, "TopicArn")
}

/// Write `document` as the topic's access policy, as its owner does.
async fn set_policy(emulator: &Emulator, topic: &str, document: &str) {
    let (status, body) = sns_as(
        emulator,
        AMBIENT_KEY,
        "SetTopicAttributes",
        &[
            ("TopicArn", topic),
            ("AttributeName", "Policy"),
            ("AttributeValue", document),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture SetTopicAttributes must succeed, got {status} / {body}"
    );
}

/// The awsQuery error code of a refusal, or `""` on a success envelope.
fn code_of(body: &str) -> String {
    if body.contains("<Code>") {
        text_of(body, "Code")
    } else {
        String::new()
    }
}

/// The message of a refusal, or `""` on a success envelope.
fn message_of(body: &str) -> String {
    if body.contains("<Message>") {
        text_of(body, "Message")
    } else {
        String::new()
    }
}

/// One row of the conditioned-grant table: the case, the principal the statement names, its
/// condition, the protocol and the endpoint the subscribe asks for, and the answer expected.
type GrantRow<'a> = (
    &'a str,
    Value,
    Option<Value>,
    &'a str,
    &'a str,
    Expected,
);

#[tokio::test]
async fn a_conditioned_topic_grant_admits_exactly_the_subscribes_it_covers() {
    let emulator = emulator();
    let account = engine::emulator_account_id().to_string();
    // A role of a known family, so a row may name it by an ARN PATTERN: the condition key
    // answers with the ROLE's ARN, not with the session's.
    let (role_arn, session_arn, access_key_id) =
        given_assumed_role(&emulator, "app-pub-a").await;
    let role_family = format!("arn:aws:iam::{account}:role/app-*");
    let other_family = format!("arn:aws:iam::{account}:role/ops-*");
    let by_name = json!({"AWS": role_arn.clone()});
    let everyone = json!("*");

    // (case, the principal the statement names, its condition, the protocol and endpoint the
    //  subscribe asks for, what the call must answer)
    let rows: Vec<GrantRow<'_>> = vec![
        (
            "no condition at all, so the grant covers every subscribe it names",
            by_name.clone(),
            None,
            "https",
            ONE_ENDPOINT,
            Expected::Served,
        ),
        (
            "StringEquals on the protocol the subscribe asks for",
            by_name.clone(),
            Some(json!({"StringEquals": {"sns:Protocol": "https"}})),
            "https",
            ONE_ENDPOINT,
            Expected::Served,
        ),
        (
            "StringEquals on another protocol than the subscribe asks for",
            by_name.clone(),
            Some(json!({"StringEquals": {"sns:Protocol": "https"}})),
            "email",
            "ops@example.com",
            Expected::Refused,
        ),
        (
            "StringNotEquals excluding a protocol the subscribe does not ask for",
            by_name.clone(),
            Some(json!({"StringNotEquals": {"sns:Protocol": "email"}})),
            "https",
            ONE_ENDPOINT,
            Expected::Served,
        ),
        (
            "StringNotEquals excluding the protocol the subscribe asks for",
            by_name.clone(),
            Some(json!({"StringNotEquals": {"sns:Protocol": "email"}})),
            "email",
            "ops@example.com",
            Expected::Refused,
        ),
        (
            "StringEquals naming one of two protocols, asked for the other",
            by_name.clone(),
            Some(json!({"StringEquals": {"sns:Protocol": ["https", "sqs"]}})),
            "email",
            "ops@example.com",
            Expected::Refused,
        ),
        (
            "StringEquals on the endpoint the subscribe names",
            by_name.clone(),
            Some(json!({"StringEquals": {"sns:Endpoint": ONE_ENDPOINT}})),
            "https",
            ONE_ENDPOINT,
            Expected::Served,
        ),
        (
            "StringEquals on another endpoint than the subscribe names",
            by_name.clone(),
            Some(json!({"StringEquals": {"sns:Endpoint": ONE_ENDPOINT}})),
            "https",
            OTHER_ENDPOINT,
            Expected::Refused,
        ),
        (
            "StringLike on a pattern the endpoint matches",
            by_name.clone(),
            Some(json!({"StringLike": {"sns:Endpoint": "https://example.com/*"}})),
            "https",
            OTHER_ENDPOINT,
            Expected::Served,
        ),
        (
            "StringLike on a pattern the endpoint does not match",
            by_name.clone(),
            Some(json!({"StringLike": {"sns:Endpoint": "https://elsewhere.example/*"}})),
            "https",
            ONE_ENDPOINT,
            Expected::Refused,
        ),
        (
            "ArnLike on a principal-ARN pattern the caller's role matches",
            everyone.clone(),
            Some(json!({"ArnLike": {"aws:PrincipalArn": role_family.clone()}})),
            "https",
            ONE_ENDPOINT,
            Expected::Served,
        ),
        (
            "ArnLike on a principal-ARN pattern the caller's role does not match",
            everyone.clone(),
            Some(json!({"ArnLike": {"aws:PrincipalArn": other_family.clone()}})),
            "https",
            ONE_ENDPOINT,
            Expected::Refused,
        ),
        (
            "StringEquals on the ARN of the role the caller is a session of",
            everyone.clone(),
            Some(json!({"StringEquals": {"aws:PrincipalArn": role_arn.clone()}})),
            "https",
            ONE_ENDPOINT,
            Expected::Served,
        ),
        (
            "StringEquals on the session ARN, which is not what the key answers",
            everyone.clone(),
            Some(json!({"StringEquals": {"aws:PrincipalArn": session_arn.clone()}})),
            "https",
            ONE_ENDPOINT,
            Expected::Refused,
        ),
        (
            "StringEquals on the account the caller's principal belongs to",
            everyone.clone(),
            Some(json!({"StringEquals": {"aws:PrincipalAccount": account.clone()}})),
            "https",
            ONE_ENDPOINT,
            Expected::Served,
        ),
        (
            "StringEquals on another account than the caller's principal belongs to",
            everyone.clone(),
            Some(json!({"StringEquals": {"aws:PrincipalAccount": ANOTHER_ACCOUNT}})),
            "https",
            ONE_ENDPOINT,
            Expected::Refused,
        ),
        (
            "the IfExists spelling over a key the subscribe DOES carry a value for",
            by_name.clone(),
            Some(json!({"StringEqualsIfExists": {"sns:Protocol": "https"}})),
            "email",
            "ops@example.com",
            Expected::Refused,
        ),
        (
            "Null asserting the endpoint key is present, which a subscribe carries",
            by_name.clone(),
            Some(json!({"Null": {"sns:Endpoint": "false"}})),
            "https",
            ONE_ENDPOINT,
            Expected::Served,
        ),
        (
            "two conditions of which the subscribe satisfies only one",
            by_name.clone(),
            Some(json!({"StringEquals": {
                "sns:Protocol": "https",
                "sns:Endpoint": OTHER_ENDPOINT,
            }})),
            "https",
            ONE_ENDPOINT,
            Expected::Refused,
        ),
    ];

    for (index, (case, principal, condition, protocol, endpoint, expected)) in
        rows.iter().enumerate()
    {
        let topic = given_topic(&emulator, &format!("conditioned-grant-{index:02}")).await;
        set_policy(
            &emulator,
            &topic,
            &conditioned(
                "Allow",
                principal.clone(),
                SUBSCRIBE_ACTION,
                &topic,
                condition.clone(),
            ),
        )
        .await;

        let (status, body) = sns_as(
            &emulator,
            &access_key_id,
            "Subscribe",
            &[
                ("TopicArn", &topic),
                ("Protocol", protocol),
                ("Endpoint", endpoint),
            ],
        )
        .await;
        match expected {
            Expected::Served => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the subscribe to be served, got {status} / {body}"
                );
                // Read back through the listing: a served subscribe left a subscription of the
                // endpoint it named on the topic.
                let (listed_status, listed) = sns_as(
                    &emulator,
                    AMBIENT_KEY,
                    "ListSubscriptionsByTopic",
                    &[("TopicArn", &topic)],
                )
                .await;
                assert_eq!(
                    listed_status, 200,
                    "case {case:?}: the owner's listing must succeed, got {listed_status} / \
                     {listed}"
                );
                assert!(
                    listed.contains(endpoint),
                    "case {case:?}: expected the listing to hold the subscribed endpoint \
                     {endpoint:?}, got {listed}"
                );
            }
            Expected::Refused => {
                assert_eq!(
                    (status, code_of(&body).as_str()),
                    AUTHORIZATION_ERROR,
                    "case {case:?}: expected the subscribe to be refused, got {status} / {body}"
                );
                assert_eq!(
                    message_of(&body),
                    not_authorized(
                        &session_arn,
                        SUBSCRIBE_ACTION,
                        &topic,
                        &no_identity_policy_allows(SUBSCRIBE_ACTION)
                    ),
                    "case {case:?}: unexpected refusal sentence, got {body}"
                );
                // Nothing was created: the decision is taken before the subscription is.
                let (_, listed) = sns_as(
                    &emulator,
                    AMBIENT_KEY,
                    "ListSubscriptionsByTopic",
                    &[("TopicArn", &topic)],
                )
                .await;
                assert!(
                    !listed.contains(endpoint),
                    "case {case:?}: a refused subscribe must create nothing, got {listed}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_key_the_call_carries_no_value_for_is_decided_as_absent() {
    let emulator = emulator();
    // A publish names no subscription, so it carries no value for either of this service's own
    // condition keys — the case IAM's absence spellings are defined over.
    let (role_arn, session_arn, access_key_id) =
        given_assumed_role(&emulator, "app-publisher").await;
    let by_name = json!({"AWS": role_arn});

    // (case, the condition the grant carries, what the publish must answer)
    let rows: Vec<(&str, Value, Expected)> = vec![
        (
            "StringEquals over the absent key, which compares against nothing",
            json!({"StringEquals": {"sns:Protocol": "https"}}),
            Expected::Refused,
        ),
        (
            "the IfExists spelling, which holds where the key is not present",
            json!({"StringEqualsIfExists": {"sns:Protocol": "https"}}),
            Expected::Served,
        ),
        (
            "a negated operator, which returns true where the key is not present",
            json!({"StringNotEquals": {"sns:Protocol": "https"}}),
            Expected::Served,
        ),
        (
            "Null asserting the key is absent, which it is",
            json!({"Null": {"sns:Protocol": "true"}}),
            Expected::Served,
        ),
        (
            "Null asserting the key is present, which it is not",
            json!({"Null": {"sns:Protocol": "false"}}),
            Expected::Refused,
        ),
    ];

    for (index, (case, condition, expected)) in rows.iter().enumerate() {
        let topic = given_topic(&emulator, &format!("absent-key-{index:02}")).await;
        set_policy(
            &emulator,
            &topic,
            &conditioned(
                "Allow",
                by_name.clone(),
                PUBLISH_ACTION,
                &topic,
                Some(condition.clone()),
            ),
        )
        .await;

        let (status, body) = sns_as(
            &emulator,
            &access_key_id,
            "Publish",
            &[("TopicArn", &topic), ("Message", "a message")],
        )
        .await;
        match expected {
            Expected::Served => assert_eq!(
                status, 200,
                "case {case:?}: expected the publish to be served, got {status} / {body}"
            ),
            Expected::Refused => {
                assert_eq!(
                    (status, code_of(&body).as_str()),
                    AUTHORIZATION_ERROR,
                    "case {case:?}: expected the publish to be refused, got {status} / {body}"
                );
                assert_eq!(
                    message_of(&body),
                    not_authorized(
                        &session_arn,
                        PUBLISH_ACTION,
                        &topic,
                        &no_identity_policy_allows(PUBLISH_ACTION)
                    ),
                    "case {case:?}: unexpected refusal sentence, got {body}"
                );
            }
        }
    }
}

#[tokio::test]
async fn an_explicit_deny_of_every_principal_refuses_the_accounts_own_credential() {
    let emulator = emulator();
    let account = engine::emulator_account_id().to_string();
    let account_root = json!({"AWS": format!("arn:aws:iam::{account}:root")});
    let everyone = json!("*");

    // The account's own published credential holds no identity-based policy and needs no
    // grant, so the only statement that refuses it is an explicit `Deny` naming EVERY
    // principal — a statement about every caller there is, which an identity of this account
    // is not. Each row is that statement, the call it is weighed against, and the answer.
    //
    // (case, the deny's principal, its condition, the operation, its parameters, the answer)
    type Call<'a> = (&'a str, Vec<(&'a str, &'a str)>);
    let subscribe_https: Call = (
        "Subscribe",
        vec![("Protocol", "https"), ("Endpoint", ONE_ENDPOINT)],
    );
    let subscribe_email: Call = (
        "Subscribe",
        vec![("Protocol", "email"), ("Endpoint", "ops@example.com")],
    );
    let publish: Call = ("Publish", vec![("Message", "a message")]);
    let rows: Vec<(&str, Value, Option<Value>, &Call, Expected)> = vec![
        (
            "a deny of every principal, over the publish it names",
            everyone.clone(),
            None,
            &publish,
            Expected::Refused,
        ),
        (
            "a deny naming the account's own identities",
            account_root.clone(),
            None,
            &publish,
            Expected::Served,
        ),
        (
            "a deny of every principal conditioned on the protocol the subscribe asks for",
            everyone.clone(),
            Some(json!({"StringEquals": {"sns:Protocol": "https"}})),
            &subscribe_https,
            Expected::Refused,
        ),
        (
            "the same deny, over a subscribe asking for another protocol",
            everyone.clone(),
            Some(json!({"StringEquals": {"sns:Protocol": "https"}})),
            &subscribe_email,
            Expected::Served,
        ),
        (
            "a deny of every protocol but one, over a subscribe asking for another",
            everyone.clone(),
            Some(json!({"StringNotEquals": {"sns:Protocol": "https"}})),
            &subscribe_email,
            Expected::Refused,
        ),
        (
            "a deny of every protocol but one, over a subscribe asking for that one",
            everyone.clone(),
            Some(json!({"StringNotEquals": {"sns:Protocol": "https"}})),
            &subscribe_https,
            Expected::Served,
        ),
    ];

    for (index, (case, principal, condition, call, expected)) in rows.iter().enumerate() {
        let topic = given_topic(&emulator, &format!("owner-deny-{index:02}")).await;
        let (operation, extra) = call;
        let action = if *operation == "Publish" {
            PUBLISH_ACTION
        } else {
            SUBSCRIBE_ACTION
        };
        set_policy(
            &emulator,
            &topic,
            &conditioned("Deny", principal.clone(), action, &topic, condition.clone()),
        )
        .await;

        let mut params: Vec<(&str, &str)> = vec![("TopicArn", &topic)];
        params.extend(extra.iter().copied());
        let (status, body) = sns_as(&emulator, AMBIENT_KEY, operation, &params).await;
        match expected {
            Expected::Served => assert_eq!(
                status, 200,
                "case {case:?}: expected {operation} to be served, got {status} / {body}"
            ),
            Expected::Refused => {
                assert_eq!(
                    (status, code_of(&body).as_str()),
                    AUTHORIZATION_ERROR,
                    "case {case:?}: expected {operation} to be refused, got {status} / {body}"
                );
                assert!(
                    message_of(&body).ends_with(RESOURCE_POLICY_DENY),
                    "case {case:?}: expected the refusal to name the document that denied it, \
                     got {body}"
                );
            }
        }
    }
}
