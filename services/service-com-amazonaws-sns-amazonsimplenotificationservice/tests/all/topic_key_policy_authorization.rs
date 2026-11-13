// SPDX-License-Identifier: CC0-1.0

//! A publish to a topic encrypted with a customer managed key is served only where the
//! authorities in force let that publish have a data key generated under the key, end to end
//! over the wire through the real `MultiServiceRouter` with KMS mounted on the same engine —
//! so the refusal is observed where a client observes it: in the answer to the signed call.
//!
//! The RULE asserted here: a topic carrying `KmsMasterKeyId` has every message published to it
//! sealed under that key, and Amazon SNS asks KMS for that data key as the message is
//! published — so the permission to use the key is in force over the publish, and a key whose
//! documents admit the publishing call nothing seals no message. The call is then refused
//! rather than answered with a message id, in the class this operation models for it:
//! `KMSAccessDeniedException` — HTTP 400, `awsQueryError` code `KMSAccessDenied`
//! (`smithy_query(query="shape", shape="KMSAccessDeniedException")`) — carrying KMS's own
//! refusal sentence relayed with the metadata block an AWS error carries, because the call
//! that failed is this service's own call to KMS.
//!
//! The rows differ along what the rule generalises over: whether the topic is on a customer
//! managed key at all, which principal the key policy grants to, which of the KMS operations
//! sealing a message performs it grants, which resource it grants them on, how the grant is
//! scoped by condition (the service the use arrives through), whether a `Deny` stands beside
//! the grant, and which operation does the publishing (`Publish` and `PublishBatch`, both of
//! which write a message to the topic and both of which model the `KMS*` family).

use serde_json::{json, Value};

use crate::sqs_subscription_delivery::{
    drain, element, emulator, given_queue, kms, sns, sns_call, Emulator,
};
use crate::topics_subscriptions_publish::xml_unescape;

/// The Region every call in this suite is signed for, which is the Region the keys and the
/// topics are minted in.
const REGION: &str = "us-east-1";

/// The account this emulator mints its resources in — the identity a key policy's
/// administrative statement names.
fn account() -> String {
    engine::emulator_account_id().to_string()
}

/// An account that is not this one, for the row whose key policy names somebody else.
const ANOTHER_ACCOUNT: &str = "210987654321";

/// The statement `CreateKey` writes by default, which "gives the Amazon Web Services account
/// that owns the KMS key permission to use it in all cryptographic operations" — the document
/// a direct publish by a principal of that account draws its permission from.
fn administers(account: &str) -> Value {
    json!({
        "Sid": "AccountAdministers",
        "Effect": "Allow",
        "Principal": {"AWS": format!("arn:aws:iam::{account}:root")},
        "Action": "kms:*",
        "Resource": "*",
    })
}

/// A statement of `effect` for `principal` over `actions` on `resource`, with `condition` when
/// the row scopes it.
fn statement(
    effect: &str,
    principal: Value,
    actions: Value,
    resource: &str,
    condition: Option<Value>,
) -> Value {
    let mut built = json!({
        "Sid": "TheRowsStatement",
        "Effect": effect,
        "Principal": principal,
        "Action": actions,
        "Resource": resource,
    });
    if let Some(condition) = condition {
        built
            .as_object_mut()
            .expect("the statement being built is a JSON object")
            .insert("Condition".to_string(), condition);
    }
    built
}

/// The principal spelling a key policy names this account's own identities by.
fn account_principal(account: &str) -> Value {
    json!({"AWS": format!("arn:aws:iam::{account}:root")})
}

/// A key policy document holding exactly `statements`.
fn key_policy(statements: Vec<Value>) -> String {
    json!({"Version": "2012-10-17", "Statement": statements}).to_string()
}

/// A key this emulator's KMS minted, carrying `statements` as its whole policy: its ARN, which
/// is what a topic's `KmsMasterKeyId` names it by.
///
/// The lockout safety check is bypassed because a row's policy may deliberately admit the
/// account nothing — "(To omit this condition, set `BypassPolicyLockoutSafetyCheck` to true.)"
/// (`PutKeyPolicy`).
async fn given_key(emulator: &Emulator, statements: Vec<Value>) -> String {
    let created = kms(emulator, "CreateKey", &json!({})).await;
    let key_arn = created["KeyMetadata"]["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateKey: no KeyMetadata.Arn in {created}"))
        .to_string();
    kms(
        emulator,
        "PutKeyPolicy",
        &json!({
            "KeyId": key_arn,
            "PolicyName": "default",
            "Policy": key_policy(statements),
            "BypassPolicyLockoutSafetyCheck": true,
        }),
    )
    .await;
    key_arn
}

