// SPDX-License-Identifier: CC0-1.0

//! What authorizes an object call made with a credential whose MINT scoped it: a session
//! policy, and the session tags the mint attached.
//!
//! The rule under test: the permissions of a temporary credential are not the role's alone.
//! "The resulting session's permissions are the intersection of the role's identity-based
//! policy and the session policies… You cannot use session policies to grant more
//! permissions than those allowed by the identity-based policy of the role that is being
//! assumed" (`AssumeRoleRequest$PolicyArns`, the STS model) — so a call the session policy
//! does not allow is refused even where the role's own policy allows it, and an explicit
//! `Deny` in the session policy refuses it whatever any grant says. And the session's TAGS
//! are what `aws:PrincipalTag/<key>` holds for a request the credential signs
//! ("`aws:PrincipalTag/tag-key` — the tag attached to the principal making the request"),
//! so an attribute-based grant written over one of them decides the call: the value the
//! session carries, or no value at all for a tag it does not carry.
//!
//! Every credential here is minted by STS's OWN `AssumeRole` — with its `Policy` and its
//! `Tags` — over the same shared engine, and every S3 request is presented with that
//! session's access key id. Nothing reaches into a store to plant a scope, a tag or a
//! decision.
//!
//! The tables span what the rule generalises over: the ACTION a session policy allows
//! against the ones it leaves out, the KEY it is written over against another key of the
//! same bucket, an explicit `Deny` beside a grant, a session minted with NO policy at all
//! (capped by nothing), and — for the tags — a session whose tag matches the grant's
//! condition, one whose tag does not, and one carrying no tag at all.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::Answer;
// The encoder every suite of this test binary that drives an awsQuery service uses, and
// the policy/ARN/refusal spellings the role-session suite already states: reused rather
// than copied again, so two suites cannot word the same thing differently.
use super::identity_authorization::form_encode;
use super::role_session_object_authorization::{
    denial_message, identity_document, object_arn, xml_element,
};

/// The Region every request in this suite is signed for.
const REGION: &str = "us-east-1";

/// The access-key id standing for the account's own published credential, which writes this
/// suite's fixtures.
const ACCOUNT_KEY: &str = "AKIDEMULATOR";

/// The bytes every object in this suite holds.
const BODY: &[u8] = b"session-scoped-subject";

/// The code S3 answers an authorization refusal with.
const ACCESS_DENIED: &str = "AccessDenied";

/// The IAM action a read of the current version of an object is authorized under.
const GET_OBJECT: &str = "s3:GetObject";

/// The IAM action an object write is authorized under.
const PUT_OBJECT: &str = "s3:PutObject";

/// The tag key the attribute-based grant in this suite is written over, and the value that
/// grant admits.
const TEAM_TAG: &str = "team";
const ADMITTED_TEAM: &str = "payments";

/// An emulator holding S3 beside the two services a decision about a session's call reads
/// from: IAM (the role and its identity policies) and STS (the session a role is assumed
/// into, with the scope and the tags its mint passed).
struct Emulator {
    router: MultiServiceRouter,
}

