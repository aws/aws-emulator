// SPDX-License-Identifier: CC0-1.0

//! A publish to a topic encrypted with a customer managed KMS key asks KMS for the data key
//! as the message is published, driven over the wire through the real `MultiServiceRouter`
//! with KMS mounted on the same engine — so what a client sees is what this suite asserts.
//!
//! The RULE asserted here: a topic carrying `KmsMasterKeyId` has every message published to it
//! encrypted under that key, and Amazon SNS asks KMS for that data key on each publish. So the
//! KEY's own condition at the moment of the publish decides the call: a key KMS can use seals
//! the message and the call answers with a message id, while a key whose state admits no
//! cryptographic operation seals nothing and the call is refused in the class KMS answered for
//! that state — `KMSDisabled` for a key that is merely disabled, which a client enables again,
//! and `KMSInvalidState` for every other state that admits none. The condition is read at
//! publish time rather than at configure time, so disabling the key stops the topic taking
//! traffic and enabling it again restores it, with nothing written to the topic in between.
//! An identifier that resolves to no key record attributes no condition to a key, so it
//! refuses nothing: the publish is refused only on what a key record says.
//!
//! The rows differ along what the rule generalises over: the FORM the topic names its key in (a
//! key id, a key ARN, an alias, an alias ARN), the key's condition (enabled, disabled, awaiting
//! deletion, awaiting imported key material, Amazon Web Services managed, named by an
//! identifier that resolves to no key record), and which operation does the publishing
//! (`Publish` and `PublishBatch`, both of which write a message to the topic and both of which
//! model the `KMS*` family).

use serde_json::json;

use crate::sqs_subscription_delivery::{
    drain, element, emulator, given_queue, kms, sns, sns_call, Emulator,
};
use crate::topics_subscriptions_publish::xml_unescape;

/// The Region every call in this suite is signed for, which is the Region the keys and the
/// topics are minted in.
const REGION: &str = "us-east-1";

/// The predefined Amazon Web Services alias of the Amazon Web Services managed key this
/// service encrypts with — the one an account that asks for SSE without naming a key of its
/// own is put on.
const AWS_MANAGED_ALIAS: &str = "alias/aws/sns";

/// A key this emulator's KMS minted: its key id and its key ARN, the two forms a topic can
/// name a key by directly.
pub(crate) async fn given_key(emulator: &Emulator) -> (String, String) {
    let created = kms(emulator, "CreateKey", &json!({})).await;
    let metadata = &created["KeyMetadata"];
    let key_id = metadata["KeyId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateKey: no KeyMetadata.KeyId in {created}"))
        .to_string();
    let key_arn = metadata["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateKey: no KeyMetadata.Arn in {created}"))
        .to_string();
    (key_id, key_arn)
}

/// The account segment of an ARN this emulator minted.
fn account_of(arn: &str) -> String {
    arn.split(':')
        .nth(4)
        .unwrap_or_else(|| panic!("no account in {arn}"))
        .to_string()
}

/// A topic called `name` whose messages are encrypted under the key `master_key_id` names, or
/// — for a row that names none — a topic on no customer managed key at all: its ARN.
pub(crate) async fn given_topic(
    emulator: &Emulator,
    name: &str,
    master_key_id: Option<&str>,
) -> String {
    let mut params: Vec<(&str, &str)> = vec![("Name", name)];
    if let Some(key) = master_key_id {
        params.push(("Attributes.entry.1.key", "KmsMasterKeyId"));
        params.push(("Attributes.entry.1.value", key));
    }
    element(&sns(emulator, "CreateTopic", &params).await, "TopicArn")
}

/// What one `Publish` answered: its HTTP status, the awsQuery error code (`""` on a success
/// envelope) and the sentence the answer carries — the message id of an accepted publish, or
/// the message of a refused one, with the envelope's XML entities decoded.
async fn publish(emulator: &Emulator, topic: &str, body: &str) -> (u16, String, String) {
    let (status, text) = sns_call(
        emulator,
        "Publish",
        &[("TopicArn", topic), ("Message", body)],
    )
    .await;
    let tag = if status == 200 { "MessageId" } else { "Code" };
    let code = if status == 200 {
        String::new()
    } else {
        xml_unescape(&element(&text, "Code"))
    };
    let sentence = match status {
        200 => element(&text, tag),
        _ => xml_unescape(&element(&text, "Message")),
    };
    (status, code, sentence)
}

