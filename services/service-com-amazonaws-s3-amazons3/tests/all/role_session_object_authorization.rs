// SPDX-License-Identifier: CC0-1.0

//! What authorizes an object call made with the credentials of an ASSUMED-ROLE SESSION.
//!
//! The rule under test: an object operation presented with a credential is authorized, per
//! action and before the handler reads or writes anything, against the documents in force on
//! the principal that credential speaks for — for a session, the identity-based policies of
//! the role it is a session of — and against the bucket's own resource policy; an explicit
//! `Deny` about the request in either refuses it, a request neither allows is refused as
//! well, and the refusal is HTTP 403 `AccessDenied` carrying IAM's sentence for the document
//! that decided it. "Amazon S3 evaluates all of the relevant access policies, user policies,
//! and resource-based policies in deciding whether to authorize the request" (Amazon S3 User
//! Guide, *Identity and Access Management for Amazon S3*), and "an explicit deny in any
//! policy overrides any allows" (IAM User Guide, *Policy evaluation logic*). A refused call
//! is also a call that did not happen: a refused write leaves no object behind and a refused
//! delete leaves the version it addressed readable.
//!
//! Every assertion drives real HTTP requests through a router holding S3 beside IAM and STS
//! over ONE shared engine, so the role, its inline policy and the session are made with
//! those services' own operations (`CreateRole` + `PutRolePolicy` + `AssumeRole`) and each S3
//! request is presented with the session's access key id — where this emulator reads which
//! principal a request speaks for. Nothing reaches into a store to plant a policy or a
//! decision.
//!
//! The tables span what the rule generalises over rather than one recorded call:
//!
//! * the PRINCIPAL spelling a resource-policy statement reaches the session by — the session
//!   ARN STS reports, the ARN of the role behind it, the account's own root (which denies
//!   every principal of the account), the wildcard, and another role's ARN, which reaches it
//!   not at all;
//! * the ACTION element — the action itself, a prefix wildcard over it, the whole service,
//!   and a list naming the read beside the write;
//! * the RESOURCE element — one key, a key prefix, every object of the bucket, and another
//!   prefix, which the call does not address;
//! * the OPERATION — `GetObject`, `HeadObject`, `PutObject` and `DeleteObject`, each judged
//!   under its own IAM action, and the two operations that READ A SOURCE object (`CopyObject`,
//!   whose destination write is judged beside it, and `UploadPartCopy`), where the action is
//!   the one the source reference asks for;
//! * and, for the identity-based half, what the role's own inline policy reaches: the action
//!   itself, the service wildcard, an explicit deny beside the grant, a grant over another
//!   key, and no policy at all.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::Answer;

/// The Region every request in this suite is signed for, and the endpoint it is addressed to.
const REGION: &str = "us-east-1";

/// The access-key id standing for the account's own published credential: neither STS nor IAM
/// holds a record of it, so it resolves to the account's ambient identity — the position AWS
/// account root credentials are in. It writes this suite's fixtures.
const ACCOUNT_KEY: &str = "AKIDEMULATOR";

/// The bytes every object in this suite holds. A policy is about who may reach an object, not
/// about what is in it, so one payload serves every row.
const BODY: &[u8] = b"role-session-authorized-subject";

/// The code S3 answers an authorization refusal with, and the status it carries (Amazon S3
/// "Error responses": `AccessDenied`, HTTP 403).
const ACCESS_DENIED: &str = "AccessDenied";

/// The IAM action a read of the current version of an object is authorized under: "to use
/// `GetObject`, you must have the `READ` access to the object" — `s3:GetObject`
/// (`GetObject`'s own documentation of its general purpose bucket permissions), which a HEAD
/// of the same object asks for too ("a `HEAD` request has the same options as a `GET`
/// operation on an object", `HeadObject`).
const GET_OBJECT: &str = "s3:GetObject";

/// The IAM action a read that NAMES a version is authorized under: "if you include a
/// `versionId` in your request header, you must have the `s3:GetObjectVersion` permission to
/// access a specific version of an object. The `s3:GetObject` permission is not required in
/// this scenario" (`GetObject`'s own documentation) — which is what a copy whose source
/// reference names a version asks for too.
const GET_OBJECT_VERSION: &str = "s3:GetObjectVersion";

/// The IAM action an object write is authorized under ("you must have the `s3:PutObject`
/// permission", `PutObject`).
const PUT_OBJECT: &str = "s3:PutObject";

/// The IAM action a delete that names no version is authorized under ("`s3:DeleteObject` — to
/// delete an object (or add a delete marker)", `DeleteObject`).
const DELETE_OBJECT: &str = "s3:DeleteObject";

/// An emulator holding S3 beside the two services a decision about a session's call reads
/// from: IAM (the role and its identity policies) and STS (the session a role is assumed
/// into).
struct Emulator {
    router: MultiServiceRouter,
}

/// One role session a row calls as: the access key id its requests are signed with, and the
/// two ARNs a resource-policy statement can name it by.
struct Session {
    /// The session's access key id, which is where this emulator reads WHO is calling.
    access_key_id: String,
    /// The assumed-role session ARN `sts:GetCallerIdentity` reports, which a refusal names.
    session_arn: String,
    /// The ARN of the role the session is of, which is the other spelling a policy names it
    /// by.
    role_arn: String,
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

