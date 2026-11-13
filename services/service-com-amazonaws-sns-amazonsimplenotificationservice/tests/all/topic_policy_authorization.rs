// SPDX-License-Identifier: CC0-1.0

//! IAM authorization of the SNS operations that address a TOPIC, over the awsQuery wire.
//!
//! The RULE asserted here: a signed call that acts on a topic is authorized by the documents
//! in force on it — the calling principal's own identity-based IAM policies and the topic's
//! access policy (its `Policy` attribute) read TOGETHER. An explicit `Deny` in either refuses
//! whatever the other allows, an `Allow` from either grants, and with neither the call is
//! refused `403 AuthorizationError` before the operation reads or writes anything. A
//! resource-based statement that names the ACCOUNT rather than the principal delegates to
//! that account's identity-based policies instead of granting on its own, which is why the
//! policy SNS generates for a topic whose owner set none authorizes nobody by itself.
//!
//! The principals are made with IAM's OWN operations over the same engine (`CreateUser` +
//! `CreateAccessKey` + `PutUserPolicy`), so the documents SNS reads are the ones IAM wrote,
//! and the calls below are the ones a client makes.
//!
//! The tables differ along what the rule generalises over: the OPERATION asked for (each of
//! the eight topic actions, plus the batch publish that performs the same action as
//! `Publish`), what the caller's identity policy says (nothing, an `Allow` of this action on
//! this topic, an `Allow` of another action, an `Allow` on another topic, a wildcard
//! `Allow`), what the topic's own policy says (the generated default, an `Allow` naming the
//! principal, an `Allow` naming the account, an explicit `Deny`), and which refusal clause
//! the sentence carries.

use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

use super::topics_subscriptions_publish::{percent_encode, text_of};

/// The region every call below is addressed to, and the one the topics are minted in.
const REGION: &str = "us-east-1";

/// The awsQuery `Version` of the SNS API, as the model's service version states it.
const SNS_VERSION: &str = "2010-03-31";

/// The awsQuery `Version` of the IAM API.
const IAM_VERSION: &str = "2010-05-08";

/// The access-key id standing for the account's own published credential: the IAM store
/// holds no record for it, so it speaks for the account itself, which carries no IAM policy
/// — the position AWS account root credentials are in. Every fixture call is signed with it.
const ACCOUNT_KEY: &str = "AKIDSNSAUTHORIZATION";

/// The status and wire error code SNS refuses an unauthorized call with: its
/// `AuthorizationErrorException`, which the model binds to HTTP 403 with the awsQuery code
/// `AuthorizationError`.
pub(crate) const AUTHORIZATION_ERROR: (u16, &str) = (403, "AuthorizationError");

/// The reason clause IAM states for the default deny a call with no grant anywhere falls to:
/// within one account the permission the principal is missing is its own, so the sentence
/// names the identity-based policies that did not grant it — the clause the recorded refusal
/// of this service carries.
pub(crate) fn no_identity_policy_allows(action: &str) -> String {
    format!("because no identity-based policy allows the {action} action")
}

/// The reason clause IAM states when the resource's own policy explicitly denies the call.
pub(crate) const RESOURCE_POLICY_DENY: &str = "with an explicit deny in a resource-based policy";

/// The reason clause IAM states when one of the principal's own documents explicitly denies
/// the call.
const IDENTITY_POLICY_DENY: &str = "with an explicit deny in an identity-based policy";

/// The sentence AWS refuses an unauthorized request with, stated here independently of the
/// crate under test.
pub(crate) fn not_authorized(principal: &str, action: &str, resource: &str, reason: &str) -> String {
    format!("User: {principal} is not authorized to perform: {action} on resource: {resource} {reason}")
}

/// An IAM principal a call can be signed by.
struct Principal {
    /// The friendly name IAM holds the user under, for a later policy write.
    user_name: String,
    /// The long-term access key id a request carries in its credential scope.
    access_key_id: String,
    /// The user's own ARN — what a refusal names.
    arn: String,
}