/// A topic called `name`, encrypted under the key `master_key_id` names when the row names
/// one: its ARN.
async fn given_topic(emulator: &Emulator, name: &str, master_key_id: Option<&str>) -> String {
    let mut params: Vec<(&str, &str)> = vec![("Name", name)];
    if let Some(key) = master_key_id {
        params.push(("Attributes.entry.1.key", "KmsMasterKeyId"));
        params.push(("Attributes.entry.1.value", key));
    }
    element(&sns(emulator, "CreateTopic", &params).await, "TopicArn")
}

/// What one publishing call answered: its HTTP status, the awsQuery error code (`""` on a
/// success envelope) and the sentence it carries — the message id of an accepted publish, or
/// the message of a refused one, with the envelope's XML entities decoded.
struct Answered {
    status: u16,
    code: String,
    sentence: String,
}

/// Publish `body` to `topic`, through the operation `action` names: `Publish` carries the
/// message in `Message`, `PublishBatch` in one entry of its batch.
async fn publish(emulator: &Emulator, action: &str, topic: &str, body: &str) -> Answered {
    let params: Vec<(&str, &str)> = match action {
        "PublishBatch" => vec![
            ("TopicArn", topic),
            ("PublishBatchRequestEntries.member.1.Id", "one"),
            ("PublishBatchRequestEntries.member.1.Message", body),
        ],
        _ => vec![("TopicArn", topic), ("Message", body)],
    };
    let (status, text) = sns_call(emulator, action, &params).await;
    if status == 200 {
        return Answered {
            status,
            code: String::new(),
            sentence: element(&text, "MessageId"),
        };
    }
    Answered {
        status,
        code: xml_unescape(&element(&text, "Code")),
        sentence: xml_unescape(&element(&text, "Message")),
    }
}

/// What a publish to the topic must answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// The data key was generated, so the message is accepted.
    Served,
    /// No document in force permits the data key, so the publish is refused.
    AccessDenied,
}

/// The status this service answers a refused `KMS*` class with, which the model binds every
/// shape of that family to.
const KMS_REFUSAL_STATUS: u16 = 400;

/// The awsQuery error code of the class a withheld data key is refused in.
const KMS_ACCESS_DENIED: &str = "KMSAccessDenied";

/// KMS's own error code for a use no document in force permits, which the relayed metadata
/// block reports.
const RELAYED_KMS_CODE: &str = "Error Code: AccessDeniedException";

/// The tail of the metadata block an AWS error carries, which every relayed KMS refusal ends
/// with; the request id inside it is minted per refused call.
const RELAY_TAIL: &str = "; Proxy: null)";

/// The clause IAM's refusal sentence opens a withheld action with.
const NOT_AUTHORIZED: &str = "is not authorized to perform: kms:";