/// One role session a row calls as.
struct Session {
    /// The session's access key id, which is where this emulator reads WHO is calling.
    access_key_id: String,
    /// The assumed-role session ARN `sts:GetCallerIdentity` reports, which a refusal names.
    session_arn: String,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing: the `Authorization` header carries only the credential
        // scope, which is where a service reads the Region and the caller's access key id.
        router.set_request_authentication(false);
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
                engine,
            ),
        );
        Self { router }
    }

    /// One S3 request signed with `access_key_id` in its SigV4 credential scope.
    async fn s3(&self, access_key_id: &str, method: &str, uri: &str, body: &[u8]) -> Answer {
        let host = format!("s3.{REGION}.amazonaws.com");
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{uri}"))
            .header("host", host.clone())
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/s3/\
                     aws4_request, SignedHeaders=host, Signature=0000"
                ),
            )
            .body(bytes::Bytes::copy_from_slice(body))
            .expect("the test request must build");
        let response = self.router.dispatch(request).await;
        let (parts, body) = response.into_parts();
        Answer {
            status: parts.status.as_u16(),
            headers: parts.headers,
            body,
        }
    }

    /// One awsQuery call to IAM or STS, as the account's own credential.
    async fn query(
        &self,
        signing_name: &str,
        version: &str,
        action: &str,
        params: &[(&str, String)],
    ) -> (u16, String) {
        let mut body = format!("Action={action}&Version={version}");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{signing_name}.amazonaws.com/"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/\
                     {signing_name}/aws4_request, SignedHeaders=host, Signature=0000"
                ),
            )
            .body(bytes::Bytes::from(body))
            .expect("a well-formed awsQuery request");
        let response = self.router.dispatch(request).await;
        (
            response.status().as_u16(),
            String::from_utf8_lossy(response.body()).to_string(),
        )
    }

    /// Create an IAM role carrying `policy` as its inline policy.
    async fn create_role(&self, role_name: &str, policy: &str) -> String {
        let account = engine::emulator_account_id();
        let role_arn = format!("arn:aws:iam::{account}:role/{role_name}");
        let trust = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": {"AWS": format!("arn:aws:iam::{account}:root")},
                // `sts:TagSession` beside the assume: "to pass session tags, the principal
                // must have the `sts:TagSession` permission" (`AssumeRole$Tags`), so a role
                // whose sessions carry tags admits both actions.
                "Action": ["sts:AssumeRole", "sts:TagSession"],
            }],
        })
        .to_string();
        let (status, body) = self
            .query(
                "iam",
                "2010-05-08",
                "CreateRole",
                &[
                    ("RoleName", role_name.to_string()),
                    ("AssumeRolePolicyDocument", trust),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "creating the role {role_name:?} must succeed, got {status} {body}"
        );
        let (status, body) = self
            .query(
                "iam",
                "2010-05-08",
                "PutRolePolicy",
                &[
                    ("RoleName", role_name.to_string()),
                    ("PolicyName", "inline".to_string()),
                    ("PolicyDocument", policy.to_string()),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "putting the inline policy of {role_name:?} must succeed, got {status} {body}"
        );
        role_arn
    }

    /// Assume `role_arn` through STS's own `AssumeRole`, passing the session policy
    /// `policy` and the session tags `tags` the mint is to attach.
    async fn assume(
        &self,
        case: &str,
        role_arn: &str,
        session_name: &str,
        policy: Option<&str>,
        tags: &[(&str, &str)],
    ) -> Session {
        let mut params = vec![
            ("RoleArn", role_arn.to_string()),
            ("RoleSessionName", session_name.to_string()),
        ];
        if let Some(policy) = policy {
            params.push(("Policy", policy.to_string()));
        }
        // The `Tags` member of `AssumeRole`, as awsQuery flattens a list of structures.
        let mut tag_params: Vec<(String, String)> = Vec::new();
        for (index, (key, value)) in tags.iter().enumerate() {
            tag_params.push((format!("Tags.member.{}.Key", index + 1), (*key).to_string()));
            tag_params.push((
                format!("Tags.member.{}.Value", index + 1),
                (*value).to_string(),
            ));
        }
        let mut borrowed: Vec<(&str, String)> = params;
        for (name, value) in &tag_params {
            borrowed.push((name.as_str(), value.clone()));
        }
        let (status, body) = self
            .query("sts", "2011-06-15", "AssumeRole", &borrowed)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: assuming {role_arn:?} must succeed, got {status} {body}"
        );
        Session {
            access_key_id: xml_element(&body, "AccessKeyId").unwrap_or_else(|| {
                panic!("case {case:?}: AssumeRole must answer an AccessKeyId, got {body}")
            }),
            session_arn: xml_element(&body, "Arn").unwrap_or_else(|| {
                panic!("case {case:?}: AssumeRole must answer the session Arn, got {body}")
            }),
        }
    }

    /// `PUT /<bucket>` as the account.
    async fn create_bucket(&self, bucket: &str) {
        let created = self.s3(ACCOUNT_KEY, "PUT", &format!("/{bucket}"), b"").await;
        assert_eq!(
            created.status,
            200,
            "the bucket must be created first, got {} with {}",
            created.status,
            body_text(&created)
        );
    }

    /// `PUT /<bucket>/<key>` as the account — a fixture every read row addresses.
    async fn write_fixture(&self, bucket: &str, key: &str) {
        let written = self
            .s3(ACCOUNT_KEY, "PUT", &format!("/{bucket}/{key}"), BODY)
            .await;
        assert_eq!(
            written.status,
            200,
            "the fixture {key:?} must be stored first, got {} with {}",
            written.status,
            body_text(&written)
        );
    }
}