/// The emulator under test: one shared engine carrying SNS beside the IAM service the
/// principals and their policies are made with.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Nothing here signs: the `Authorization` header carries only the credential scope,
        // which is where a service reads the caller's access key id from.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// One awsQuery call, with `access_key_id` in the SigV4 credential scope.
    async fn query(
        &self,
        access_key_id: &str,
        signing_name: &str,
        version: &str,
        action: &str,
        params: &[(String, String)],
    ) -> (u16, String) {
        let mut body = format!("Action={action}&Version={version}");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&percent_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{signing_name}.amazonaws.com/"))
            .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/\
                     {signing_name}/aws4_request, SignedHeaders=host, Signature=00"
                ),
            )
            .body(bytes::Bytes::from(body))
            .expect("a well-formed awsQuery request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        (
            status,
            String::from_utf8_lossy(response.body()).to_string(),
        )
    }

    /// One SNS call signed by `access_key_id`.
    async fn sns(
        &self,
        access_key_id: &str,
        action: &str,
        params: &[(String, String)],
    ) -> (u16, String) {
        self.query(access_key_id, "sns", SNS_VERSION, action, params)
            .await
    }

    /// One IAM fixture call, signed with the account's own credential; it must succeed.
    async fn iam(&self, action: &str, params: &[(&str, String)]) -> String {
        let owned: Vec<(String, String)> = params
            .iter()
            .map(|(name, value)| ((*name).to_string(), value.clone()))
            .collect();
        let (status, body) = self
            .query(ACCOUNT_KEY, "iam", IAM_VERSION, action, &owned)
            .await;
        assert_eq!(
            status, 200,
            "the fixture {action} must succeed, got {status}: {body}"
        );
        body
    }

    /// An IAM user with a long-term access key and no policy at all.
    async fn user(&self, user_name: &str) -> Principal {
        let created = self
            .iam("CreateUser", &[("UserName", user_name.to_string())])
            .await;
        let arn = xml_element(&created, "Arn")
            .unwrap_or_else(|| panic!("CreateUser answers the user's Arn, got {created}"));
        let minted = self
            .iam("CreateAccessKey", &[("UserName", user_name.to_string())])
            .await;
        let access_key_id = xml_element(&minted, "AccessKeyId")
            .unwrap_or_else(|| panic!("CreateAccessKey answers an AccessKeyId, got {minted}"));
        Principal {
            user_name: user_name.to_string(),
            access_key_id,
            arn,
        }
    }

    /// Write the one inline policy of `principal`, under the same policy name every time, so
    /// a write REPLACES the document the principal carried.
    async fn put_user_policy(&self, principal: &Principal, document: &str) {
        self.iam(
            "PutUserPolicy",
            &[
                ("UserName", principal.user_name.clone()),
                ("PolicyName", "inline".to_string()),
                ("PolicyDocument", document.to_string()),
            ],
        )
        .await;
    }

    /// Remove that inline policy again, leaving the principal with no identity-based grant.
    async fn delete_user_policy(&self, principal: &Principal) {
        self.iam(
            "DeleteUserPolicy",
            &[
                ("UserName", principal.user_name.clone()),
                ("PolicyName", "inline".to_string()),
            ],
        )
        .await;
    }

    /// A topic created by the account's own credential, with its ARN.
    async fn topic(&self, name: &str) -> String {
        let (status, body) = self
            .sns(ACCOUNT_KEY, "CreateTopic", &[param("Name", name)])
            .await;
        assert_eq!(
            status, 200,
            "the fixture CreateTopic {name} must succeed, got {status}: {body}"
        );
        xml_element(&body, "TopicArn")
            .unwrap_or_else(|| panic!("CreateTopic answers a TopicArn, got {body}"))
    }

    /// Write a topic's access policy, as its owner does.
    async fn set_topic_policy(&self, topic_arn: &str, document: &str) {
        let (status, body) = self
            .sns(
                ACCOUNT_KEY,
                "SetTopicAttributes",
                &[
                    param("TopicArn", topic_arn),
                    param("AttributeName", "Policy"),
                    param("AttributeValue", document),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "the fixture SetTopicAttributes must succeed, got {status}: {body}"
        );
    }

    /// The `Policy` attribute the topic reports, read by its owner.
    async fn topic_policy(&self, topic_arn: &str) -> String {
        let (status, body) = self
            .sns(
                ACCOUNT_KEY,
                "GetTopicAttributes",
                &[param("TopicArn", topic_arn)],
            )
            .await;
        assert_eq!(
            status, 200,
            "the fixture GetTopicAttributes must succeed, got {status}: {body}"
        );
        // The `Policy` entry of the attribute map: its `<value>` is the element that follows
        // the `<key>Policy</key>` of its own entry.
        match body.find("<key>Policy</key>") {
            Some(start) => xml_element(&body[start..], "value").unwrap_or_default(),
            None => String::new(),
        }
    }
}