#[tokio::test]
async fn a_publish_to_an_encrypted_topic_is_served_only_where_the_key_admits_it() {
    let emulator = emulator();
    let account = account();
    let this_service = format!("sns.{REGION}.amazonaws.com");
    let another_service = format!("dynamodb.{REGION}.amazonaws.com");

    // (case, the statements the key's policy holds — `None` for a topic on no customer managed
    //  key at all, what the publish must answer)
    let rows: Vec<(&str, Option<Vec<Value>>, Expected)> = vec![
        (
            "no customer managed key at all, so no key policy is in force",
            None,
            Expected::Served,
        ),
        (
            "the policy CreateKey writes for the account that owns the key",
            Some(vec![administers(&account)]),
            Expected::Served,
        ),
        (
            "a key policy granting the account a read of the key and nothing more",
            Some(vec![statement(
                "Allow",
                account_principal(&account),
                json!("kms:DescribeKey"),
                "*",
                None,
            )]),
            Expected::AccessDenied,
        ),
        (
            "a key policy naming another account's identities",
            Some(vec![administers(ANOTHER_ACCOUNT)]),
            Expected::AccessDenied,
        ),
        (
            "a key policy naming this service's own principal and not the publisher",
            Some(vec![statement(
                "Allow",
                json!({"Service": "sns.amazonaws.com"}),
                json!(["kms:GenerateDataKey*", "kms:Decrypt"]),
                "*",
                None,
            )]),
            Expected::AccessDenied,
        ),
        (
            "the account granted exactly the operations sealing a message performs",
            Some(vec![statement(
                "Allow",
                account_principal(&account),
                json!(["kms:GenerateDataKey*", "kms:Decrypt"]),
                "*",
                None,
            )]),
            Expected::Served,
        ),
        (
            "the account granted the data-key generation without the decrypt",
            Some(vec![statement(
                "Allow",
                account_principal(&account),
                json!("kms:GenerateDataKey*"),
                "*",
                None,
            )]),
            Expected::AccessDenied,
        ),
        (
            "the account's grant beside an explicit deny of the decrypt",
            Some(vec![
                administers(&account),
                statement(
                    "Deny",
                    account_principal(&account),
                    json!("kms:Decrypt"),
                    "*",
                    None,
                ),
            ]),
            Expected::AccessDenied,
        ),
        (
            "the account's grant scoped to the use arriving through this service",
            Some(vec![statement(
                "Allow",
                account_principal(&account),
                json!("kms:*"),
                "*",
                Some(json!({"StringEquals": {"kms:ViaService": this_service}})),
            )]),
            Expected::Served,
        ),
        (
            "the account's grant scoped to a use arriving through another service",
            Some(vec![statement(
                "Allow",
                account_principal(&account),
                json!("kms:*"),
                "*",
                Some(json!({"StringEquals": {"kms:ViaService": another_service}})),
            )]),
            Expected::AccessDenied,
        ),
    ];

    for (index, (case, statements, expected)) in rows.iter().enumerate() {
        let key = match statements {
            Some(statements) => Some(given_key(&emulator, statements.clone()).await),
            None => None,
        };
        let topic = given_topic(
            &emulator,
            &format!("key-policy-{index:02}"),
            key.as_deref(),
        )
        .await;
        // Both publishing operations seal their messages under the SAME key, so one key policy
        // decides both.
        for action in ["Publish", "PublishBatch"] {
            let answered = publish(&emulator, action, &topic, "a message").await;
            match expected {
                Expected::Served => {
                    assert_eq!(
                        (answered.status, answered.code.as_str()),
                        (200, ""),
                        "case {case:?} through {action}: expected the publish to be served, got \
                         {} / {:?}",
                        answered.status,
                        answered.sentence
                    );
                    assert!(
                        !answered.sentence.is_empty(),
                        "case {case:?} through {action}: a served publish answers a message id, \
                         got {:?}",
                        answered.sentence
                    );
                }
                Expected::AccessDenied => {
                    assert_eq!(
                        (answered.status, answered.code.as_str()),
                        (KMS_REFUSAL_STATUS, KMS_ACCESS_DENIED),
                        "case {case:?} through {action}: expected the publish to be refused, got \
                         {} / {:?}",
                        answered.status,
                        answered.sentence
                    );
                    // The sentence is KMS's own, relayed with the metadata block of the KMS
                    // call that failed: this service does not word the refusal itself.
                    for required in [NOT_AUTHORIZED, RELAYED_KMS_CODE, RELAY_TAIL] {
                        assert!(
                            answered.sentence.contains(required),
                            "case {case:?} through {action}: expected the relayed KMS sentence to \
                             carry {required:?}, got {:?}",
                            answered.sentence
                        );
                    }
                    assert!(
                        answered.sentence.contains(key.as_deref().unwrap_or_default()),
                        "case {case:?} through {action}: expected the refusal to name the key it \
                         is about, got {:?}",
                        answered.sentence
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn a_refused_publish_delivers_nothing_and_is_served_once_the_key_policy_grants_it() {
    let emulator = emulator();
    // A key whose policy admits only a read of the key itself: the publish has no authority to
    // seal a message under it until a statement grants the data-key operations.
    let key = given_key(
        &emulator,
        vec![statement(
            "Allow",
            account_principal(&account()),
            json!("kms:DescribeKey"),
            "*",
            None,
        )],
    )
    .await;
    let topic = given_topic(&emulator, "withheld-then-granted", Some(&key)).await;
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

    let refused = publish(&emulator, "Publish", &topic, "withheld").await;
    assert_eq!(
        (refused.status, refused.code.as_str()),
        (KMS_REFUSAL_STATUS, KMS_ACCESS_DENIED),
        "the publish must be refused while the key admits no data-key operation, got {} / {:?}",
        refused.status,
        refused.sentence
    );

    // The refusal is taken before the message is recorded, so nothing was sealed and nothing
    // was delivered.
    let delivered = drain(&emulator, &url).await;
    assert!(
        delivered.is_empty(),
        "a refused publish seals and delivers nothing, so the subscriber must stay empty, got \
         {delivered:?}"
    );

    // The permission is read at PUBLISH time, so adding the grant lets the next publish
    // through with nothing else changed.
    kms(
        &emulator,
        "PutKeyPolicy",
        &json!({
            "KeyId": key,
            "PolicyName": "default",
            "Policy": key_policy(vec![administers(&account())]),
            "BypassPolicyLockoutSafetyCheck": true,
        }),
    )
    .await;
    let served = publish(&emulator, "Publish", &topic, "granted").await;
    assert_eq!(
        (served.status, served.code.as_str()),
        (200, ""),
        "the publish must be served once the key policy grants it, got {} / {:?}",
        served.status,
        served.sentence
    );
    assert!(
        !served.sentence.is_empty(),
        "a served publish answers a message id, got {:?}",
        served.sentence
    );
    assert_eq!(
        drain(&emulator, &url)
            .await
            .iter()
            .map(|(body, _)| body.clone())
            .collect::<Vec<_>>(),
        vec!["granted".to_string()],
        "only the message published once the key policy granted the use may arrive"
    );
}