    /// One S3 request signed with `access_key_id` in its SigV4 credential scope — which is
    /// where this emulator reads which principal a request speaks for.
    async fn s3(&self, access_key_id: &str, method: &str, uri: &str, body: &[u8]) -> Answer {
        self.s3_with_headers(access_key_id, method, uri, &[], body)
            .await
    }

    /// [`Emulator::s3`], carrying the request headers an operation is addressed with — the
    /// copy source among them.
    async fn s3_with_headers(
        &self,
        access_key_id: &str,
        method: &str,
        uri: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> Answer {
        let host = format!("s3.{REGION}.amazonaws.com");
        let mut builder = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{uri}"))
            .header("host", host.clone())
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/s3/\
                     aws4_request, SignedHeaders=host, Signature=0000"
                ),
            );
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder
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

    /// Create an IAM role carrying `policy` as its inline policy (when one is given), assume
    /// it under `session_name`, and answer the session that assume minted.
    async fn assumed_role(
        &self,
        role_name: &str,
        session_name: &str,
        policy: Option<&str>,
    ) -> Session {
        let account = engine::emulator_account_id();
        let role_arn = format!("arn:aws:iam::{account}:role/{role_name}");
        let trust = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": {"AWS": format!("arn:aws:iam::{account}:root")},
                "Action": "sts:AssumeRole",
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
        if let Some(policy) = policy {
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
        }
        let (status, body) = self
            .query(
                "sts",
                "2011-06-15",
                "AssumeRole",
                &[
                    ("RoleArn", role_arn.clone()),
                    ("RoleSessionName", session_name.to_string()),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "assuming {role_name:?} must succeed, got {status} {body}"
        );
        Session {
            access_key_id: xml_element(&body, "AccessKeyId")
                .unwrap_or_else(|| panic!("AssumeRole must answer an AccessKeyId, got {body}")),
            session_arn: xml_element(&body, "Arn")
                .unwrap_or_else(|| panic!("AssumeRole must answer the session Arn, got {body}")),
            role_arn,
        }
    }

    /// `PUT /<bucket>` as the account — the bucket every object in this suite lives in.
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

    /// `PUT /<bucket>?policy` as the account — the resource-based policy under test. The
    /// answer carries no body ("the request succeeded with no content"), which is the status
    /// the policy suites of this crate pin.
    async fn put_bucket_policy(&self, case: &str, bucket: &str, document: &str) {
        let stored = self
            .s3(
                ACCOUNT_KEY,
                "PUT",
                &format!("/{bucket}?policy"),
                document.as_bytes(),
            )
            .await;
        assert_eq!(
            stored.status,
            204,
            "case {case:?}: the bucket policy {document} must be stored, got {} with {}",
            stored.status,
            body_text(&stored)
        );
    }

    /// `PUT /<bucket>/<key>` as the account — a fixture every read, delete and overwrite row
    /// addresses. Written before any policy is installed, so no row's document can decide
    /// another row's setup. Answers the version id the write minted, for a bucket whose
    /// versioning is enabled.
    async fn write_fixture(&self, bucket: &str, key: &str) -> Option<String> {
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
        written.header("x-amz-version-id")
    }

    /// `PUT /<bucket>?versioning` as the account — what makes the writes into a bucket mint
    /// version ids, so a copy source can name one.
    async fn enable_versioning(&self, bucket: &str) {
        let configured = self
            .s3(
                ACCOUNT_KEY,
                "PUT",
                &format!("/{bucket}?versioning"),
                b"<VersioningConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
                  <Status>Enabled</Status></VersioningConfiguration>",
            )
            .await;
        assert_eq!(
            configured.status,
            200,
            "versioning must be enabled on {bucket:?}, got {} with {}",
            configured.status,
            body_text(&configured)
        );
    }
}

/// Percent-encode one `application/x-www-form-urlencoded` component, leaving only RFC 3986's
/// unreserved characters literal — so a policy document's braces and quotes survive the round
/// trip.
///
/// A copy: an integration-test module cannot be imported across crates, so every suite that
/// drives an awsQuery service holds its own.
fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// Percent-encode one `application/x-www-form-urlencoded` component, leaving only RFC 3986's
/// unreserved characters literal — so a policy document's braces and quotes survive the round
/// trip.
///
/// `pub(crate)`: the suite that drives a SESSION-scoped assume
/// ([`super::session_scoped_object_authorization`]) reads the same answers, so the readings
/// below are shared rather than copied again.
pub(crate) fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// An answer's body as text, for error documents.
fn body_text(answer: &Answer) -> String {
    String::from_utf8_lossy(&answer.body).to_string()
}

/// A policy document carrying exactly `statement`.
fn document(statement: Value) -> String {
    json!({"Version": "2012-10-17", "Statement": [statement]}).to_string()
}

/// One resource-policy statement: the effect, the principal it names, the actions it covers
/// and the resources it is about.
fn resource_statement(effect: &str, principal: &str, actions: Value, resources: Value) -> Value {
    json!({
        "Effect": effect,
        "Principal": {"AWS": principal},
        "Action": actions,
        "Resource": resources,
    })
}

/// An identity-based document carrying `statements` — the inline policy a role holds, whose
/// statements name no principal because the document is attached to one.
pub(crate) fn identity_document(statements: Vec<Value>) -> String {
    json!({"Version": "2012-10-17", "Statement": statements}).to_string()
}

/// One identity-based statement over `actions` and `resources`.
fn identity_statement(effect: &str, actions: Value, resources: Value) -> Value {
    json!({
        "Effect": effect,
        "Action": actions,
        "Resource": resources,
    })
}

/// The ARN of the object `key` of `bucket` — S3's own ARN form, which carries neither a
/// Region nor an account.
pub(crate) fn object_arn(bucket: &str, key: &str) -> String {
    format!("arn:aws:s3:::{bucket}/{key}")
}

/// The sentence S3 refuses a call with, written out here rather than built from the service's
/// own helper so the wording has something to be wrong about: who was refused, which IAM
/// action on which resource, and the clause naming the document that decided it.
pub(crate) fn denial_message(principal: &str, action: &str, resource: &str, reason: &str) -> String {
    format!("User: {principal} is not authorized to perform: {action} on resource: \"{resource}\" {reason}")
}

/// The clause IAM publishes for a request a RESOURCE-based document explicitly denies.
const RESOURCE_DENY: &str = "with an explicit deny in a resource-based policy";

/// The clause IAM publishes for a request an IDENTITY-based document explicitly denies.
const IDENTITY_DENY: &str = "with an explicit deny in an identity-based policy";

/// The clause IAM publishes for a request no readable grant reaches at all.
fn no_grant(action: &str) -> String {
    format!("because no identity-based policy allows the {action} action")
}

/// Which object operation a row drives — each judged under its own IAM action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Call {
    /// `GetObject`, which answers the stored bytes.
    Get,
    /// `HeadObject`, which carries no body, so its status is the whole answer.
    Head,
    /// `PutObject`, which must leave no object behind when it is refused.
    Put,
    /// `DeleteObject`, which must leave the version it addressed readable when it is refused.
    Delete,
}