/// One form parameter.
fn param(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// The text of the first `<tag>…</tag>` element of `body`, or `None` when the response
/// carries none — the sibling harness's own scrape of an element ([`text_of`], which decodes
/// the XML entities the envelope escapes), read as the presence question the assertions below
/// ask.
fn xml_element(body: &str, tag: &str) -> Option<String> {
    body.contains(&format!("<{tag}>"))
        .then(|| text_of(body, tag))
}

/// The awsQuery error code of a refusal, or `""` on a success envelope.
fn error_code(body: &str) -> String {
    xml_element(body, "Code").unwrap_or_default()
}

/// The message of a refusal, or `""` on a success envelope.
fn error_message(body: &str) -> String {
    xml_element(body, "Message").unwrap_or_default()
}

/// One topic-addressed operation: the IAM action it performs, the operation name, and the
/// parameters it carries beside `TopicArn`.
struct Call {
    action: &'static str,
    operation: &'static str,
    extra: Vec<(String, String)>,
}

/// Every operation that acts on the topic the request names, each with the IAM action it
/// performs.
///
/// The eight actions are the ones a topic's own access policy can grant — the set the policy
/// SNS generates for a topic with none enumerates — plus `PublishBatch`, which performs the
/// same `SNS:Publish` action as `Publish` because the SNS action namespace publishes none of
/// its own for it ("Actions defined by Amazon Simple Notification Service", IAM Service
/// Authorization Reference).
fn topic_calls(account: &str) -> Vec<Call> {
    vec![
        Call {
            action: "SNS:Publish",
            operation: "Publish",
            extra: vec![param("Message", "a message")],
        },
        Call {
            action: "SNS:Publish",
            operation: "PublishBatch",
            extra: vec![
                param("PublishBatchRequestEntries.member.1.Id", "one"),
                param("PublishBatchRequestEntries.member.1.Message", "a message"),
            ],
        },
        Call {
            action: "SNS:GetTopicAttributes",
            operation: "GetTopicAttributes",
            extra: Vec::new(),
        },
        Call {
            action: "SNS:SetTopicAttributes",
            operation: "SetTopicAttributes",
            extra: vec![
                param("AttributeName", "DisplayName"),
                param("AttributeValue", "gated"),
            ],
        },
        Call {
            action: "SNS:Subscribe",
            operation: "Subscribe",
            extra: vec![
                param("Protocol", "sqs"),
                param(
                    "Endpoint",
                    &format!("arn:aws:sqs:{REGION}:{account}:authorization-gate"),
                ),
                param("ReturnSubscriptionArn", "true"),
            ],
        },
        Call {
            action: "SNS:ListSubscriptionsByTopic",
            operation: "ListSubscriptionsByTopic",
            extra: Vec::new(),
        },
        Call {
            action: "SNS:AddPermission",
            operation: "AddPermission",
            extra: vec![
                param("Label", "delegated"),
                param("AWSAccountId.member.1", account),
                param("ActionName.member.1", "Publish"),
            ],
        },
        Call {
            action: "SNS:RemovePermission",
            operation: "RemovePermission",
            extra: vec![param("Label", "delegated")],
        },
        Call {
            action: "SNS:DeleteTopic",
            operation: "DeleteTopic",
            extra: Vec::new(),
        },
    ]
}

/// The parameters one call carries, for the topic `topic_arn`.
fn parameters(call: &Call, topic_arn: &str) -> Vec<(String, String)> {
    let mut params = vec![param("TopicArn", topic_arn)];
    params.extend(call.extra.iter().cloned());
    params
}

/// An identity-based document allowing `action` on `resource`, in the spelling an IAM policy
/// uses for an SNS action (lowercase namespace; IAM compares action names
/// case-insensitively).
fn identity_allow(action: &str, resource: &str) -> String {
    let action = action.replace("SNS:", "sns:");
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":"{action}","Resource":"{resource}"}}]}}"#
    )
}

