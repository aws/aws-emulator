// SPDX-License-Identifier: CC0-1.0

//! A `Publish` is served only where the policies in force authorize it, end to end over the
//! wire through the real `MultiServiceRouter` with IAM, STS and SQS mounted on the same
//! engine — so the refusal and the grant are observed where a client observes them: in the
//! answer to the signed call, and on the queue subscribed to the topic.
//!
//! The RULE asserted here: a publish to a topic is authorized by the UNION of the calling
//! principal's identity-based policies and the topic's own `Policy` attribute. An explicit
//! `Deny` in either refuses, whatever any grant says; otherwise an `Allow` about the publish
//! in EITHER document is enough, so a principal holding no identity-based policy at all may
//! publish to a topic whose policy names it, and a principal the topic's policy says nothing
//! about may publish when its own policies allow the action on that topic. A statement naming
//! the ACCOUNT delegates to that account's identity-based policies rather than granting, and
//! with neither grant the call is refused with the modelled `AuthorizationErrorException` —
//! HTTP 403, `awsQueryError` code `AuthorizationError`
//! (`smithy_query(query="operation", operation="Publish")`) — carrying IAM's refusal
//! sentence, and nothing is recorded or delivered.
//!
//! The rows differ along what the rule generalises over: which of the two documents carries
//! the grant, whether either carries an explicit deny, the ACTION and the RESOURCE a grant is
//! written for (this action, every action of the service, every action at all, another
//! action, another topic), which principal a topic statement names (the caller, the account,
//! another principal, everyone), the LETTER CASE a policy spells the action in, and whether
//! the topic carries a policy of its own at all — a topic whose owner has set none carries the
//! one SNS generates, whose only statement is conditioned on a cross-service key a directly
//! signed call carries no value for.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::sqs_subscription_delivery::grant_to_sns;
use crate::topics_subscriptions_publish::{percent_encode, text_of};

/// The Region every call below is signed for.
pub(crate) const REGION: &str = "us-east-1";

/// The access key id of the credential this emulator publishes at start, which no IAM record
/// names: the account's own, the one every in-process call between services is signed with.
pub(crate) const AMBIENT_KEY: &str = "AKIDTESTTESTTEST";

/// The action a publish is authorized under, as SNS's own policy actions and its refusals
/// spell it.
const PUBLISH_ACTION: &str = "SNS:Publish";

/// SNS with the three services an authorization decision spans, on ONE engine: the IAM store
/// holding the caller's identity-based policies, the STS store that resolves which principal
/// the request's access key id speaks for, and SQS, whose queue is where a delivery the
/// publish owed is observed.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

pub(crate) fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    Emulator { router }
}