impl Call {
    /// The HTTP method this call is bound to.
    fn method(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Head => "HEAD",
            Self::Put => "PUT",
            Self::Delete => "DELETE",
        }
    }

    /// The IAM action it is authorized under.
    fn action(self) -> &'static str {
        match self {
            Self::Get | Self::Head => GET_OBJECT,
            Self::Put => PUT_OBJECT,
            Self::Delete => DELETE_OBJECT,
        }
    }

    /// Whether the key must already hold an object for this call to be about one: a write
    /// addresses a key it creates, every other call a version that is there.
    fn needs_fixture(self) -> bool {
        self != Self::Put
    }

    /// The status a call this policy leaves to the caller answers: a delete answers 204 "no
    /// content" (`DeleteObject` reports nothing about what it removed) and the rest 200.
    fn served_status(self) -> u16 {
        match self {
            Self::Delete => 204,
            _ => 200,
        }
    }
}

/// What a row's call must answer.
enum Expected {
    /// The call is left to the caller: the served status, and the stored bytes for a read.
    Served,
    /// The call is refused 403 `AccessDenied`, with this reason clause closing the sentence:
    /// the rest of it is the principal the request signed for, the IAM action the call is
    /// judged under and the object it addressed.
    Refused(String),
}

/// One row of either table: the case, the document in force, the key the call addresses, the
/// call itself and what it must answer.
struct Row {
    case: &'static str,
    /// The object key this row addresses — its own, so one row's document cannot decide
    /// another's call.
    key: String,
    /// The bucket policy this row installs, or `None` for a bucket carrying none.
    bucket_policy: Option<String>,
    call: Call,
    expected: Expected,
}

/// Drive one row's call as `session` and grade the answer, then check that a REFUSED call
/// changed nothing: a refused write left no object and a refused delete left the version it
/// addressed readable.
async fn drive(emulator: &Emulator, bucket: &str, session: &Session, row: &Row) {
    if let Some(policy) = &row.bucket_policy {
        emulator.put_bucket_policy(row.case, bucket, policy).await;
    }
    let uri = format!("/{bucket}/{}", row.key);
    let body: &[u8] = match row.call {
        Call::Put => BODY,
        _ => b"",
    };
    let answer = emulator
        .s3(&session.access_key_id, row.call.method(), &uri, body)
        .await;
    let (case, call) = (row.case, row.call);
    match &row.expected {
        Expected::Served => {
            assert_eq!(
                answer.status,
                call.served_status(),
                "case {case:?}: {call:?} must be left to the caller with HTTP {}, got {} with {}",
                call.served_status(),
                answer.status,
                body_text(&answer)
            );
            if call == Call::Get {
                assert_eq!(
                    answer.body.as_ref(),
                    BODY,
                    "case {case:?}: {call:?} must answer the stored bytes, got {:?}",
                    body_text(&answer)
                );
            }
        }
        Expected::Refused(reason) => {
            let message = denial_message(
                &session.session_arn,
                call.action(),
                &object_arn(bucket, &row.key),
                reason,
            );
            assert_eq!(
                answer.status, 403,
                "case {case:?}: {call:?} must be refused with HTTP 403, got {} with {}",
                answer.status,
                body_text(&answer)
            );
            // A HEAD carries no body at all, so its status is everything it reports; every
            // other refusal carries the error document S3 words the decision in.
            if call != Call::Head {
                assert_eq!(
                    answer.error_code(),
                    ACCESS_DENIED,
                    "case {case:?}: {call:?} must be refused {ACCESS_DENIED}, got {}",
                    body_text(&answer)
                );
                assert_eq!(
                    answer.error_message(),
                    message,
                    "case {case:?}: {call:?} must carry the published sentence, got {}",
                    body_text(&answer)
                );
            }
            // The refusal is taken BEFORE the handler writes anything, so a refused write
            // left no object and a refused delete left the version it addressed readable.
            // Asked of the two calls that would have MUTATED the store (the policies of
            // those rows name the session rather than the account, so the account's own
            // read-back is not itself a decision under test).
            let expected_status = match call {
                Call::Put => Some(404),
                Call::Delete => Some(200),
                Call::Get | Call::Head => None,
            };
            if let Some(expected_status) = expected_status {
                let read_back = emulator.s3(ACCOUNT_KEY, "HEAD", &uri, b"").await;
                assert_eq!(
                    read_back.status, expected_status,
                    "case {case:?}: after the refused {call:?} the key must read back HTTP \
                     {expected_status}, got {}",
                    read_back.status
                );
            }
        }
    }
}