/// RULE, over every topic-addressed operation: with no grant anywhere the call is refused
/// `403 AuthorizationError`, naming the principal, the action and the topic; the identical
/// call is served once the caller's identity policy allows that action on that topic; and a
/// grant of another action, or of the same action on another topic, allows nothing.
#[tokio::test]
async fn every_topic_action_needs_a_grant_and_is_served_by_one() {
    let emulator = Emulator::new();
    let account = ::engine::emulator_account_id().to_string();
    let principal = emulator.user("topic-actions").await;
    let elsewhere = format!("arn:aws:sns:{REGION}:{account}:another-topic");

    for (index, call) in topic_calls(&account).into_iter().enumerate() {
        let case = call.operation;
        // One topic per row, so a row that deletes or rewrites the topic cannot decide the
        // next row's answer.
        let topic = emulator.topic(&format!("topic-action-{index:02}")).await;
        let params = parameters(&call, &topic);

        // No identity policy, and the topic carries the policy SNS generates for a topic
        // whose owner set none: nothing allows the call.
        let (status, body) = emulator.sns(&principal.access_key_id, case, &params).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            AUTHORIZATION_ERROR,
            "case {case:?}: a call with no grant must be refused, got {status}: {body}"
        );
        assert_eq!(
            error_message(&body),
            not_authorized(
                &principal.arn,
                call.action,
                &topic,
                &no_identity_policy_allows(call.action)
            ),
            "case {case:?}: unexpected sentence, got {body}"
        );

        // A grant of ANOTHER action on this topic, and of THIS action on another topic:
        // neither covers the call.
        for (reason, document) in [
            (
                "a grant of another action",
                identity_allow("SNS:ListTopics", &topic),
            ),
            (
                "a grant on another topic",
                identity_allow(call.action, &elsewhere),
            ),
        ] {
            emulator.put_user_policy(&principal, &document).await;
            let (status, body) = emulator.sns(&principal.access_key_id, case, &params).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                AUTHORIZATION_ERROR,
                "case {case:?} with {reason:?}: the call must still be refused, got \
                 {status}: {body}"
            );
        }

        // The grant the call needs: the same request is served.
        emulator
            .put_user_policy(&principal, &identity_allow(call.action, &topic))
            .await;
        let (status, body) = emulator.sns(&principal.access_key_id, case, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the granted call must be served, got {status}: {body}"
        );

        // And the grant is what served it: removing it refuses the identical call again.
        emulator.delete_user_policy(&principal).await;
        let (status, body) = emulator.sns(&principal.access_key_id, case, &params).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            AUTHORIZATION_ERROR,
            "case {case:?}: the call must be refused once the grant is removed, got \
             {status}: {body}"
        );
    }
}