/// An answer's body as text, with the XML entities S3 escapes a message's quotes as read
/// back, so an expected sentence can be compared as it was worded.
fn body_text(answer: &Answer) -> String {
    String::from_utf8_lossy(&answer.body)
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
}

/// The clause IAM publishes for a call the SESSION POLICY of the credential does not allow.
fn no_session_grant(action: &str) -> String {
    format!("because no session policy allows the {action} action")
}

/// The clause IAM publishes for a call a session policy explicitly denies.
const SESSION_DENY: &str = "with an explicit deny in a session policy";

/// The clause IAM publishes for a call no identity-based policy allows.
fn no_identity_grant(action: &str) -> String {
    format!("because no identity-based policy allows the {action} action")
}

/// Assert an S3 answer is the refusal `reason` words, in S3's own `AccessDenied` envelope.
fn assert_refused(case: &str, answer: &Answer, principal: &str, action: &str, resource: &str, reason: &str) {
    let text = body_text(answer);
    assert_eq!(
        answer.status, 403,
        "case {case:?}: expected HTTP 403, got {} with {text}",
        answer.status
    );
    assert!(
        text.contains(ACCESS_DENIED),
        "case {case:?}: expected the {ACCESS_DENIED} code, got {text}"
    );
    let expected = denial_message(principal, action, resource, reason);
    assert!(
        text.contains(&expected),
        "case {case:?}: expected the message {expected:?}, got {text}"
    );
}