#[tokio::test]
async fn a_role_sessions_object_call_is_refused_by_an_explicit_deny_in_the_bucket_policy() {
    const BUCKET: &str = "role-session-resource-policy";
    let emulator = Emulator::new();
    emulator.create_bucket(BUCKET).await;

    // The session under test, whose role may perform every S3 action: with the identity half
    // of the decision satisfied, only the BUCKET's own policy can refuse it.
    let session = emulator
        .assumed_role(
            "resource-policy-reader",
            "probe-session",
            Some(&identity_document(vec![identity_statement(
                "Allow",
                json!("s3:*"),
                json!("*"),
            )])),
        )
        .await;
    // A second role, which no row calls as: it is there so one row's deny can name a
    // principal of this account that is NOT the caller.
    let other = emulator
        .assumed_role("resource-policy-bystander", "bystander-session", None)
        .await;
    let account_root = format!("arn:aws:iam::{}:root", engine::emulator_account_id());

    let rows: Vec<Row> = vec![
        // The principal spellings that reach the session: the ARN STS reports for it, the
        // role behind it, the account it belongs to, and the wildcard.
        Row {
            case: "the deny names the session ARN",
            key: "denied/by-session-arn".to_string(),
            bucket_policy: Some(document(resource_statement(
                "Deny",
                &session.session_arn,
                json!(GET_OBJECT),
                json!(object_arn(BUCKET, "denied/*")),
            ))),
            call: Call::Get,
            expected: Expected::Refused(RESOURCE_DENY.to_string()),
        },
        Row {
            case: "the deny names the role behind the session",
            key: "denied/by-role-arn".to_string(),
            bucket_policy: Some(document(resource_statement(
                "Deny",
                &session.role_arn,
                json!(GET_OBJECT),
                json!(object_arn(BUCKET, "denied/*")),
            ))),
            call: Call::Get,
            expected: Expected::Refused(RESOURCE_DENY.to_string()),
        },
        Row {
            case: "the deny names the account the session belongs to",
            key: "denied/by-account-root".to_string(),
            bucket_policy: Some(document(resource_statement(
                "Deny",
                &account_root,
                json!(GET_OBJECT),
                json!(object_arn(BUCKET, "denied/*")),
            ))),
            call: Call::Get,
            expected: Expected::Refused(RESOURCE_DENY.to_string()),
        },
        Row {
            case: "the deny names every principal",
            key: "denied/by-wildcard".to_string(),
            bucket_policy: Some(document(resource_statement(
                "Deny",
                "*",
                json!(GET_OBJECT),
                json!(object_arn(BUCKET, "denied/*")),
            ))),
            call: Call::Get,
            expected: Expected::Refused(RESOURCE_DENY.to_string()),
        },
        // A deny that does not reach this call: another principal of the account, and another
        // key prefix.
        Row {
            case: "the deny names another role of the account",
            key: "denied/for-another-role".to_string(),
            bucket_policy: Some(document(resource_statement(
                "Deny",
                &other.role_arn,
                json!(GET_OBJECT),
                json!(object_arn(BUCKET, "denied/*")),
            ))),
            call: Call::Get,
            expected: Expected::Served,
        },
        Row {
            case: "the deny names another key prefix",
            key: "denied/outside-the-deny".to_string(),
            bucket_policy: Some(document(resource_statement(
                "Deny",
                &session.session_arn,
                json!(GET_OBJECT),
                json!(object_arn(BUCKET, "elsewhere/*")),
            ))),
            call: Call::Get,
            expected: Expected::Served,
        },
        // The ACTION and RESOURCE elements, in the forms a published bucket policy writes
        // them: the exact key, a prefix wildcard over the action, the whole service, and the
        // bucket's every object.
        Row {
            case: "the deny names the exact key",
            key: "denied/exact-key".to_string(),
            bucket_policy: Some(document(resource_statement(
                "Deny",
                &session.session_arn,
                json!(GET_OBJECT),
                json!(object_arn(BUCKET, "denied/exact-key")),
            ))),
            call: Call::Get,
            expected: Expected::Refused(RESOURCE_DENY.to_string()),
        },
        Row {
            case: "the deny names a prefix of the action",
            key: "denied/action-prefix".to_string(),
            bucket_policy: Some(document(resource_statement(
                "Deny",
                &session.session_arn,
                json!("s3:Get*"),
                json!(object_arn(BUCKET, "*")),
            ))),
            call: Call::Get,
            expected: Expected::Refused(RESOURCE_DENY.to_string()),
        },
        Row {
            case: "the deny names every action of the service",
            key: "denied/service-wildcard".to_string(),
            bucket_policy: Some(document(resource_statement(
                "Deny",
                &session.session_arn,
                json!("s3:*"),
                json!(object_arn(BUCKET, "denied/*")),
            ))),
            call: Call::Head,
            expected: Expected::Refused(RESOURCE_DENY.to_string()),
        },
        // The OPERATIONS beside the read, each under its own action — and each refusal is a
        // call that did not happen (the write leaves no object, the delete leaves the
        // version).
        Row {
            case: "the deny names the read beside the write",
            key: "denied/write".to_string(),
            bucket_policy: Some(document(resource_statement(
                "Deny",
                &session.session_arn,
                json!([GET_OBJECT, PUT_OBJECT]),
                json!(object_arn(BUCKET, "denied/*")),
            ))),
            call: Call::Put,
            expected: Expected::Refused(RESOURCE_DENY.to_string()),
        },
        Row {
            case: "a write outside the denied prefix",
            key: "allowed/write".to_string(),
            bucket_policy: Some(document(resource_statement(
                "Deny",
                &session.session_arn,
                json!([GET_OBJECT, PUT_OBJECT]),
                json!(object_arn(BUCKET, "denied/*")),
            ))),
            call: Call::Put,
            expected: Expected::Served,
        },
        Row {
            case: "the deny names the delete",
            key: "denied/delete".to_string(),
            bucket_policy: Some(document(resource_statement(
                "Deny",
                &session.session_arn,
                json!(DELETE_OBJECT),
                json!(object_arn(BUCKET, "denied/*")),
            ))),
            call: Call::Delete,
            expected: Expected::Refused(RESOURCE_DENY.to_string()),
        },
        Row {
            case: "a delete the policy says nothing about",
            key: "allowed/delete".to_string(),
            bucket_policy: Some(document(resource_statement(
                "Deny",
                &session.session_arn,
                json!(GET_OBJECT),
                json!(object_arn(BUCKET, "denied/*")),
            ))),
            call: Call::Delete,
            expected: Expected::Served,
        },
    ];

    // Every fixture is written BEFORE any document is installed, so no row's policy can
    // decide another row's setup.
    for row in &rows {
        if row.call.needs_fixture() {
            let _ = emulator.write_fixture(BUCKET, &row.key).await;
        }
    }
    for row in &rows {
        drive(&emulator, BUCKET, &session, row).await;
    }
}