/// Disable or enable the key `key_id` names, as a client of KMS does.
pub(crate) async fn set_key_enabled(emulator: &Emulator, key_id: &str, enabled: bool) {
    let operation = if enabled { "EnableKey" } else { "DisableKey" };
    kms(emulator, operation, &json!({"KeyId": key_id})).await;
    let described = kms(emulator, "DescribeKey", &json!({"KeyId": key_id})).await;
    let expected = if enabled { "Enabled" } else { "Disabled" };
    assert_eq!(
        described["KeyMetadata"]["KeyState"].as_str(),
        Some(expected),
        "{operation} {key_id}: expected KeyState {expected:?}, got {described}"
    );
}

/// The metadata block an AWS error carries, which a relayed KMS refusal ends with. The request
/// id inside it is minted per refused call, so the tail is what every relay shares.
pub(crate) const RELAY_TAIL: &str = "; Proxy: null)";

/// Assert that `message` is KMS's own sentence `expected`, relayed with the metadata block of
/// the KMS call that failed, reporting `kms_code`.
pub(crate) fn assert_relayed(case: &str, message: &str, expected: &str, kms_code: &str) {
    assert!(
        message.starts_with(expected),
        "case {case:?}: expected the message to open with KMS's own sentence {expected:?}, got \
         {message:?}"
    );
    assert!(
        message.contains(&format!("Error Code: {kms_code}")),
        "case {case:?}: expected the relayed KMS error code {kms_code:?}, got {message:?}"
    );
    assert!(
        message.ends_with(RELAY_TAIL),
        "case {case:?}: expected the relayed metadata block to end with {RELAY_TAIL:?}, got \
         {message:?}"
    );
}

#[tokio::test]
async fn a_publish_follows_the_key_state_however_the_topic_names_its_key() {
    let emulator = emulator();

    // The FORM the topic names its key in — a key id, a key ARN, an alias and an alias ARN are
    // the four spellings a `KmsMasterKeyId` can carry, and KMS resolves each to the same key.
    let forms: &[&str] = &[
        "the key id",
        "the key ARN",
        // An alias is the form the recorded probe named its key by.
        "an alias name",
        "an alias ARN",
    ];

    for (index, case) in forms.iter().enumerate() {
        let (key_id, key_arn) = given_key(&emulator).await;
        let alias = format!("alias/topic-key-{index}");
        kms(
            &emulator,
            "CreateAlias",
            &json!({"AliasName": alias, "TargetKeyId": key_id}),
        )
        .await;
        let named = match *case {
            "the key id" => key_id.clone(),
            "the key ARN" => key_arn.clone(),
            "an alias name" => alias.clone(),
            "an alias ARN" => format!("arn:aws:kms:{REGION}:{}:{alias}", account_of(&key_arn)),
            other => panic!("no identifier form for {other:?}"),
        };
        let topic = given_topic(&emulator, &format!("sse-kms-{index}"), Some(&named)).await;

        // The control, before anything is broken: the key is enabled, so the publish is
        // accepted and answers with a message id.
        let (status, code, message_id) = publish(&emulator, &topic, "while enabled").await;
        assert_eq!(
            (status, code.as_str()),
            (200, ""),
            "case {case:?}: a publish under an enabled key must be accepted, got \
             {status} / {message_id:?}"
        );
        assert!(
            !message_id.is_empty(),
            "case {case:?}: an accepted publish must answer with a MessageId, got \
             {message_id:?}"
        );

        set_key_enabled(&emulator, &key_id, false).await;
        let (status, code, message) = publish(&emulator, &topic, "while disabled").await;
        assert_eq!(
            (status, code.as_str()),
            (400, "KMSDisabled"),
            "case {case:?}: a publish under a disabled key must be refused KMSDisabled, got \
             {status} / {code} / {message:?}"
        );
        assert_relayed(
            case,
            &message,
            &format!("{key_arn} is disabled."),
            "DisabledException",
        );

        // Enabling the key again restores the topic: the refusal was the key's condition at
        // the moment of the publish, not a state the topic was left in.
        set_key_enabled(&emulator, &key_id, true).await;
        let (status, code, message_id) = publish(&emulator, &topic, "after enabling").await;
        assert_eq!(
            (status, code.as_str()),
            (200, ""),
            "case {case:?}: a publish under a re-enabled key must be accepted again, got \
             {status} / {message_id:?}"
        );
    }
}