// ---------------------------------------------------------------------------
// RULE: a session policy CAPS what the role's own policies grant — the permissions of the
// credential are the intersection — and an explicit deny in one refuses the call whatever
// grants it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_session_policy_caps_what_the_role_allows() {
    let emulator = Emulator::new();
    let bucket = "session-scope-bucket";
    emulator.create_bucket(bucket).await;
    emulator.write_fixture(bucket, "scoped").await;
    emulator.write_fixture(bucket, "other").await;

    // The role allows both actions over every object of the bucket, so every refusal below
    // is the SESSION's scope talking and never the role's policy.
    let role_policy = identity_document(vec![json!({
        "Effect": "Allow",
        "Action": [GET_OBJECT, PUT_OBJECT],
        "Resource": format!("arn:aws:s3:::{bucket}/*"),
    })]);
    let role_arn = emulator.create_role("session-scope-role", &role_policy).await;

    /// One row: the session policy the mint passed, the call made with the credential, and
    /// what the intersection of the role's policy and that scope leaves of it.
    struct Row {
        label: &'static str,
        /// `None` is a session the mint scoped with nothing, which is capped by nothing.
        policy: Option<Value>,
        method: &'static str,
        key: &'static str,
        action: &'static str,
        /// `None` is a call the intersection leaves to the session; `Some(reason)` the
        /// clause its refusal carries.
        refused_with: Option<String>,
    }

    let rows = [
        Row {
            label: "no session policy at all, so the role's grant stands",
            policy: None,
            method: "GET",
            key: "scoped",
            action: GET_OBJECT,
            refused_with: None,
        },
        Row {
            label: "a session policy that allows exactly this read",
            policy: Some(json!({
                "Effect": "Allow",
                "Action": GET_OBJECT,
                "Resource": object_arn(bucket, "scoped"),
            })),
            method: "GET",
            key: "scoped",
            action: GET_OBJECT,
            refused_with: None,
        },
        Row {
            label: "a session policy written over another key",
            policy: Some(json!({
                "Effect": "Allow",
                "Action": GET_OBJECT,
                "Resource": object_arn(bucket, "scoped"),
            })),
            method: "GET",
            key: "other",
            action: GET_OBJECT,
            refused_with: Some(no_session_grant(GET_OBJECT)),
        },
        Row {
            label: "a session policy that leaves the write out",
            policy: Some(json!({
                "Effect": "Allow",
                "Action": GET_OBJECT,
                "Resource": format!("arn:aws:s3:::{bucket}/*"),
            })),
            method: "PUT",
            key: "scoped",
            action: PUT_OBJECT,
            refused_with: Some(no_session_grant(PUT_OBJECT)),
        },
        Row {
            label: "a session policy that explicitly denies the read it also grants",
            policy: Some(json!({
                "Effect": "Deny",
                "Action": GET_OBJECT,
                "Resource": format!("arn:aws:s3:::{bucket}/*"),
            })),
            method: "GET",
            key: "scoped",
            action: GET_OBJECT,
            refused_with: Some(SESSION_DENY.to_string()),
        },
        Row {
            label: "a session policy over the whole service, which allows both",
            policy: Some(json!({
                "Effect": "Allow",
                "Action": "s3:*",
                "Resource": "*",
            })),
            method: "PUT",
            key: "other",
            action: PUT_OBJECT,
            refused_with: None,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let label = row.label;
        let policy = row
            .policy
            .clone()
            .map(|statement| identity_document(vec![statement]));
        let session = emulator
            .assume(
                label,
                &role_arn,
                &format!("scope-{index}"),
                policy.as_deref(),
                &[],
            )
            .await;
        let answer = emulator
            .s3(
                &session.access_key_id,
                row.method,
                &format!("/{bucket}/{}", row.key),
                BODY,
            )
            .await;
        match &row.refused_with {
            None => assert!(
                (200..300).contains(&answer.status),
                "case {label:?}: the intersection must leave this call to the session, got {} \
                 with {}",
                answer.status,
                body_text(&answer)
            ),
            Some(reason) => assert_refused(
                label,
                &answer,
                &session.session_arn,
                row.action,
                &object_arn(bucket, row.key),
                reason,
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: the session's TAGS are what `aws:PrincipalTag/<key>` holds for a request the
// credential signs, so an attribute-based grant written over one of them decides the call.
// ---------------------------------------------------------------------------

/// One row of the tag table: the tags the session is minted with, and whether the grant's
/// condition then holds.
struct TagRow {
    label: &'static str,
    tags: &'static [(&'static str, &'static str)],
    allowed: bool,
}

#[tokio::test]
async fn an_attribute_based_grant_is_decided_by_the_sessions_tags() {
    let emulator = Emulator::new();
    let bucket = "session-tag-bucket";
    emulator.create_bucket(bucket).await;
    emulator.write_fixture(bucket, "abac").await;

    // The role's ONLY grant is conditioned on the session tag, so the decision is the tag's.
    let role_policy = identity_document(vec![json!({
        "Effect": "Allow",
        "Action": GET_OBJECT,
        "Resource": format!("arn:aws:s3:::{bucket}/*"),
        "Condition": {
            "StringEquals": { format!("aws:PrincipalTag/{TEAM_TAG}"): ADMITTED_TEAM },
        },
    })]);
    let role_arn = emulator.create_role("session-tag-role", &role_policy).await;

    // The rows differ along what the rule generalises over: the tag the session carries
    // (the admitted value, another value, no tag at all), and a session carrying the
    // admitted value under a DIFFERENT key, which the condition does not read.
    let rows = [
        TagRow {
            label: "the session carries the admitted tag",
            tags: &[(TEAM_TAG, ADMITTED_TEAM)],
            allowed: true,
        },
        TagRow {
            label: "the session carries another value for it",
            tags: &[(TEAM_TAG, "logistics")],
            allowed: false,
        },
        TagRow {
            label: "the session carries no tag at all",
            tags: &[],
            allowed: false,
        },
        TagRow {
            label: "the session carries the value under another key",
            tags: &[("squad", ADMITTED_TEAM)],
            allowed: false,
        },
        TagRow {
            label: "the session carries the admitted tag beside others",
            tags: &[("squad", "logistics"), (TEAM_TAG, ADMITTED_TEAM)],
            allowed: true,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let label = row.label;
        let session = emulator
            .assume(label, &role_arn, &format!("tagged-{index}"), None, row.tags)
            .await;
        let answer = emulator
            .s3(&session.access_key_id, "GET", &format!("/{bucket}/abac"), b"")
            .await;
        if row.allowed {
            assert_eq!(
                answer.status,
                200,
                "case {label:?}: the grant's condition holds, so the read must be served, got \
                 {} with {}",
                answer.status,
                body_text(&answer)
            );
            assert_eq!(
                answer.body.as_ref(),
                BODY,
                "case {label:?}: expected the stored bytes, got {}",
                body_text(&answer)
            );
        } else {
            assert_refused(
                label,
                &answer,
                &session.session_arn,
                GET_OBJECT,
                &object_arn(bucket, "abac"),
                &no_identity_grant(GET_OBJECT),
            );
        }
    }
}