/// A SigV4 `Authorization` whose credential scope names `signing_name` and whose key id is
/// `access_key_id` — which is the only part of it that matters here: the key id is what names
/// the principal the request speaks for, and this harness turns request authentication off, so
/// the signature itself is not a credential.
fn authorization(signing_name: &str, access_key_id: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// One form-encoded (awsQuery) call signed with `access_key_id`, with whatever status it
/// answered: the shape both SNS and IAM take, differing only in the signing name and version.
async fn query(
    emulator: &Emulator,
    signing_name: &str,
    version: &str,
    access_key_id: &str,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        // The form encoding is the suite-shared one, not a second copy.
        body.push_str(&percent_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(signing_name, access_key_id))
        .body(bytes::Bytes::from(body))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    (status, text)
}

/// Form parameters from `(name, value)` pairs.
fn params(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect()
}

/// One SNS call signed with `access_key_id`, with whatever status it answered.
pub(crate) async fn sns_as(
    emulator: &Emulator,
    access_key_id: &str,
    action: &str,
    pairs: &[(&str, &str)],
) -> (u16, String) {
    query(
        emulator,
        "sns",
        "2010-03-31",
        access_key_id,
        action,
        &params(pairs),
    )
    .await
}

/// One SNS call made with the account's own credential, which must succeed: the setup calls
/// of each row.
async fn sns(emulator: &Emulator, action: &str, pairs: &[(&str, &str)]) -> String {
    let (status, body) = sns_as(emulator, AMBIENT_KEY, action, pairs).await;
    assert_eq!(
        status, 200,
        "SNS {action}: expected HTTP 200, got {status} / {body}"
    );
    body
}

/// One IAM call that must succeed.
pub(crate) async fn iam(emulator: &Emulator, action: &str, pairs: &[(&str, &str)]) -> String {
    let (status, body) = query(
        emulator,
        "iam",
        "2010-05-08",
        AMBIENT_KEY,
        action,
        &params(pairs),
    )
    .await;
    assert_eq!(
        status, 200,
        "IAM {action}: expected HTTP 200, got {status} / {body}"
    );
    body
}

/// One STS call that must succeed.
pub(crate) async fn sts(emulator: &Emulator, action: &str, pairs: &[(&str, &str)]) -> String {
    let (status, body) = query(
        emulator,
        "sts",
        "2011-06-15",
        AMBIENT_KEY,
        action,
        &params(pairs),
    )
    .await;
    assert_eq!(
        status, 200,
        "STS {action}: expected HTTP 200, got {status} / {body}"
    );
    body
}

/// One `awsJson1_0` call against SQS that must succeed.
async fn sqs(emulator: &Emulator, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sqs.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .header("authorization", authorization("sqs", AMBIENT_KEY))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "SQS {operation}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// A topic, with a queue subscribed to it: the ARN of the topic, and the url of the queue a
/// delivery is counted on.
async fn given_topic_with_a_subscribed_queue(emulator: &Emulator, name: &str) -> (String, String) {
    let topic = text_of(&sns(emulator, "CreateTopic", &[("Name", name)]).await, "TopicArn");
    let created = sqs(
        emulator,
        "CreateQueue",
        &json!({"QueueName": format!("{name}-queue")}),
    )
    .await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateQueue {name}: no QueueUrl in {created}"))
        .to_string();
    let attributes = sqs(
        emulator,
        "GetQueueAttributes",
        &json!({"QueueUrl": url, "AttributeNames": ["QueueArn"]}),
    )
    .await;
    let queue_arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("GetQueueAttributes {url}: no QueueArn in {attributes}"))
        .to_string();
    // The queue policy AWS documents for an SNS subscriber, so a delivery that IS owed arrives
    // rather than being withheld by the queue.
    sqs(
        emulator,
        "SetQueueAttributes",
        &json!({"QueueUrl": url, "Attributes": {"Policy": grant_to_sns(&queue_arn)}}),
    )
    .await;
    sns(
        emulator,
        "Subscribe",
        &[
            ("TopicArn", topic.as_str()),
            ("Protocol", "sqs"),
            ("Endpoint", queue_arn.as_str()),
            ("ReturnSubscriptionArn", "true"),
        ],
    )
    .await;
    (topic, url)
}

/// An IAM user with an access key: its ARN, and the key id a request signs with to speak for
/// it.
pub(crate) async fn given_user(emulator: &Emulator, name: &str) -> (String, String) {
    let created = iam(emulator, "CreateUser", &[("UserName", name)]).await;
    let arn = text_of(&created, "Arn");
    assert!(
        !arn.is_empty(),
        "CreateUser {name}: expected an Arn, got {created}"
    );
    let key = iam(emulator, "CreateAccessKey", &[("UserName", name)]).await;
    let access_key_id = text_of(&key, "AccessKeyId");
    assert!(
        !access_key_id.is_empty(),
        "CreateAccessKey {name}: expected an AccessKeyId, got {key}"
    );
    (arn, access_key_id)
}

/// An IAM role the account may assume, assumed: the role's own ARN, the ARN STS reports for the
/// session it minted, and the key id a request signs with to speak for that session.
///
/// The role trusts the account's root principal, which is the spelling that admits every
/// principal of the account — so the setup call, made with the account's own credential, may
/// assume it.
pub(crate) async fn given_assumed_role(
    emulator: &Emulator,
    name: &str,
) -> (String, String, String) {
    let trust = policy(json!({
        "Effect": "Allow",
        "Principal": {"AWS": format!("arn:aws:iam::{}:root", engine::emulator_account_id())},
        "Action": "sts:AssumeRole",
    }));
    let created = iam(
        emulator,
        "CreateRole",
        &[
            ("RoleName", name),
            ("AssumeRolePolicyDocument", &trust),
        ],
    )
    .await;
    let role_arn = text_of(&created, "Arn");
    assert!(
        !role_arn.is_empty(),
        "CreateRole {name}: expected an Arn, got {created}"
    );
    let assumed = sts(
        emulator,
        "AssumeRole",
        &[("RoleArn", &role_arn), ("RoleSessionName", "publishing")],
    )
    .await;
    let session_arn = text_of(&assumed, "Arn");
    let access_key_id = text_of(&assumed, "AccessKeyId");
    assert!(
        !session_arn.is_empty() && !access_key_id.is_empty(),
        "AssumeRole {name}: expected a session ARN and an AccessKeyId, got {assumed}"
    );
    (role_arn, session_arn, access_key_id)
}

/// How many messages the queue at `url` holds, DELETED as they are read.
async fn depth(emulator: &Emulator, url: &str) -> usize {
    let received = sqs(
        emulator,
        "ReceiveMessage",
        &json!({"QueueUrl": url, "MaxNumberOfMessages": 10}),
    )
    .await;
    let messages = received["Messages"].as_array().cloned().unwrap_or_default();
    for message in &messages {
        let handle = message["ReceiptHandle"].as_str().unwrap_or_default();
        sqs(
            emulator,
            "DeleteMessage",
            &json!({"QueueUrl": url, "ReceiptHandle": handle}),
        )
        .await;
    }
    messages.len()
}

/// One document around one statement.
pub(crate) fn policy(statement: Value) -> String {
    json!({"Version": "2012-10-17", "Statement": [statement]}).to_string()
}

/// The sentence IAM refuses an unauthorized call with: the principal that signed, the action,
/// the resource it addressed and which rule withheld the grant. Fixed text apart from those
/// four, each of which comes from the request and the stored policies.
fn not_authorized(principal: &str, resource: &str, reason: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {PUBLISH_ACTION} on resource: \
         {resource} {reason}"
    )
}