#[tokio::test]
async fn a_role_sessions_object_call_needs_a_grant_in_the_roles_own_policies() {
    const BUCKET: &str = "role-session-identity-policy";
    let emulator = Emulator::new();
    emulator.create_bucket(BUCKET).await;

    // One role per identity-policy shape, each assumed into its own session: what the role's
    // own documents reach is the whole decision here, because the bucket carries no policy.
    let every_action = emulator
        .assumed_role(
            "identity-every-action",
            "every-action-session",
            Some(&identity_document(vec![identity_statement(
                "Allow",
                json!("s3:*"),
                json!("*"),
            )])),
        )
        .await;
    let read_only = emulator
        .assumed_role(
            "identity-read-only",
            "read-only-session",
            Some(&identity_document(vec![identity_statement(
                "Allow",
                json!(GET_OBJECT),
                json!(object_arn(BUCKET, "*")),
            )])),
        )
        .await;
    let denied_prefix = emulator
        .assumed_role(
            "identity-denied-prefix",
            "denied-prefix-session",
            Some(&identity_document(vec![
                identity_statement("Allow", json!("s3:*"), json!("*")),
                identity_statement(
                    "Deny",
                    json!(GET_OBJECT),
                    json!(object_arn(BUCKET, "denied/*")),
                ),
            ])),
        )
        .await;
    let other_key = emulator
        .assumed_role(
            "identity-other-key",
            "other-key-session",
            Some(&identity_document(vec![identity_statement(
                "Allow",
                json!(GET_OBJECT),
                json!(object_arn(BUCKET, "elsewhere/*")),
            )])),
        )
        .await;
    let no_policy = emulator
        .assumed_role("identity-no-policy", "no-policy-session", None)
        .await;

    // `(case, the session calling, the key, the call, what it must answer)`. Each row's key
    // is written by the account first, so a refusal is about the policies and not about a
    // missing object — and every session below may list the bucket, or may not, which is
    // what decides whether an ABSENT key answers 404 or 403; that question has its own table
    // in `identity_authorization`.
    let rows: Vec<(&str, &Session, &str, Call, Expected)> = vec![
        (
            "the service wildcard reaches the read",
            &every_action,
            "reports/wildcard.csv",
            Call::Get,
            Expected::Served,
        ),
        (
            "the grant names the read itself",
            &read_only,
            "reports/read-only.csv",
            Call::Get,
            Expected::Served,
        ),
        (
            "a read-only grant does not reach the write",
            &read_only,
            "reports/read-only-write.csv",
            Call::Put,
            Expected::Refused(no_grant(PUT_OBJECT)),
        ),
        (
            "a read-only grant does not reach the delete",
            &read_only,
            "reports/read-only-delete.csv",
            Call::Delete,
            Expected::Refused(no_grant(DELETE_OBJECT)),
        ),
        (
            "an explicit deny beside the grant refuses the read",
            &denied_prefix,
            "denied/identity.csv",
            Call::Get,
            Expected::Refused(IDENTITY_DENY.to_string()),
        ),
        (
            "the same role reaches a key its deny does not name",
            &denied_prefix,
            "reports/identity.csv",
            Call::Get,
            Expected::Served,
        ),
        (
            "a grant over another key does not reach this one",
            &other_key,
            "reports/other-key.csv",
            Call::Get,
            Expected::Refused(no_grant(GET_OBJECT)),
        ),
        (
            "a role carrying no policy reaches nothing",
            &no_policy,
            "reports/no-policy.csv",
            Call::Get,
            Expected::Refused(no_grant(GET_OBJECT)),
        ),
        (
            "a role carrying no policy may not write either",
            &no_policy,
            "reports/no-policy-write.csv",
            Call::Put,
            Expected::Refused(no_grant(PUT_OBJECT)),
        ),
        (
            "a HEAD asks for the same permission as the read",
            &no_policy,
            "reports/no-policy-head.csv",
            Call::Head,
            Expected::Refused(no_grant(GET_OBJECT)),
        ),
    ];

    let rows: Vec<(&Session, Row)> = rows
        .into_iter()
        .map(|(case, session, key, call, expected)| {
            (
                session,
                Row {
                    case,
                    key: key.to_string(),
                    bucket_policy: None,
                    call,
                    expected,
                },
            )
        })
        .collect();
    for (_, row) in &rows {
        if row.call.needs_fixture() {
            let _ = emulator.write_fixture(BUCKET, &row.key).await;
        }
    }
    for (session, row) in &rows {
        drive(&emulator, BUCKET, session, row).await;
    }
}

