// SPDX-License-Identifier: CC0-1.0

//! What a credential `sts:GetFederationToken` minted may do in S3.
//!
//! The rule under test: **a federated-user session's permissions are the INTERSECTION of the
//! identity-based policies of the IAM user whose long-term credentials minted it and the
//! session policies that mint passed.** "When you pass session policies, the session
//! permissions are the intersection of the IAM user policies and the session policies that you
//! pass … You cannot use session policies to grant more permissions than those that are
//! defined in the permissions policy of the IAM user", and "if you do not pass any session
//! policies, then the resulting federated user session has no permissions"
//! (`GetFederationTokenRequest$Policy`, the STS model). So, for a request signed with such a
//! credential:
//!
//! * a call both the user's own documents and the session policy allow is served;
//! * a call the user's documents allow and the session policy does not is refused, and the
//!   refusal names the SESSION policy as the document that withheld the grant, because that
//!   is what has to be widened for the call to go through;
//! * a call only the session policy allows is refused as if the session policy had allowed
//!   nothing — it grants nothing of its own — and the refusal names the identity's documents;
//! * an explicit `Deny` in the session policy refuses whatever grants it;
//! * a mint that passed no session policy at all vends a credential with no permissions.
//!
//! Every refusal is HTTP 403 `AccessDenied` carrying S3's one sentence, naming the principal
//! as the `FederatedUser` ARN the mint answered with, the action asked about and the resource
//! addressed — and a refused `CreateBucket` leaves no bucket behind, so the account's own
//! listing does not name it.
//!
//! Every assertion drives real HTTP requests through a router holding S3 beside the two
//! services such a decision reads from: IAM (the user, its access key and its inline policy,
//! written with `CreateUser` + `PutUserPolicy` + `CreateAccessKey`) and STS (the mint itself,
//! `GetFederationToken`). Each S3 request then presents the access key id of the credential
//! that mint vended.
//!
//! The table spans what the rule generalises over rather than the one recorded call: what the
//! session policy states (nothing at all, a narrower grant, an explicit deny, another
//! bucket's objects, more than the user allows, exactly the action called), what the user's
//! own policy allows, which operation is called (a listing, an object read, an object write, a
//! bucket create) and therefore which action and resource the decision is taken over.

use std::sync::Arc;

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

// The two readers every suite of this test binary that drives an awsQuery service needs, each
// held in ONE place: the percent-encoder this crate already encodes a form component with, and
// the reader that takes the XML escaping back off an element's text.
use super::credential_scoped_authorization::element;
use super::identity_authorization::form_encode;

/// The Region every request in this suite is signed for, and the endpoint it addresses.
const REGION: &str = "us-east-1";

/// The access-key id standing for the account's own published credential, which writes this
/// suite's fixtures: neither STS nor IAM holds a record of it, so it resolves to the account's
/// ambient identity.
const ACCOUNT_KEY: &str = "AKIDEMULATOR";

/// The bucket the object and listing rows address.
const BUCKET: &str = "federated-session-subject";

/// The object every object row addresses.
const KEY: &str = "reports/held.csv";

/// The bytes that object holds. A policy is about who may reach an object, not about what is
/// in it, so one payload serves every row.
const BODY: &[u8] = b"federated-session-subject";

/// The code S3 answers an authorization refusal with, and the status it carries (Amazon S3
/// "Error responses": `AccessDenied`, HTTP 403).
const ACCESS_DENIED: &str = "AccessDenied";

/// How long a minted federation session lasts here — inside the modelled range
/// (`GetFederationTokenRequest$DurationSeconds`, 900..129600) and comfortably longer than the
/// suite's own run, so no row is decided by a credential that expired mid-test.
const SESSION_SECONDS: i32 = 3600;

/// The action that lets the minting IAM user make the mint at all, carried by every row's user
/// policy beside the actions the row is about.
const MINT_ACTION: &str = "sts:GetFederationToken";

/// An emulator holding S3 beside the two services a decision about a federated session reads
/// from: IAM (the minting user and its identity-based documents) and STS (the session).
struct Emulator {
    router: MultiServiceRouter,
}

/// What a dispatched request answered: the status, and the body as text.
struct Answer {
    status: u16,
    body: String,
}