/// RULE: the topic's own policy is the other half of the decision — a statement naming the
/// principal grants the call with no identity policy at all, a statement naming the ACCOUNT
/// only delegates to that account's identity policies, and an explicit `Deny` refuses
/// whatever the identity policy allows.
#[tokio::test]
async fn the_topic_policy_grants_by_principal_and_denies_over_any_allow() {
    let emulator = Emulator::new();
    let account = ::engine::emulator_account_id().to_string();
    let principal = emulator.user("topic-policy-reader").await;
    let root = format!("arn:aws:iam::{account}:root");

    // One row: the case, the topic policy to write (none leaves the generated default in
    // place), the caller's identity policy, and the clause a refusal must carry — each
    // document built for the row's own topic, because a policy names the topic it governs.
    type Document = Box<dyn Fn(&str) -> Option<String>>;
    let no_document: fn() -> Document = || Box::new(|_topic: &str| None);
    let rows: Vec<(&str, Document, Document, Option<String>)> = vec![
        (
            "the generated default policy, with no identity policy",
            no_document(),
            no_document(),
            Some(no_identity_policy_allows("SNS:Publish")),
        ),
        (
            "a topic policy naming the principal",
            {
                let who = principal.arn.clone();
                Box::new(move |topic| {
                    Some(resource_statement("Allow", &who, "SNS:Publish", topic))
                })
            },
            no_document(),
            None,
        ),
        (
            "a topic policy naming the account, with no identity policy",
            {
                let who = root.clone();
                Box::new(move |topic| {
                    Some(resource_statement("Allow", &who, "SNS:Publish", topic))
                })
            },
            no_document(),
            Some(no_identity_policy_allows("SNS:Publish")),
        ),
        (
            "a topic policy naming the account, with an identity policy",
            {
                let who = root.clone();
                Box::new(move |topic| {
                    Some(resource_statement("Allow", &who, "SNS:Publish", topic))
                })
            },
            Box::new(|_topic| Some(identity_allow("sns:*", "*"))),
            None,
        ),
        (
            "a topic policy denying the principal, beside an identity allow",
            {
                let who = principal.arn.clone();
                Box::new(move |topic| {
                    Some(resource_statement("Deny", &who, "SNS:Publish", topic))
                })
            },
            Box::new(|_topic| Some(identity_allow("sns:*", "*"))),
            Some(RESOURCE_POLICY_DENY.to_string()),
        ),
        (
            "a topic policy denying every principal, beside an identity allow",
            Box::new(|topic| Some(resource_statement("Deny", "*", "SNS:*", topic))),
            Box::new(|_topic| Some(identity_allow("sns:*", "*"))),
            Some(RESOURCE_POLICY_DENY.to_string()),
        ),
        (
            "an identity policy denying the action, beside a topic-policy allow",
            {
                let who = principal.arn.clone();
                Box::new(move |topic| {
                    Some(resource_statement("Allow", &who, "SNS:Publish", topic))
                })
            },
            Box::new(|topic| {
                Some(format!(
                    r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":"sns:*",
                    "Resource":"*"}},{{"Effect":"Deny","Action":"sns:Publish","Resource":"{topic}"}}]}}"#
                ))
            }),
            Some(IDENTITY_POLICY_DENY.to_string()),
        ),
    ];

    for (index, (case, policy, identity, expected)) in rows.into_iter().enumerate() {
        // One topic per row: a policy that denies every principal locks the topic for its
        // owner too, exactly as it does on AWS, so no row may inherit another's document.
        let topic = emulator.topic(&format!("topic-policy-{index:02}")).await;
        let identity = identity(&topic);
        if let Some(policy) = policy(&topic) {
            emulator.set_topic_policy(&topic, &policy).await;
        }
        // The generated default is what a topic whose owner wrote no policy reports, so the
        // row that leaves it in place is asserted to have it.
        else {
            let reported = emulator.topic_policy(&topic).await;
            assert!(
                reported.contains("__default_policy_ID"),
                "case {case:?}: a topic with no policy of its own reports the generated \
                 default, got {reported}"
            );
        }
        match identity.as_deref() {
            Some(document) => emulator.put_user_policy(&principal, document).await,
            None => {
                // The account holds at most one inline policy here, written under one name,
                // so a row that carries none starts from a principal with nothing.
                let (status, _) = emulator
                    .query(
                        ACCOUNT_KEY,
                        "iam",
                        IAM_VERSION,
                        "DeleteUserPolicy",
                        &[
                            param("UserName", &principal.user_name),
                            param("PolicyName", "inline"),
                        ],
                    )
                    .await;
                assert!(
                    status == 200 || status == 404,
                    "case {case:?}: clearing the inline policy answered {status}"
                );
            }
        }
        let (status, body) = emulator
            .sns(
                &principal.access_key_id,
                "Publish",
                &[param("TopicArn", &topic), param("Message", "a message")],
            )
            .await;
        match expected {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: the call must be served, got {status}: {body}"
                );
                assert!(
                    xml_element(&body, "MessageId").is_some(),
                    "case {case:?}: a served publish answers a MessageId, got {body}"
                );
            }
            Some(clause) => {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    AUTHORIZATION_ERROR,
                    "case {case:?}: the call must be refused, got {status}: {body}"
                );
                assert_eq!(
                    error_message(&body),
                    not_authorized(&principal.arn, "SNS:Publish", &topic, &clause),
                    "case {case:?}: unexpected sentence, got {body}"
                );
            }
        }
    }
}