/// What the policies in force must decide for a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// The publish is served, and the subscribed queue receives it.
    Served,
    /// The publish is refused with this clause, and the queue receives nothing.
    Refused(&'static str),
}

/// The clause a refusal states when no readable identity-based document allows the action.
const NO_IDENTITY_GRANT: &str =
    "because no identity-based policy allows the SNS:Publish action";

/// The clause a refusal states when the resource's own policy explicitly denies it.
const RESOURCE_DENY: &str = "with an explicit deny in a resource-based policy";

/// The clause a refusal states when one of the principal's own documents explicitly denies it.
const IDENTITY_DENY: &str = "with an explicit deny in an identity-based policy";

/// A document built from the topic's ARN and the caller's ARN, or `None` for a row that
/// installs none.
type DocumentOf = fn(&str, &str) -> Option<String>;

#[tokio::test]
async fn a_publish_is_served_only_where_the_policies_in_force_authorize_it() {
    // (case, the user's inline identity policy, the topic's `Policy`, the verdict)
    let rows: &[(&str, DocumentOf, DocumentOf, Verdict)] = &[
        (
            "neither document grants anything",
            |_, _| None,
            |_, _| None,
            Verdict::Refused(NO_IDENTITY_GRANT),
        ),
        (
            "the identity policy allows the publish on this topic",
            |topic, _| {
                Some(policy(json!({
                    "Effect": "Allow",
                    "Action": "sns:Publish",
                    "Resource": topic,
                })))
            },
            |_, _| None,
            Verdict::Served,
        ),
        (
            "the identity policy allows every action of the service, anywhere",
            |_, _| {
                Some(policy(json!({
                    "Effect": "Allow",
                    "Action": "sns:*",
                    "Resource": "*",
                })))
            },
            |_, _| None,
            Verdict::Served,
        ),
        (
            "the identity policy spells the action the way SNS does",
            |topic, _| {
                Some(policy(json!({
                    "Effect": "Allow",
                    "Action": "SNS:Publish",
                    "Resource": topic,
                })))
            },
            |_, _| None,
            Verdict::Served,
        ),
        (
            "the identity policy allows another action of the service",
            |topic, _| {
                Some(policy(json!({
                    "Effect": "Allow",
                    "Action": "sns:Subscribe",
                    "Resource": topic,
                })))
            },
            |_, _| None,
            Verdict::Refused(NO_IDENTITY_GRANT),
        ),
        (
            "the identity policy allows the publish on ANOTHER topic",
            |topic, _| {
                Some(policy(json!({
                    "Effect": "Allow",
                    "Action": "sns:Publish",
                    "Resource": format!("{topic}-elsewhere"),
                })))
            },
            |_, _| None,
            Verdict::Refused(NO_IDENTITY_GRANT),
        ),
        (
            "the TOPIC policy alone names the caller",
            |_, _| None,
            |topic, user| {
                Some(policy(json!({
                    "Effect": "Allow",
                    "Principal": {"AWS": user},
                    "Action": "SNS:Publish",
                    "Resource": topic,
                })))
            },
            Verdict::Served,
        ),
        (
            "the topic policy names everyone, unconditionally",
            |_, _| None,
            |topic, _| {
                Some(policy(json!({
                    "Effect": "Allow",
                    "Principal": {"AWS": "*"},
                    "Action": "SNS:Publish",
                    "Resource": topic,
                })))
            },
            Verdict::Served,
        ),
        (
            "the topic policy names ANOTHER principal",
            |_, _| None,
            |topic, user| {
                Some(policy(json!({
                    "Effect": "Allow",
                    "Principal": {"AWS": format!("{user}-elsewhere")},
                    "Action": "SNS:Publish",
                    "Resource": topic,
                })))
            },
            Verdict::Refused(NO_IDENTITY_GRANT),
        ),
        (
            "the topic policy names the ACCOUNT, which delegates rather than grants",
            |_, _| None,
            |topic, user| {
                Some(policy(json!({
                    "Effect": "Allow",
                    "Principal": {"AWS": account_root_of(user)},
                    "Action": "SNS:Publish",
                    "Resource": topic,
                })))
            },
            Verdict::Refused(NO_IDENTITY_GRANT),
        ),
        (
            "the account delegation beside an identity grant",
            |topic, _| {
                Some(policy(json!({
                    "Effect": "Allow",
                    "Action": "sns:Publish",
                    "Resource": topic,
                })))
            },
            |topic, user| {
                Some(policy(json!({
                    "Effect": "Allow",
                    "Principal": {"AWS": account_root_of(user)},
                    "Action": "SNS:Publish",
                    "Resource": topic,
                })))
            },
            Verdict::Served,
        ),
        (
            "an explicit deny in the topic policy over an identity grant",
            |topic, _| {
                Some(policy(json!({
                    "Effect": "Allow",
                    "Action": "sns:Publish",
                    "Resource": topic,
                })))
            },
            |topic, user| {
                Some(policy(json!({
                    "Effect": "Deny",
                    "Principal": {"AWS": user},
                    "Action": "SNS:Publish",
                    "Resource": topic,
                })))
            },
            Verdict::Refused(RESOURCE_DENY),
        ),
        (
            "an explicit deny in the identity policy over a topic grant",
            |topic, _| {
                Some(policy(json!({
                    "Effect": "Deny",
                    "Action": "sns:Publish",
                    "Resource": topic,
                })))
            },
            |topic, user| {
                Some(policy(json!({
                    "Effect": "Allow",
                    "Principal": {"AWS": user},
                    "Action": "SNS:Publish",
                    "Resource": topic,
                })))
            },
            Verdict::Refused(IDENTITY_DENY),
        ),
        (
            "a topic deny of the ACCOUNT, which denies each of its principals",
            |topic, _| {
                Some(policy(json!({
                    "Effect": "Allow",
                    "Action": "sns:Publish",
                    "Resource": topic,
                })))
            },
            |topic, user| {
                Some(policy(json!({
                    "Effect": "Deny",
                    "Principal": {"AWS": account_root_of(user)},
                    "Action": "SNS:Publish",
                    "Resource": topic,
                })))
            },
            Verdict::Refused(RESOURCE_DENY),
        ),
    ];

    for (case, identity_document, topic_document, verdict) in rows {
        let emulator = emulator();
        let (topic, queue) = given_topic_with_a_subscribed_queue(&emulator, "orders").await;
        let (user, access_key_id) = given_user(&emulator, "publisher").await;
        if let Some(document) = identity_document(&topic, &user) {
            iam(
                &emulator,
                "PutUserPolicy",
                &[
                    ("UserName", "publisher"),
                    ("PolicyName", "publish"),
                    ("PolicyDocument", &document),
                ],
            )
            .await;
        }
        if let Some(document) = topic_document(&topic, &user) {
            sns(
                &emulator,
                "SetTopicAttributes",
                &[
                    ("TopicArn", &topic),
                    ("AttributeName", "Policy"),
                    ("AttributeValue", &document),
                ],
            )
            .await;
        }

        let (status, body) = sns_as(
            &emulator,
            &access_key_id,
            "Publish",
            &[("TopicArn", &topic), ("Message", "probe")],
        )
        .await;
        let delivered = depth(&emulator, &queue).await;
        match verdict {
            Verdict::Served => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the publish to be served, got {status} / {body}"
                );
                let message_id = text_of(&body, "MessageId");
                assert!(
                    !message_id.is_empty(),
                    "case {case:?}: expected a MessageId, got {body}"
                );
                assert_eq!(
                    delivered, 1,
                    "case {case:?}: expected the subscribed queue to receive the message, got \
                     {delivered} message(s)"
                );
            }
            Verdict::Refused(reason) => {
                assert_eq!(
                    status, 403,
                    "case {case:?}: expected HTTP 403, got {status} / {body}"
                );
                let code = text_of(&body, "Code");
                assert_eq!(
                    code, "AuthorizationError",
                    "case {case:?}: expected the AuthorizationError code, got {code:?} in {body}"
                );
                let message = text_of(&body, "Message");
                assert_eq!(
                    message,
                    not_authorized(&user, &topic, reason),
                    "case {case:?}: expected IAM's refusal wording, got {message:?}"
                );
                assert_eq!(
                    delivered, 0,
                    "case {case:?}: a refused publish must deliver nothing, got {delivered} \
                     message(s)"
                );
            }
        }
    }
}