impl Answer {
    /// The `<Code>` of S3's restXml error document (the model sets `noErrorWrapping`, so it is
    /// the document's own child).
    fn error_code(&self) -> String {
        element(&self.body, "Code").unwrap_or_else(|| self.body.clone())
    }

    /// The `<Message>` of that same document.
    fn error_message(&self) -> String {
        element(&self.body, "Message").unwrap_or_default()
    }
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing: the `Authorization` header carries only the credential
        // scope, which is where a service reads the caller's access key id.
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

    /// One S3 request, presented as signed with `access_key_id` — which is where this emulator
    /// reads which principal a request speaks for.
    async fn s3(&self, access_key_id: &str, method: &str, uri: &str, body: &[u8]) -> Answer {
        let host = format!("s3.{REGION}.amazonaws.com");
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{uri}"))
            .header("host", host.clone())
            .header("content-type", "application/octet-stream")
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
            body: String::from_utf8_lossy(&body).to_string(),
        }
    }

    /// One awsQuery call to IAM or STS, presented as signed with `access_key_id`.
    async fn query(
        &self,
        access_key_id: &str,
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
            .header("host", format!("{signing_name}.amazonaws.com"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/\
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

    /// Create [`BUCKET`] and write [`KEY`] into it, as the account's own credential.
    async fn seeded_bucket(&self) {
        let created = self.s3(ACCOUNT_KEY, "PUT", &format!("/{BUCKET}"), b"").await;
        assert_eq!(
            created.status, 200,
            "the bucket must be created first, got {} with {}",
            created.status, created.body
        );
        let written = self
            .s3(ACCOUNT_KEY, "PUT", &format!("/{BUCKET}/{KEY}"), BODY)
            .await;
        assert_eq!(
            written.status, 200,
            "the fixture must be stored first, got {} with {}",
            written.status, written.body
        );
    }

    /// The account's own `ListBuckets`, which is how a row checks whether a create left a
    /// bucket behind.
    async fn account_buckets(&self) -> String {
        let listed = self.s3(ACCOUNT_KEY, "GET", "/", b"").await;
        assert_eq!(
            listed.status, 200,
            "the account's own listing must be served, got {} with {}",
            listed.status, listed.body
        );
        listed.body
    }

    /// An IAM user carrying exactly `actions` as its inline policy's grant, a federation
    /// session minted with its long-term key, and the session policy `scope` (when the row
    /// passes one).
    async fn federated_session(
        &self,
        name: &str,
        actions: &[&str],
        scope: Option<&str>,
    ) -> Session {
        let (status, body) = self
            .query(
                ACCOUNT_KEY,
                "iam",
                "2010-05-08",
                "CreateUser",
                &[("UserName", name.to_string())],
            )
            .await;
        assert_eq!(status, 200, "the user {name:?} must be created, got {body}");
        let mut granted: Vec<&str> = actions.to_vec();
        granted.push(MINT_ACTION);
        let document = json!({
            "Version": "2012-10-17",
            "Statement": [{"Effect": "Allow", "Action": granted, "Resource": "*"}],
        })
        .to_string();
        let (status, body) = self
            .query(
                ACCOUNT_KEY,
                "iam",
                "2010-05-08",
                "PutUserPolicy",
                &[
                    ("UserName", name.to_string()),
                    ("PolicyName", "granted".to_string()),
                    ("PolicyDocument", document),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "the policy of {name:?} must be stored, got {body}"
        );
        let (status, body) = self
            .query(
                ACCOUNT_KEY,
                "iam",
                "2010-05-08",
                "CreateAccessKey",
                &[("UserName", name.to_string())],
            )
            .await;
        assert_eq!(
            status, 200,
            "an access key for {name:?} must be vended, got {body}"
        );
        let long_term = element(&body, "AccessKeyId")
            .unwrap_or_else(|| panic!("CreateAccessKey must answer an AccessKeyId, got {body}"));
        let mut params = vec![
            ("Name", name.to_string()),
            ("DurationSeconds", SESSION_SECONDS.to_string()),
        ];
        if let Some(scope) = scope {
            params.push(("Policy", scope.to_string()));
        }
        let (status, body) = self
            .query(
                &long_term,
                "sts",
                "2011-06-15",
                "GetFederationToken",
                &params,
            )
            .await;
        assert_eq!(
            status, 200,
            "the mint for {name:?} must succeed, got {status} {body}"
        );
        Session {
            access_key_id: element(&body, "AccessKeyId").unwrap_or_else(|| {
                panic!("GetFederationToken must answer an AccessKeyId, got {body}")
            }),
            // The `Credentials` shape carries no `Arn`, so the first one in the document is
            // the `FederatedUser`'s — the principal a refusal of this session names.
            federated_user_arn: element(&body, "Arn").unwrap_or_else(|| {
                panic!("GetFederationToken must answer the FederatedUser Arn, got {body}")
            }),
        }
    }
}

/// One minted federation session: the access key id its requests are signed with, and the ARN
/// a refusal names it by.
struct Session {
    access_key_id: String,
    federated_user_arn: String,
}

/// WHICH call a row makes: the IAM action it is authorized under, and the resource that
/// decision is taken on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Call {
    /// `ListObjectsV2`, authorized under `s3:ListBucket` on the bucket.
    ListBucket,
    /// `GetObject`, authorized under `s3:GetObject` on the key.
    GetObject,
    /// `PutObject`, authorized under `s3:PutObject` on the key.
    PutObject,
    /// `CreateBucket`, authorized under `s3:CreateBucket` on a bucket that does not exist
    /// yet — the leg the recorded federation flow diverged on.
    CreateBucket,
}

/// The bucket a `CreateBucket` row addresses: one per row, so a served create and a refused
/// one never address the same name.
fn created_bucket(index: usize) -> String {
    format!("federated-session-created-{index}")
}

impl Call {
    /// The IAM action a grant has to name for this call ("you must have the `s3:PutObject`
    /// permission", `PutObject`; "you must have the `READ` access to the object" —
    /// `s3:GetObject`, `GetObject`; "you must have the `s3:ListBucket` permission",
    /// `ListObjectsV2`; "in addition to the `s3:CreateBucket` permission", `CreateBucket`).
    fn action(self) -> &'static str {
        match self {
            Self::ListBucket => "s3:ListBucket",
            Self::GetObject => "s3:GetObject",
            Self::PutObject => "s3:PutObject",
            Self::CreateBucket => "s3:CreateBucket",
        }
    }

    /// The resource a refusal of this call names: the bucket for the listing and the create,
    /// the object for the two object calls.
    fn resource(self, index: usize) -> String {
        match self {
            Self::ListBucket => format!("arn:aws:s3:::{BUCKET}"),
            Self::GetObject | Self::PutObject => format!("arn:aws:s3:::{BUCKET}/{KEY}"),
            Self::CreateBucket => format!("arn:aws:s3:::{}", created_bucket(index)),
        }
    }

    /// Make the call as `access_key_id`.
    async fn drive(self, emulator: &Emulator, access_key_id: &str, index: usize) -> Answer {
        match self {
            Self::ListBucket => {
                emulator
                    .s3(
                        access_key_id,
                        "GET",
                        &format!("/{BUCKET}?list-type=2"),
                        b"",
                    )
                    .await
            }
            Self::GetObject => {
                emulator
                    .s3(access_key_id, "GET", &format!("/{BUCKET}/{KEY}"), b"")
                    .await
            }
            Self::PutObject => {
                emulator
                    .s3(
                        access_key_id,
                        "PUT",
                        &format!("/{BUCKET}/{KEY}"),
                        b"rewritten",
                    )
                    .await
            }
            Self::CreateBucket => {
                emulator
                    .s3(
                        access_key_id,
                        "PUT",
                        &format!("/{}", created_bucket(index)),
                        b"",
                    )
                    .await
            }
        }
    }
}

/// What the documents in force answer one call: served, or refused with the clause naming the
/// document that decided it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// The call is authorized: both the minting user's documents and the session policy allow
    /// it.
    Served,
    /// Refused because the session policies the credential was minted with do not allow the
    /// action, whatever the user's own documents allow.
    NoSessionGrant,
    /// Refused by an explicit `Deny` in the session policy.
    SessionPolicyDeny,
    /// Refused because nothing in the minting user's documents granted the action — a session
    /// policy grants nothing of its own.
    NoIdentityGrant,
}