/// Which class a publish is refused in, over the conditions a topic's key can be in — and
/// which conditions refuse nothing at all.
#[tokio::test]
async fn the_class_a_publish_is_refused_in_follows_the_condition_of_the_topics_key() {
    let emulator = emulator();
    let (usable_key_id, usable_key_arn) = given_key(&emulator).await;
    let account = account_of(&usable_key_arn);

    // A key whose deletion is scheduled is in a state no cryptographic operation can use,
    // which is a different class from a key that is merely disabled.
    let (awaiting_deletion, awaiting_deletion_arn) = given_key(&emulator).await;
    kms(
        &emulator,
        "ScheduleKeyDeletion",
        &json!({"KeyId": awaiting_deletion, "PendingWindowInDays": 7}),
    )
    .await;

    // A key created with no key material of its own is another state no cryptographic
    // operation can use: "To create a KMS key with no key material (for imported key
    // material), set this value to EXTERNAL"
    // (`smithy_query(query="doc",shape="CreateKeyRequest",member="Origin")`).
    let awaiting_material = kms(
        &emulator,
        "CreateKey",
        &json!({"Origin": "EXTERNAL", "KeySpec": "SYMMETRIC_DEFAULT"}),
    )
    .await;
    let awaiting_material_arn = awaiting_material["KeyMetadata"]["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateKey EXTERNAL: no Arn in {awaiting_material}"))
        .to_string();

    // A disabled key, for the row that pins which class a disabled key is refused in beside
    // the states that are not merely disabled.
    let (disabled_key_id, disabled_key_arn) = given_key(&emulator).await;
    kms(&emulator, "DisableKey", &json!({"KeyId": disabled_key_id})).await;

    // An identifier that resolves to no key record attributes no condition to a key: a
    // well-formed key id of a key this account never had, an alias nobody created, a key ARN
    // of another Region's endpoint, and an ARN whose account segment is not an account at all.
    let absent = "00000000-0000-0000-0000-000000000000";
    let accountless = format!("arn:aws:kms:{REGION}:not-an-account:key/{usable_key_id}");

    // (case, the `KmsMasterKeyId` the topic carries, the awsQuery code the publish must be
    //  refused with — `None` for a publish that must be accepted)
    let rows: Vec<(&str, Option<String>, Option<&str>)> = vec![
        ("no customer managed key at all", None, None),
        (
            "an enabled customer managed key",
            Some(usable_key_arn.clone()),
            None,
        ),
        (
            "the predefined Amazon Web Services alias of this service",
            Some(AWS_MANAGED_ALIAS.to_string()),
            None,
        ),
        (
            "a key id that names no key",
            Some(absent.to_string()),
            None,
        ),
        (
            "an alias that names no alias",
            Some("alias/never-created".to_string()),
            None,
        ),
        (
            "a key ARN of another Region",
            Some(format!("arn:aws:kms:eu-west-1:{account}:key/{usable_key_id}")),
            None,
        ),
        (
            "an ARN whose account segment names no account",
            Some(accountless),
            None,
        ),
        (
            "a disabled key",
            Some(disabled_key_arn.clone()),
            Some("KMSDisabled"),
        ),
        (
            "a key whose deletion is scheduled",
            Some(awaiting_deletion_arn.clone()),
            Some("KMSInvalidState"),
        ),
        (
            "a key still awaiting its imported key material",
            Some(awaiting_material_arn.clone()),
            Some("KMSInvalidState"),
        ),
    ];

    for (index, (case, master_key_id, refused_with)) in rows.iter().enumerate() {
        let topic = given_topic(
            &emulator,
            &format!("key-condition-{index}"),
            master_key_id.as_deref(),
        )
        .await;
        let (status, code, sentence) = publish(&emulator, &topic, "sealed").await;
        match refused_with {
            None => {
                assert_eq!(
                    (status, code.as_str()),
                    (200, ""),
                    "case {case:?}: the publish must be accepted, got {status} / {code} / \
                     {sentence:?}"
                );
                assert!(
                    !sentence.is_empty(),
                    "case {case:?}: an accepted publish must answer with a MessageId, got \
                     {sentence:?}"
                );
            }
            Some(expected) => {
                assert_eq!(
                    (status, code.as_str()),
                    (400, *expected),
                    "case {case:?}: expected the publish refused {expected}, got {status} / \
                     {code} / {sentence:?}"
                );
                assert!(
                    sentence.ends_with(RELAY_TAIL),
                    "case {case:?}: the refusal relays the KMS call that failed, so it must \
                     end with {RELAY_TAIL:?}, got {sentence:?}"
                );
            }
        }
    }

    // The sentence of the key-state refusal is KMS's own for the state the key is in, which
    // names the key by ARN — so a client learns which key refused and whether its state can
    // change.
    let topic = given_topic(&emulator, "awaiting-deletion", Some(&awaiting_deletion_arn)).await;
    let (_, _, message) = publish(&emulator, &topic, "sealed").await;
    assert_relayed(
        "a key whose deletion is scheduled",
        &message,
        &format!("{awaiting_deletion_arn} is pending deletion."),
        "KMSInvalidStateException",
    );
}