/// The root user of the account an IAM ARN names: the spelling a resource policy names a whole
/// account by, read off the caller's own ARN rather than written out.
fn account_root_of(user_arn: &str) -> String {
    let account = user_arn
        .split(':')
        .nth(4)
        .unwrap_or_else(|| panic!("no account in {user_arn}"));
    format!("arn:aws:iam::{account}:root")
}

#[tokio::test]
async fn a_batch_publish_is_decided_by_the_same_rule() {
    let emulator = emulator();
    let (topic, queue) = given_topic_with_a_subscribed_queue(&emulator, "batched").await;
    let (user, access_key_id) = given_user(&emulator, "batcher").await;
    let entries = [
        ("TopicArn", topic.as_str()),
        ("PublishBatchRequestEntries.member.1.Id", "one"),
        ("PublishBatchRequestEntries.member.1.Message", "first"),
        ("PublishBatchRequestEntries.member.2.Id", "two"),
        ("PublishBatchRequestEntries.member.2.Message", "second"),
    ];

    // A batch publishes under the same action one `Publish` does, so a caller neither document
    // grants is refused the whole batch and no entry of it is delivered.
    let (status, body) = sns_as(&emulator, &access_key_id, "PublishBatch", &entries).await;
    assert_eq!(
        status, 403,
        "case \"a batch by a principal with no grant\": expected HTTP 403, got {status} / {body}"
    );
    let code = text_of(&body, "Code");
    assert_eq!(
        code, "AuthorizationError",
        "case \"a batch by a principal with no grant\": expected the AuthorizationError code, \
         got {code:?} in {body}"
    );
    let message = text_of(&body, "Message");
    assert_eq!(
        message,
        not_authorized(&user, &topic, NO_IDENTITY_GRANT),
        "case \"a batch by a principal with no grant\": expected IAM's refusal wording, got \
         {message:?}"
    );
    let delivered = depth(&emulator, &queue).await;
    assert_eq!(
        delivered, 0,
        "case \"a batch by a principal with no grant\": a refused batch must deliver nothing, \
         got {delivered} message(s)"
    );

    // The same signed call, once the caller's own policy allows the action on that topic.
    iam(
        &emulator,
        "PutUserPolicy",
        &[
            ("UserName", "batcher"),
            ("PolicyName", "publish"),
            (
                "PolicyDocument",
                &policy(json!({
                    "Effect": "Allow",
                    "Action": "sns:Publish",
                    "Resource": topic,
                })),
            ),
        ],
    )
    .await;
    let (status, body) = sns_as(&emulator, &access_key_id, "PublishBatch", &entries).await;
    assert_eq!(
        status, 200,
        "case \"a batch by a granted principal\": expected HTTP 200, got {status} / {body}"
    );
    let delivered = depth(&emulator, &queue).await;
    assert_eq!(
        delivered, 2,
        "case \"a batch by a granted principal\": expected both entries delivered, got \
         {delivered} message(s)"
    );
}