impl Verdict {
    /// The reason clause S3's refusal sentence carries for this verdict, for the IAM `action`
    /// asked about. These are IAM's own published clauses, one per document that can withhold
    /// or withdraw a grant.
    fn clause(self, action: &str) -> Option<String> {
        match self {
            Self::Served => None,
            Self::NoSessionGrant => Some(format!(
                "because no session policy allows the {action} action"
            )),
            Self::SessionPolicyDeny => {
                Some("with an explicit deny in a session policy".to_string())
            }
            Self::NoIdentityGrant => Some(format!(
                "because no identity-based policy allows the {action} action"
            )),
        }
    }
}

/// One row: what the minting user's own policy allows, what the mint's session policy states,
/// the call made with the vended credential, and what the intersection answers it.
struct Row {
    /// What to call the case in a failure.
    case: &'static str,
    /// The S3 actions the minting user's inline policy allows, beside [`MINT_ACTION`].
    user_actions: &'static [&'static str],
    /// The inline session policy the mint passed, or `None` for a mint that passed none.
    scope: Option<&'static str>,
    /// The call the row makes with the vended credential.
    call: Call,
    /// What the rule says that call is answered.
    verdict: Verdict,
}

/// The intersection table.
const ROWS: &[Row] = &[
    Row {
        case: "a session policy allowing just the read, on the read",
        user_actions: &["s3:*"],
        scope: Some(
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#,
        ),
        call: Call::GetObject,
        verdict: Verdict::Served,
    },
    Row {
        case: "a session policy allowing just the read, on the write",
        user_actions: &["s3:*"],
        scope: Some(
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#,
        ),
        call: Call::PutObject,
        verdict: Verdict::NoSessionGrant,
    },
    Row {
        case: "a session policy denying the write above its own broad grant",
        user_actions: &["s3:*"],
        scope: Some(
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:*","Resource":"*"},{"Effect":"Deny","Action":"s3:PutObject","Resource":"*"}]}"#,
        ),
        call: Call::PutObject,
        verdict: Verdict::SessionPolicyDeny,
    },
    Row {
        // A session policy grants nothing of its own: what the IAM user may not do, a session
        // minted with its credentials may not do either, however wide the scope is written.
        case: "a session policy wider than the minting user's own grant",
        user_actions: &["s3:GetObject"],
        scope: Some(
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:*","Resource":"*"}]}"#,
        ),
        call: Call::PutObject,
        verdict: Verdict::NoIdentityGrant,
    },
    Row {
        // "If you do not pass any session policies, then the resulting federated user session
        // has no permissions" — even where the minting user's own policy allows everything.
        case: "a mint that passed no session policy at all",
        user_actions: &["s3:*"],
        scope: None,
        call: Call::GetObject,
        verdict: Verdict::NoSessionGrant,
    },
    Row {
        case: "a session policy scoped to another bucket's objects",
        user_actions: &["s3:*"],
        scope: Some(
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:*","Resource":"arn:aws:s3:::somewhere-else/*"}]}"#,
        ),
        call: Call::GetObject,
        verdict: Verdict::NoSessionGrant,
    },
    Row {
        case: "a session policy whose listing grant covers the bucket",
        user_actions: &["s3:*"],
        scope: Some(
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:ListBucket","Resource":"*"}]}"#,
        ),
        call: Call::ListBucket,
        verdict: Verdict::Served,
    },
    Row {
        // The recorded federation flow: the user's own policy allows the create, the session
        // policy names only the listing of the account's buckets.
        case: "a session policy that does not name the bucket create",
        user_actions: &["s3:CreateBucket", "s3:ListAllMyBuckets"],
        scope: Some(
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:ListAllMyBuckets","Resource":"*"}]}"#,
        ),
        call: Call::CreateBucket,
        verdict: Verdict::NoSessionGrant,
    },
    Row {
        // The same flow with the create inside the intersection, so the scoping is a decision
        // about the action and not a blanket refusal of every federated session.
        case: "a session policy that names the bucket create",
        user_actions: &["s3:CreateBucket"],
        scope: Some(
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:CreateBucket","Resource":"*"}]}"#,
        ),
        call: Call::CreateBucket,
        verdict: Verdict::Served,
    },
];

/// RULE: a federated-user session's permissions are the intersection of the minting IAM user's
/// identity-based policies and the session policies the mint passed — a call outside that
/// intersection is refused 403 `AccessDenied`, naming the document that withheld or withdrew
/// the grant, and a mint that passed no session policy has no permissions at all.
#[tokio::test]
async fn a_federation_session_is_the_intersection_of_the_users_policies_and_the_session_policy() {
    for (index, row) in ROWS.iter().enumerate() {
        let emulator = Emulator::new();
        emulator.seeded_bucket().await;
        let session = emulator
            .federated_session(&format!("federating{index}"), row.user_actions, row.scope)
            .await;
        let answer = row
            .call
            .drive(&emulator, &session.access_key_id, index)
            .await;
        assert_verdict(
            row.case,
            &answer,
            row.verdict,
            row.call,
            index,
            &session.federated_user_arn,
        );
        // A create the intersection refuses must leave nothing behind, and one it allows must
        // leave the bucket the account can list.
        if row.call == Call::CreateBucket {
            let name = created_bucket(index);
            let listed = emulator.account_buckets().await;
            let holds = listed.contains(&format!("<Name>{name}</Name>"));
            assert_eq!(
                holds,
                row.verdict == Verdict::Served,
                "case {:?}: expected the account's listing to name {name:?} only where the \
                 create was served, got {listed}",
                row.case
            );
        }
    }
}

/// Assert that `answer` is what `verdict` says, for the call `call` made by `principal`.
fn assert_verdict(
    case: &str,
    answer: &Answer,
    verdict: Verdict,
    call: Call,
    index: usize,
    principal: &str,
) {
    match verdict.clause(call.action()) {
        None => assert_eq!(
            answer.status, 200,
            "case {case:?} ({call:?}): expected the call to be authorized, got {} with {}",
            answer.status, answer.body
        ),
        Some(clause) => {
            assert_eq!(
                answer.status, 403,
                "case {case:?} ({call:?}): expected HTTP 403, got {} with {}",
                answer.status, answer.body
            );
            let code = answer.error_code();
            assert_eq!(
                code, ACCESS_DENIED,
                "case {case:?} ({call:?}): expected {ACCESS_DENIED}, got {code:?}: {}",
                answer.body
            );
            let expected = format!(
                "User: {principal} is not authorized to perform: {} on resource: \"{}\" {clause}",
                call.action(),
                call.resource(index)
            );
            let message = answer.error_message();
            assert_eq!(
                message, expected,
                "case {case:?} ({call:?}): expected {expected:?}, got {message:?}"
            );
        }
    }
}

/// RULE: the principal a refusal of a federated session names is the `FederatedUser` ARN the
/// mint answered with — `arn:aws:sts::<account>:federated-user/<Name>` — whatever the name the
/// mint asked for.
///
/// The rows span the modelled `Name` (`userNameType`, length 2..32, pattern `^[\w+=,.@-]*$`):
/// the shortest name, one carrying the punctuation that pattern admits, and the longest.
#[tokio::test]
async fn a_refusal_names_the_federated_user_the_mint_answered_with() {
    let account = engine::emulator_account_id();
    // The session policy of every row allows the listing alone, so the object read it then
    // makes falls outside the intersection and is refused.
    let scope = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:ListAllMyBuckets","Resource":"*"}]}"#;
    for name in ["ab", "Name+With=Chars,.@-", "a_32_character_federated_usernam"] {
        let emulator = Emulator::new();
        emulator.seeded_bucket().await;
        let session = emulator
            .federated_session(name, &["s3:*"], Some(scope))
            .await;
        assert_eq!(
            session.federated_user_arn,
            format!("arn:aws:sts::{account}:federated-user/{name}"),
            "case {name:?}: the mint must answer the federated-user ARN"
        );
        let answer = Call::GetObject
            .drive(&emulator, &session.access_key_id, 0)
            .await;
        assert_verdict(
            name,
            &answer,
            Verdict::NoSessionGrant,
            Call::GetObject,
            0,
            &session.federated_user_arn,
        );
    }
}