#[tokio::test]
async fn a_refused_publish_writes_nothing_to_the_encrypted_topic() {
    let emulator = emulator();
    let (key_id, key_arn) = given_key(&emulator).await;
    let topic = given_topic(&emulator, "nothing-written", Some(&key_arn)).await;
    let (url, queue_arn) = given_queue(&emulator, "encrypted-topic-subscriber").await;
    sns(
        &emulator,
        "Subscribe",
        &[
            ("TopicArn", topic.as_str()),
            ("Protocol", "sqs"),
            ("Endpoint", queue_arn.as_str()),
            ("Attributes.entry.1.key", "RawMessageDelivery"),
            ("Attributes.entry.1.value", "true"),
        ],
    )
    .await;

    let (status, _, _) = publish(&emulator, &topic, "before").await;
    assert_eq!(status, 200, "the control publish must be accepted");
    assert_eq!(
        drain(&emulator, &url)
            .await
            .iter()
            .map(|(body, _)| body.clone())
            .collect::<Vec<_>>(),
        vec!["before".to_string()],
        "the subscriber must receive the message published while the key was enabled"
    );

    set_key_enabled(&emulator, &key_id, false).await;
    let (status, code, _) = publish(&emulator, &topic, "while disabled").await;
    assert_eq!(
        (status, code.as_str()),
        (400, "KMSDisabled"),
        "a publish under a disabled key must be refused"
    );
    let delivered = drain(&emulator, &url).await;
    assert!(
        delivered.is_empty(),
        "a refused publish seals and delivers nothing, so the subscriber must stay empty, got \
         {delivered:?}"
    );

    set_key_enabled(&emulator, &key_id, true).await;
    let (status, _, _) = publish(&emulator, &topic, "after").await;
    assert_eq!(status, 200, "the publish after enabling must be accepted");
    assert_eq!(
        drain(&emulator, &url)
            .await
            .iter()
            .map(|(body, _)| body.clone())
            .collect::<Vec<_>>(),
        vec!["after".to_string()],
        "only the message published after the key was enabled again may arrive"
    );
}