#[tokio::test]
async fn the_accounts_own_credential_publishes_and_a_topic_policy_does_not_bound_it() {
    let emulator = emulator();
    let (topic, queue) = given_topic_with_a_subscribed_queue(&emulator, "ambient").await;

    // The account's own published credential holds no IAM policies — the position AWS account
    // root credentials are in, and the credential every in-process call another emulated
    // service makes to this one is signed with — so its publish is served.
    let (status, body) = sns_as(
        &emulator,
        AMBIENT_KEY,
        "Publish",
        &[("TopicArn", &topic), ("Message", "probe")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the account's own credential\": expected the publish to be served, got {status} \
         / {body}"
    );
    let delivered = depth(&emulator, &queue).await;
    assert_eq!(
        delivered, 1,
        "case \"the account's own credential\": expected the message delivered, got {delivered} \
         message(s)"
    );

    // Neither document is in force on that credential, so a topic policy that grants nobody
    // — even one denying the account outright — leaves the in-process callers of this service
    // reachable, which is the exemption the gate states.
    let account_root = format!("arn:aws:iam::{}:root", engine::emulator_account_id());
    sns(
        &emulator,
        "SetTopicAttributes",
        &[
            ("TopicArn", &topic),
            ("AttributeName", "Policy"),
            (
                "AttributeValue",
                &policy(json!({
                    "Effect": "Deny",
                    "Principal": {"AWS": account_root},
                    "Action": "SNS:Publish",
                    "Resource": topic,
                })),
            ),
        ],
    )
    .await;
    let (status, body) = sns_as(
        &emulator,
        AMBIENT_KEY,
        "Publish",
        &[("TopicArn", &topic), ("Message", "probe")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a topic policy over the account's own credential\": expected the publish to be \
         served, got {status} / {body}"
    );
    let delivered = depth(&emulator, &queue).await;
    assert_eq!(
        delivered, 1,
        "case \"a topic policy over the account's own credential\": expected the message \
         delivered, got {delivered} message(s)"
    );
}

/// Whether a row's grant is written on the ROLE's own policies or on the topic's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RoleGrant {
    /// An inline policy on the role allows the publish on the topic.
    OnTheRole,
    /// The topic's policy names the ROLE's ARN — not the session's — as the principal.
    OnTheTopic,
    /// Neither document says anything.
    Nowhere,
}

#[tokio::test]
async fn a_role_session_is_judged_on_the_roles_policies() {
    // (case, where the grant is written, the verdict)
    let rows: &[(&str, RoleGrant, Verdict)] = &[
        (
            "the role's own policy allows the publish",
            RoleGrant::OnTheRole,
            Verdict::Served,
        ),
        (
            "the topic policy names the ROLE the session is of",
            RoleGrant::OnTheTopic,
            Verdict::Served,
        ),
        (
            "neither document grants anything",
            RoleGrant::Nowhere,
            Verdict::Refused(NO_IDENTITY_GRANT),
        ),
    ];

    for (case, grant, verdict) in rows {
        let emulator = emulator();
        let (topic, queue) = given_topic_with_a_subscribed_queue(&emulator, "roled").await;
        let (role, session, access_key_id) = given_assumed_role(&emulator, "publishing-role").await;
        match grant {
            // The permissions in force on a session are the ROLE's, so the grant is written on
            // the role and the session inherits it.
            RoleGrant::OnTheRole => {
                iam(
                    &emulator,
                    "PutRolePolicy",
                    &[
                        ("RoleName", "publishing-role"),
                        ("PolicyName", "publish"),
                        (
                            "PolicyDocument",
                            &policy(json!({
                                "Effect": "Allow",
                                "Action": "sns:Publish",
                                "Resource": topic,
                            })),
                        ),
                    ],
                )
                .await;
            }
            // A resource policy names the ROLE rather than the session it minted, so a session
            // of that role is the principal such a statement grants to.
            RoleGrant::OnTheTopic => {
                sns(
                    &emulator,
                    "SetTopicAttributes",
                    &[
                        ("TopicArn", &topic),
                        ("AttributeName", "Policy"),
                        (
                            "AttributeValue",
                            &policy(json!({
                                "Effect": "Allow",
                                "Principal": {"AWS": role},
                                "Action": "SNS:Publish",
                                "Resource": topic,
                            })),
                        ),
                    ],
                )
                .await;
            }
            RoleGrant::Nowhere => {}
        }

        let (status, body) = sns_as(
            &emulator,
            &access_key_id,
            "Publish",
            &[("TopicArn", &topic), ("Message", "probe")],
        )
        .await;
        let delivered = depth(&emulator, &queue).await;
        match verdict {
            Verdict::Served => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the publish to be served, got {status} / {body}"
                );
                assert_eq!(
                    delivered, 1,
                    "case {case:?}: expected the message delivered, got {delivered} message(s)"
                );
            }
            Verdict::Refused(reason) => {
                assert_eq!(
                    status, 403,
                    "case {case:?}: expected HTTP 403, got {status} / {body}"
                );
                let code = text_of(&body, "Code");
                assert_eq!(
                    code, "AuthorizationError",
                    "case {case:?}: expected the AuthorizationError code, got {code:?} in {body}"
                );
                let message = text_of(&body, "Message");
                // The refusal names the principal `sts:GetCallerIdentity` reports for the
                // credential that signed: the SESSION, not the role behind it.
                assert_eq!(
                    message,
                    not_authorized(&session, &topic, reason),
                    "case {case:?}: expected IAM's refusal wording, got {message:?}"
                );
                assert_eq!(
                    delivered, 0,
                    "case {case:?}: a refused publish must deliver nothing, got {delivered} \
                     message(s)"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_permissions_boundary_caps_an_identity_grant() {
    // (case, the boundary document's Action element, the verdict)
    let rows: &[(&str, &str, Verdict)] = &[
        (
            "a boundary that also allows the publish",
            "sns:Publish",
            Verdict::Served,
        ),
        (
            "a boundary that allows another action only",
            "sns:Subscribe",
            Verdict::Refused("because no permissions boundary allows the SNS:Publish action"),
        ),
    ];

    for (case, boundary_action, verdict) in rows {
        let emulator = emulator();
        let (topic, queue) = given_topic_with_a_subscribed_queue(&emulator, "bounded").await;
        let (user, access_key_id) = given_user(&emulator, "bounded-publisher").await;
        iam(
            &emulator,
            "PutUserPolicy",
            &[
                ("UserName", "bounded-publisher"),
                ("PolicyName", "publish"),
                (
                    "PolicyDocument",
                    &policy(json!({
                        "Effect": "Allow",
                        "Action": "sns:Publish",
                        "Resource": topic,
                    })),
                ),
            ],
        )
        .await;
        let created = iam(
            &emulator,
            "CreatePolicy",
            &[
                ("PolicyName", "boundary"),
                (
                    "PolicyDocument",
                    &policy(json!({
                        "Effect": "Allow",
                        "Action": boundary_action,
                        "Resource": "*",
                    })),
                ),
            ],
        )
        .await;
        let boundary_arn = text_of(&created, "Arn");
        assert!(
            !boundary_arn.is_empty(),
            "case {case:?}: expected the boundary policy's Arn, got {created}"
        );
        iam(
            &emulator,
            "PutUserPermissionsBoundary",
            &[
                ("UserName", "bounded-publisher"),
                ("PermissionsBoundary", &boundary_arn),
            ],
        )
        .await;

        let (status, body) = sns_as(
            &emulator,
            &access_key_id,
            "Publish",
            &[("TopicArn", &topic), ("Message", "probe")],
        )
        .await;
        let delivered = depth(&emulator, &queue).await;
        match verdict {
            Verdict::Served => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the publish to be served, got {status} / {body}"
                );
                assert_eq!(
                    delivered, 1,
                    "case {case:?}: expected the message delivered, got {delivered} message(s)"
                );
            }
            Verdict::Refused(reason) => {
                assert_eq!(
                    status, 403,
                    "case {case:?}: expected HTTP 403, got {status} / {body}"
                );
                let message = text_of(&body, "Message");
                assert_eq!(
                    message,
                    not_authorized(&user, &topic, reason),
                    "case {case:?}: expected IAM's refusal wording, got {message:?}"
                );
                assert_eq!(
                    delivered, 0,
                    "case {case:?}: a refused publish must deliver nothing, got {delivered} \
                     message(s)"
                );
            }
        }
    }
}