/// RULE: the decision is taken before the operation touches anything, so a refused call
/// changes no state and reports nothing about what the account holds — including whether the
/// topic it named exists.
#[tokio::test]
async fn a_refused_call_changes_nothing_and_reports_nothing() {
    let emulator = Emulator::new();
    let account = ::engine::emulator_account_id().to_string();
    let principal = emulator.user("no-side-effects").await;
    let topic = emulator.topic("no-side-effects").await;

    // A topic that does not exist is answered the same way as one that does: an unauthorized
    // caller learns nothing about the account's records.
    let absent = format!("arn:aws:sns:{REGION}:{account}:never-created");
    for (case, arn) in [("an existing topic", &topic), ("no such topic", &absent)] {
        let (status, body) = emulator
            .sns(
                &principal.access_key_id,
                "Publish",
                &[param("TopicArn", arn), param("Message", "a message")],
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            AUTHORIZATION_ERROR,
            "case {case:?}: expected the authorization refusal, got {status}: {body}"
        );
        assert_eq!(
            error_message(&body),
            not_authorized(
                &principal.arn,
                "SNS:Publish",
                arn,
                &no_identity_policy_allows("SNS:Publish")
            ),
            "case {case:?}: unexpected sentence, got {body}"
        );
    }

    // A refused write leaves the record exactly as it was: the display name is not set, the
    // policy is not rewritten, and the topic is still there after a refused delete.
    let (status, body) = emulator
        .sns(
            &principal.access_key_id,
            "SetTopicAttributes",
            &[
                param("TopicArn", &topic),
                param("AttributeName", "DisplayName"),
                param("AttributeValue", "unauthorized"),
            ],
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        AUTHORIZATION_ERROR,
        "a refused write expects the authorization refusal, got {status}: {body}"
    );
    let (status, body) = emulator
        .sns(&principal.access_key_id, "DeleteTopic", &[param("TopicArn", &topic)])
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        AUTHORIZATION_ERROR,
        "a refused delete expects the authorization refusal, got {status}: {body}"
    );
    let (status, body) = emulator
        .sns(
            ACCOUNT_KEY,
            "GetTopicAttributes",
            &[param("TopicArn", &topic)],
        )
        .await;
    assert_eq!(
        status, 200,
        "the topic must survive the refused delete, got {status}: {body}"
    );
    assert!(
        !body.contains("unauthorized"),
        "the refused write must have set no attribute, got {body}"
    );
}

/// A resource-based statement: `effect` for `principal` on `action` of `resource`, in the
/// shape a topic's `Policy` attribute carries one.
fn resource_statement(effect: &str, principal: &str, action: &str, resource: &str) -> String {
    format!(
        r#"{{"Version":"2008-10-17","Id":"authorization-gate","Statement":[{{"Sid":"gate",
        "Effect":"{effect}","Principal":{{"AWS":"{principal}"}},"Action":"{action}",
        "Resource":"{resource}"}}]}}"#
    )
}