#[tokio::test]
async fn a_batch_publish_to_an_encrypted_topic_is_judged_by_the_same_key_condition() {
    let emulator = emulator();
    let (key_id, key_arn) = given_key(&emulator).await;
    let topic = given_topic(&emulator, "batched-sse-kms", Some(&key_arn)).await;

    let entries: &[(&str, &str)] = &[
        ("PublishBatchRequestEntries.member.1.Id", "one"),
        ("PublishBatchRequestEntries.member.1.Message", "first"),
        ("PublishBatchRequestEntries.member.2.Id", "two"),
        ("PublishBatchRequestEntries.member.2.Message", "second"),
    ];
    let mut params: Vec<(&str, &str)> = vec![("TopicArn", topic.as_str())];
    params.extend_from_slice(entries);

    let (status, text) = sns_call(&emulator, "PublishBatch", &params).await;
    assert_eq!(
        status, 200,
        "a batch published under an enabled key must be accepted, got {text}"
    );
    assert_eq!(
        text.matches("<MessageId>").count(),
        2,
        "every entry of the accepted batch must report a MessageId, got {text}"
    );

    set_key_enabled(&emulator, &key_id, false).await;
    let (status, text) = sns_call(&emulator, "PublishBatch", &params).await;
    let code = xml_unescape(&element(&text, "Code"));
    assert_eq!(
        (status, code.as_str()),
        (400, "KMSDisabled"),
        "the whole batch must be refused while the topic's key is disabled, got {status} / {text}"
    );
    assert_relayed(
        "a batch published under a disabled key",
        &xml_unescape(&element(&text, "Message")),
        &format!("{key_arn} is disabled."),
        "DisabledException",
    );
    assert!(
        !text.contains("<MessageId>"),
        "a refused batch publishes nothing, so no entry may report a MessageId, got {text}"
    );

    set_key_enabled(&emulator, &key_id, true).await;
    let (status, text) = sns_call(&emulator, "PublishBatch", &params).await;
    assert_eq!(
        status, 200,
        "the batch published after the key was enabled again must be accepted, got {text}"
    );
}

/// The topic's key is read from the topic's own persisted attributes, so a key SET on a topic
/// that had none governs the next publish, and clearing it takes the topic off the key.
#[tokio::test]
async fn setting_and_clearing_the_topic_key_moves_the_publish_with_it() {
    let emulator = emulator();
    let (key_id, key_arn) = given_key(&emulator).await;
    let topic = given_topic(&emulator, "key-set-later", None).await;
    set_key_enabled(&emulator, &key_id, false).await;

    // (case, the value `SetTopicAttributes` writes for `KmsMasterKeyId`, the code the next
    //  publish must answer with — `""` for an accepted one)
    let rows: &[(&str, &str, &str)] = &[
        ("no key is set yet", "", ""),
        ("the disabled key is set", key_arn.as_str(), "KMSDisabled"),
        ("the key is cleared again", "", ""),
    ];

    for (case, value, expected) in rows {
        if !value.is_empty() || *case == "the key is cleared again" {
            sns(
                &emulator,
                "SetTopicAttributes",
                &[
                    ("TopicArn", topic.as_str()),
                    ("AttributeName", "KmsMasterKeyId"),
                    ("AttributeValue", value),
                ],
            )
            .await;
        }
        let (status, code, sentence) = publish(&emulator, &topic, "sealed").await;
        let status_expected = if expected.is_empty() { 200 } else { 400 };
        assert_eq!(
            (status, code.as_str()),
            (status_expected, *expected),
            "case {case:?}: expected {status_expected} / {expected:?}, got {status} / {code} / \
             {sentence:?}"
        );
    }
}

/// The reported attributes of an encrypted topic are untouched by the publish path: the key a
/// topic carries is what a reader reports, whatever condition that key is in.
#[tokio::test]
async fn a_refused_publish_leaves_the_topics_reported_key_unchanged() {
    let emulator = emulator();
    let (key_id, key_arn) = given_key(&emulator).await;
    let topic = given_topic(&emulator, "reported-key", Some(&key_arn)).await;
    set_key_enabled(&emulator, &key_id, false).await;
    publish(&emulator, &topic, "refused").await;

    let attributes = sns(
        &emulator,
        "GetTopicAttributes",
        &[("TopicArn", topic.as_str())],
    )
    .await;
    let entries: Vec<&str> = attributes.split("<entry>").collect();
    let reported = entries
        .iter()
        .find(|entry| entry.contains("<key>KmsMasterKeyId</key>"))
        .unwrap_or_else(|| panic!("no KmsMasterKeyId entry in {attributes}"));
    assert!(
        reported.contains(&format!("<value>{key_arn}</value>")),
        "the topic must go on reporting the key it carries, got {reported:?}"
    );
}