/// Which object a copy's refusal is about — the half of the copy the decision refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Half {
    /// The read of the object `x-amz-copy-source` names.
    SourceRead,
    /// The write of the object the request addresses.
    DestinationWrite,
}

/// What a copy must answer: it is left to the session, or refused over one of its two halves
/// with the action and the reason clause given.
enum CopyExpected {
    Served,
    Refused {
        action: String,
        half: Half,
        reason: String,
    },
}

/// One row of the copy table: the case, the session the copy is made as, the documents in
/// force on the two buckets, whether the copy source names the source's version, and what the
/// copy must answer.
struct CopyRow<'a> {
    case: &'static str,
    session: &'a Session,
    /// The SOURCE bucket's own policy, `None` for a bucket carrying none.
    source_policy: Option<String>,
    /// The DESTINATION bucket's own policy, `None` for a bucket carrying none.
    destination_policy: Option<String>,
    /// Whether `x-amz-copy-source` qualifies the reference with the source's `versionId`,
    /// which is what asks for the version-scoped read instead of the plain one.
    names_the_version: bool,
    expected: CopyExpected,
}

#[tokio::test]
async fn a_copy_by_a_role_session_authorizes_both_of_its_halves_against_that_session() {
    const SOURCE: &str = "role-session-copy-source";
    const DESTINATION: &str = "role-session-copy-destination";
    /// The key the source object is written at, which every row's copy reads.
    const SOURCE_KEY: &str = "reports/source.csv";
    let emulator = Emulator::new();
    emulator.create_bucket(SOURCE).await;
    emulator.create_bucket(DESTINATION).await;
    // Versioning on the source is what mints the version id one row's copy source names.
    emulator.enable_versioning(SOURCE).await;
    let version_id = emulator
        .write_fixture(SOURCE, SOURCE_KEY)
        .await
        .expect("a write into a versioning-enabled bucket mints a version id");

    let source_objects = object_arn(SOURCE, "*");
    let destination_objects = object_arn(DESTINATION, "*");
    // One role per identity-policy shape: what the role's own documents reach is half of
    // every decision below.
    let both_halves = emulator
        .assumed_role(
            "copy-both-halves",
            "both-halves-session",
            Some(&identity_document(vec![identity_statement(
                "Allow",
                json!("s3:*"),
                json!("*"),
            )])),
        )
        .await;
    let write_only = emulator
        .assumed_role(
            "copy-write-only",
            "write-only-session",
            Some(&identity_document(vec![identity_statement(
                "Allow",
                json!(PUT_OBJECT),
                json!(destination_objects.clone()),
            )])),
        )
        .await;
    let read_only = emulator
        .assumed_role(
            "copy-read-only",
            "read-only-session",
            Some(&identity_document(vec![identity_statement(
                "Allow",
                json!(GET_OBJECT),
                json!(source_objects.clone()),
            )])),
        )
        .await;
    // A role reaching the current version of the source and the destination write, and
    // nothing version-scoped: `s3:GetObject` does not cover `s3:GetObjectVersion`.
    let current_version_only = emulator
        .assumed_role(
            "copy-current-version-only",
            "current-version-session",
            Some(&identity_document(vec![identity_statement(
                "Allow",
                json!([GET_OBJECT, PUT_OBJECT]),
                json!([source_objects.clone(), destination_objects.clone()]),
            )])),
        )
        .await;

    let rows: Vec<CopyRow> = vec![
        CopyRow {
            case: "a session both halves are left to",
            session: &both_halves,
            source_policy: None,
            destination_policy: None,
            names_the_version: false,
            expected: CopyExpected::Served,
        },
        CopyRow {
            case: "the source bucket denies the read to the session",
            session: &both_halves,
            source_policy: Some(document(resource_statement(
                "Deny",
                &both_halves.session_arn,
                json!(GET_OBJECT),
                json!(source_objects.clone()),
            ))),
            destination_policy: None,
            names_the_version: false,
            expected: CopyExpected::Refused {
                action: GET_OBJECT.to_string(),
                half: Half::SourceRead,
                reason: RESOURCE_DENY.to_string(),
            },
        },
        CopyRow {
            case: "the destination bucket denies the write to the role behind the session",
            session: &both_halves,
            source_policy: None,
            destination_policy: Some(document(resource_statement(
                "Deny",
                &both_halves.role_arn,
                json!(PUT_OBJECT),
                json!(destination_objects.clone()),
            ))),
            names_the_version: false,
            expected: CopyExpected::Refused {
                action: PUT_OBJECT.to_string(),
                half: Half::DestinationWrite,
                reason: RESOURCE_DENY.to_string(),
            },
        },
        CopyRow {
            case: "the session's own policy does not reach the source read",
            session: &write_only,
            source_policy: None,
            destination_policy: None,
            names_the_version: false,
            expected: CopyExpected::Refused {
                action: GET_OBJECT.to_string(),
                half: Half::SourceRead,
                reason: no_grant(GET_OBJECT),
            },
        },
        CopyRow {
            case: "the session's own policy does not reach the destination write",
            session: &read_only,
            source_policy: None,
            destination_policy: None,
            names_the_version: false,
            expected: CopyExpected::Refused {
                action: PUT_OBJECT.to_string(),
                half: Half::DestinationWrite,
                reason: no_grant(PUT_OBJECT),
            },
        },
        CopyRow {
            case: "a copy source naming the version asks for the version-scoped read",
            session: &current_version_only,
            source_policy: None,
            destination_policy: None,
            names_the_version: true,
            expected: CopyExpected::Refused {
                action: GET_OBJECT_VERSION.to_string(),
                half: Half::SourceRead,
                reason: no_grant(GET_OBJECT_VERSION),
            },
        },
        CopyRow {
            case: "the same copy of the current version is left to that session",
            session: &current_version_only,
            source_policy: None,
            destination_policy: None,
            names_the_version: false,
            expected: CopyExpected::Served,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        // Each row addresses its own destination key, so one row's copy cannot be read as
        // another's; a bucket carrying no policy for this row has the row before it cleared.
        let destination_key = format!("copies/{index}.csv");
        emulator
            .put_bucket_policy(
                case,
                SOURCE,
                row.source_policy.as_deref().unwrap_or(&permissive(SOURCE)),
            )
            .await;
        emulator
            .put_bucket_policy(
                case,
                DESTINATION,
                row.destination_policy
                    .as_deref()
                    .unwrap_or(&permissive(DESTINATION)),
            )
            .await;
        let copy_source = match row.names_the_version {
            true => format!("/{SOURCE}/{SOURCE_KEY}?versionId={version_id}"),
            false => format!("/{SOURCE}/{SOURCE_KEY}"),
        };
        let answer = emulator
            .s3_with_headers(
                &row.session.access_key_id,
                "PUT",
                &format!("/{DESTINATION}/{destination_key}"),
                &[("x-amz-copy-source", copy_source.as_str())],
                b"",
            )
            .await;
        match &row.expected {
            CopyExpected::Served => assert_eq!(
                answer.status,
                200,
                "case {case:?}: the copy must be left to the session, got {} with {}",
                answer.status,
                body_text(&answer)
            ),
            CopyExpected::Refused {
                action,
                half,
                reason,
            } => {
                assert_eq!(
                    answer.status, 403,
                    "case {case:?}: the copy must be refused with HTTP 403, got {} with {}",
                    answer.status,
                    body_text(&answer)
                );
                assert_eq!(
                    answer.error_code(),
                    ACCESS_DENIED,
                    "case {case:?}: the copy must be refused {ACCESS_DENIED}, got {}",
                    body_text(&answer)
                );
                // The refusal names the object of the HALF it refused: the copy source for a
                // read the policies do not reach, the destination for a write they do not.
                let resource = match half {
                    Half::SourceRead => object_arn(SOURCE, SOURCE_KEY),
                    Half::DestinationWrite => object_arn(DESTINATION, &destination_key),
                };
                assert_eq!(
                    answer.error_message(),
                    denial_message(&row.session.session_arn, action, &resource, reason),
                    "case {case:?}: the copy must carry the published sentence, got {}",
                    body_text(&answer)
                );
            }
        }
        // What the DESTINATION holds afterwards, read by the account: the copy's bytes for a
        // copy that was served, and nothing at all for one that was refused — a refused copy
        // copies nothing.
        let read_back = emulator
            .s3(
                ACCOUNT_KEY,
                "GET",
                &format!("/{DESTINATION}/{destination_key}"),
                b"",
            )
            .await;
        match &row.expected {
            CopyExpected::Served => assert_eq!(
                read_back.body.as_ref(),
                BODY,
                "case {case:?}: the copy must read back with the source's bytes, got {} with {}",
                read_back.status,
                body_text(&read_back)
            ),
            CopyExpected::Refused { .. } => assert_eq!(
                read_back.status, 404,
                "case {case:?}: a refused copy must store nothing, got {} with {}",
                read_back.status,
                body_text(&read_back)
            ),
        }
    }
}

/// A bucket policy that decides nothing about an object call: it allows the account's own
/// credential the one action that keeps the policy itself readable, so a row carrying no
/// document of its own clears the row before it without leaving a grant or a deny behind.
fn permissive(bucket: &str) -> String {
    document(resource_statement(
        "Allow",
        &format!("arn:aws:iam::{}:root", engine::emulator_account_id()),
        json!("s3:GetBucketPolicy"),
        json!(format!("arn:aws:s3:::{bucket}")),
    ))
}

#[tokio::test]
async fn a_part_copy_by_a_role_session_authorizes_its_source_read_against_that_session() {
    const SOURCE: &str = "role-session-part-copy-source";
    const DESTINATION: &str = "role-session-part-copy-destination";
    /// The key the source object is written at, which every row's part copy reads.
    const SOURCE_KEY: &str = "reports/part-source.csv";
    let emulator = Emulator::new();
    emulator.create_bucket(SOURCE).await;
    emulator.create_bucket(DESTINATION).await;
    let _ = emulator.write_fixture(SOURCE, SOURCE_KEY).await;

    let source_objects = object_arn(SOURCE, "*");
    let every_action = emulator
        .assumed_role(
            "part-copy-every-action",
            "part-copy-session",
            Some(&identity_document(vec![identity_statement(
                "Allow",
                json!("s3:*"),
                json!("*"),
            )])),
        )
        .await;
    let no_source_read = emulator
        .assumed_role(
            "part-copy-no-source-read",
            "no-source-read-session",
            Some(&identity_document(vec![identity_statement(
                "Allow",
                json!(PUT_OBJECT),
                json!("*"),
            )])),
        )
        .await;

    // `(case, the session copying, the source bucket's policy, what the part copy must
    // answer)` — the same rule as a whole-object copy's read half, asked of the operation
    // that copies a span of the source into an upload.
    let rows: Vec<(&str, &Session, Option<String>, Option<String>)> = vec![
        ("a session the read is left to", &every_action, None, None),
        (
            "the source bucket denies the read to the session",
            &every_action,
            Some(document(resource_statement(
                "Deny",
                &every_action.session_arn,
                json!(GET_OBJECT),
                json!(source_objects.clone()),
            ))),
            Some(RESOURCE_DENY.to_string()),
        ),
        (
            "the session's own policy does not reach the read",
            &no_source_read,
            None,
            Some(no_grant(GET_OBJECT)),
        ),
    ];

    for (index, (case, session, source_policy, refusal)) in rows.iter().enumerate() {
        let destination_key = format!("part-copies/{index}.csv");
        emulator
            .put_bucket_policy(
                case,
                SOURCE,
                source_policy.as_deref().unwrap_or(&permissive(SOURCE)),
            )
            .await;
        // The upload the part is copied into is initiated by the ACCOUNT, so what this row
        // judges is the copy's read of the source and nothing about the upload.
        let initiated = emulator
            .s3(
                ACCOUNT_KEY,
                "POST",
                &format!("/{DESTINATION}/{destination_key}?uploads"),
                b"",
            )
            .await;
        assert_eq!(
            initiated.status,
            200,
            "case {case:?}: the upload must be initiated first, got {} with {}",
            initiated.status,
            body_text(&initiated)
        );
        let upload_id = initiated
            .element("UploadId")
            .unwrap_or_else(|| panic!("case {case:?}: an initiation reports an UploadId"));
        let answer = emulator
            .s3_with_headers(
                &session.access_key_id,
                "PUT",
                &format!(
                    "/{DESTINATION}/{destination_key}?partNumber=1&uploadId={upload_id}\
                     &x-id=UploadPartCopy"
                ),
                &[(
                    "x-amz-copy-source",
                    format!("/{SOURCE}/{SOURCE_KEY}").as_str(),
                )],
                b"",
            )
            .await;
        match refusal {
            None => assert_eq!(
                answer.status,
                200,
                "case {case:?}: the part copy must be left to the session, got {} with {}",
                answer.status,
                body_text(&answer)
            ),
            Some(reason) => {
                assert_eq!(
                    answer.status, 403,
                    "case {case:?}: the part copy must be refused with HTTP 403, got {} with {}",
                    answer.status,
                    body_text(&answer)
                );
                assert_eq!(
                    answer.error_code(),
                    ACCESS_DENIED,
                    "case {case:?}: the part copy must be refused {ACCESS_DENIED}, got {}",
                    body_text(&answer)
                );
                assert_eq!(
                    answer.error_message(),
                    denial_message(
                        &session.session_arn,
                        GET_OBJECT,
                        &object_arn(SOURCE, SOURCE_KEY),
                        reason
                    ),
                    "case {case:?}: the part copy must carry the published sentence, got {}",
                    body_text(&answer)
                );
            }
        }
        // What the UPLOAD holds afterwards, listed by the account: the part for a copy that
        // was served, and no part at all for one that was refused — a refused part copy
        // copies nothing into the upload.
        let parts = emulator
            .s3(
                ACCOUNT_KEY,
                "GET",
                &format!("/{DESTINATION}/{destination_key}?uploadId={upload_id}"),
                b"",
            )
            .await;
        let listed = body_text(&parts);
        assert_eq!(
            listed.contains("<Part>"),
            refusal.is_none(),
            "case {case:?}: expected a listed part={}, got {listed}",
            refusal.is_none()
        );
    }
}
