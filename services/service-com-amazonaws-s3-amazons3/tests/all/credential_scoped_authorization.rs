// SPDX-License-Identifier: CC0-1.0

//! What the CREDENTIAL a request presented contributes to the authorization decision: the
//! condition key saying whether it was obtained with multi-factor authentication, and the
//! session policies its mint scoped it with.
//!
//! The rule under test: **a request is authorized against every document in force on the
//! credential that signed it — the bucket's own policy, the identity-based documents of the
//! principal it speaks for, and the session policies its mint passed — over a request context
//! that answers `aws:MultiFactorAuthPresent` from that credential.** So:
//!
//! * the MFA key is `true` for a session whose mint satisfied an MFA challenge, `false` for
//!   every other session, and ABSENT for a long-term access key ("this key is present in the
//!   request context only when the principal uses temporary credentials", AWS General
//!   Reference, *Global condition context keys*) — which is why an `…IfExists` clause over it
//!   holds for a long-term key while a clause without the suffix does not, and why `Null`
//!   tells the two apart;
//! * a statement conditioned on that key therefore DECIDES the call: a `Deny` whose clause
//!   holds refuses it whatever grants it, and a conditioned `Allow` whose clause does not hold
//!   grants nothing;
//! * the session policies CAP what the identity's documents grant and grant nothing of their
//!   own ("the resulting session's permissions are the intersection of the role's
//!   identity-based policies and the session policies", `AssumeRoleRequest$Policy`), so an
//!   explicit `Deny` in one refuses, a request they do not allow is refused even where the
//!   identity allows it, and a request only they allow is refused as if they had allowed
//!   nothing.
//!
//! Every refusal is HTTP 403 `AccessDenied` carrying S3's one sentence, with the clause naming
//! the document that decided it, and a refused write leaves no object behind.
//!
//! Every assertion drives real HTTP requests through a router holding S3 beside IAM and STS
//! over ONE shared engine: the user, its access key, the role and the policies are made with
//! those services' own operations (`CreateUser` + `PutUserPolicy` + `CreateAccessKey`,
//! `CreateRole` + `PutRolePolicy` + `AssumeRole`), the plain session with
//! `sts:GetSessionToken`, and the MFA-backed one by enrolling a virtual MFA device
//! (`iam:CreateVirtualMFADevice` + `iam:EnableMFADevice`) and exchanging one of its pass codes
//! for a session — so every credential a row presents is one those services really minted.
//! Each S3 request then presents that credential's access key id.
//!
//! The tables span what the rule generalises over rather than one recorded call: which
//! credential signs (long-term key, plain session, MFA-backed session), which operator
//! compares the key (`Bool`, its `IfExists` spelling, the existence test, in both directions),
//! whether the conditioned statement is the grant or a `Deny` over a broad grant, whether the
//! document is the identity's or the bucket's, which operation is called (a listing, an object
//! read, an object write), and — for the scope — what the session policy states (nothing, a
//! narrower grant, an explicit deny, another bucket, more than the identity allows, a clause
//! over the same MFA key).

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request in this suite is signed for, and the endpoint it addresses.
const REGION: &str = "us-east-1";

/// The access-key id standing for the account's own published credential, which writes this
/// suite's fixtures: neither STS nor IAM holds a record of it, so it resolves to the account's
/// ambient identity.
const ACCOUNT_KEY: &str = "AKIDEMULATOR";

/// The bucket every row addresses.
const BUCKET: &str = "credential-scoped-subject";

/// The object every object row addresses.
const KEY: &str = "reports/held.csv";

/// The bytes that object holds. A policy is about who may reach an object, not about what is
/// in it, so one payload serves every row.
const BODY: &[u8] = b"credential-scoped-subject";

/// The code S3 answers an authorization refusal with, and the status it carries (Amazon S3
/// "Error responses": `AccessDenied`, HTTP 403).
const ACCESS_DENIED: &str = "AccessDenied";

/// How long a session minted here lasts — comfortably longer than the suite's own run, so no
/// row is decided by a credential that expired mid-test.
const SESSION_SECONDS: i32 = 3600;

/// An emulator holding S3 beside the two services a decision about a credential reads from:
/// IAM (the principal and its identity-based documents) and STS (the session a credential was
/// minted for).
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
        self.write_fixture().await;
    }

    /// Write [`KEY`] as the account's own credential — the fixture every read row addresses.
    async fn write_fixture(&self) {
        let written = self
            .s3(ACCOUNT_KEY, "PUT", &format!("/{BUCKET}/{KEY}"), BODY)
            .await;
        assert_eq!(
            written.status, 200,
            "the fixture must be stored first, got {} with {}",
            written.status, written.body
        );
    }

    /// `PUT /<bucket>?policy` as the account — the resource-based policy under test. The answer
    /// carries no content, which is the status the policy suites of this crate pin.
    async fn put_bucket_policy(&self, document: &str) {
        let stored = self
            .s3(
                ACCOUNT_KEY,
                "PUT",
                &format!("/{BUCKET}?policy"),
                document.as_bytes(),
            )
            .await;
        assert_eq!(
            stored.status, 204,
            "the bucket policy {document} must be stored, got {} with {}",
            stored.status, stored.body
        );
    }

    /// An IAM user carrying exactly `document` as its inline policy, and the three credentials
    /// a row calls as.
    async fn user_with_document(&self, name: &str, document: &str) -> Credentials {
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
        let arn = element(&body, "Arn")
            .unwrap_or_else(|| panic!("CreateUser must answer the user Arn, got {body}"));
        let (status, body) = self
            .query(
                ACCOUNT_KEY,
                "iam",
                "2010-05-08",
                "PutUserPolicy",
                &[
                    ("UserName", name.to_string()),
                    ("PolicyName", "scoped".to_string()),
                    ("PolicyDocument", document.to_string()),
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
        let plain_session = self.plain_session(&long_term).await;
        let mfa_session = self.mfa_session(name, &long_term).await;
        Credentials {
            arn,
            long_term,
            plain_session,
            mfa_session,
        }
    }

    /// `sts:GetSessionToken` under `access_key_id`, presenting NO MFA challenge: a temporary
    /// credential of the same identity, for which the MFA key answers `false`.
    async fn plain_session(&self, access_key_id: &str) -> String {
        let (status, body) = self
            .query(
                access_key_id,
                "sts",
                "2011-06-15",
                "GetSessionToken",
                &[("DurationSeconds", SESSION_SECONDS.to_string())],
            )
            .await;
        assert_eq!(
            status, 200,
            "GetSessionToken must mint a session, got {status} {body}"
        );
        element(&body, "AccessKeyId")
            .unwrap_or_else(|| panic!("GetSessionToken must answer an AccessKeyId, got {body}"))
    }

    /// A session of the same identity whose mint SATISFIED an MFA challenge — the credential
    /// for which the MFA key answers `true`.
    ///
    /// Driven exactly as the holder of a device drives it: the account provisions a virtual MFA
    /// device (`iam:CreateVirtualMFADevice`), enables it for the user with two codes in a row
    /// (`iam:EnableMFADevice`), and the user exchanges one more code for a session
    /// (`sts:GetSessionToken` with `SerialNumber` and `TokenCode`). The codes are computed here
    /// from RFC 6238's definition over the seed the create answered with, so nothing about the
    /// credential is planted in a store.
    async fn mfa_session(&self, user_name: &str, long_term: &str) -> String {
        let (status, body) = self
            .query(
                ACCOUNT_KEY,
                "iam",
                "2010-05-08",
                "CreateVirtualMFADevice",
                &[("VirtualMFADeviceName", format!("{user_name}-device"))],
            )
            .await;
        assert_eq!(
            status, 200,
            "a virtual MFA device must be provisioned, got {status} {body}"
        );
        let serial_number = element(&body, "SerialNumber")
            .unwrap_or_else(|| panic!("the create must answer a SerialNumber, got {body}"));
        // `Base32StringSeed` is a blob, so the wire carries it base64-encoded.
        let encoded = element(&body, "Base32StringSeed")
            .unwrap_or_else(|| panic!("the create must answer the seed, got {body}"));
        let seed = String::from_utf8(
            data_encoding::BASE64
                .decode(encoded.as_bytes())
                .unwrap_or_else(|error| panic!("the seed must be base64: {error}")),
        )
        .expect("the seed is base32 text");
        let step = current_time_step();
        let (status, body) = self
            .query(
                ACCOUNT_KEY,
                "iam",
                "2010-05-08",
                "EnableMFADevice",
                &[
                    ("UserName", user_name.to_string()),
                    ("SerialNumber", serial_number.clone()),
                    ("AuthenticationCode1", pass_code(&seed, step)),
                    ("AuthenticationCode2", pass_code(&seed, step + 1)),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "the device must be enabled for {user_name:?}, got {status} {body}"
        );
        let (status, body) = self
            .query(
                long_term,
                "sts",
                "2011-06-15",
                "GetSessionToken",
                &[
                    ("DurationSeconds", SESSION_SECONDS.to_string()),
                    ("SerialNumber", serial_number),
                    ("TokenCode", pass_code(&seed, current_time_step())),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "the MFA-backed mint must succeed, got {status} {body}"
        );
        element(&body, "AccessKeyId")
            .unwrap_or_else(|| panic!("GetSessionToken must answer an AccessKeyId, got {body}"))
    }

    /// Create an IAM role carrying `document` as its inline policy, assume it with the session
    /// policy `scope` (when one is given), and answer the session that assume minted.
    async fn assumed_role(&self, role_name: &str, document: &str, scope: Option<&str>) -> Session {
        let account = engine::emulator_account_id();
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
                ACCOUNT_KEY,
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
                ACCOUNT_KEY,
                "iam",
                "2010-05-08",
                "PutRolePolicy",
                &[
                    ("RoleName", role_name.to_string()),
                    ("PolicyName", "inline".to_string()),
                    ("PolicyDocument", document.to_string()),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "the inline policy of {role_name:?} must be stored, got {status} {body}"
        );
        let mut params = vec![
            ("RoleArn", format!("arn:aws:iam::{account}:role/{role_name}")),
            ("RoleSessionName", "scoped".to_string()),
            ("DurationSeconds", SESSION_SECONDS.to_string()),
        ];
        if let Some(scope) = scope {
            params.push(("Policy", scope.to_string()));
        }
        let (status, body) = self
            .query(ACCOUNT_KEY, "sts", "2011-06-15", "AssumeRole", &params)
            .await;
        assert_eq!(
            status, 200,
            "assuming {role_name:?} must succeed, got {status} {body}"
        );
        Session {
            access_key_id: element(&body, "AccessKeyId")
                .unwrap_or_else(|| panic!("AssumeRole must answer an AccessKeyId, got {body}")),
            arn: element(&body, "Arn")
                .unwrap_or_else(|| panic!("AssumeRole must answer the session Arn, got {body}")),
        }
    }
}

/// The three credentials of one IAM user, which differ only in what the MFA condition key
/// answers for them.
struct Credentials {
    /// The user's own ARN, which every refusal of any of the three names.
    arn: String,
    /// The user's long-term access key: the MFA key carries NO value for it.
    long_term: String,
    /// A session minted without an MFA challenge: the key answers `false`.
    plain_session: String,
    /// A session whose mint satisfied an MFA challenge: the key answers `true`.
    mfa_session: String,
}

/// One assumed-role session: the access key id its requests are signed with, and the ARN a
/// refusal names it by.
struct Session {
    access_key_id: String,
    arn: String,
}

/// WHICH credential a row's expectation is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Presented {
    /// The user's long-term access key, for which the MFA key is absent.
    LongTermKey,
    /// A session minted with no MFA challenge, for which it is `false`.
    PlainSession,
    /// A session whose mint satisfied one, for which it is `true`.
    MfaSession,
}

impl Presented {
    /// The access key id this credential signs with.
    fn access_key_id(self, credentials: &Credentials) -> &str {
        match self {
            Self::LongTermKey => &credentials.long_term,
            Self::PlainSession => &credentials.plain_session,
            Self::MfaSession => &credentials.mfa_session,
        }
    }
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
}

impl Call {
    /// The IAM action a grant has to name for this call ("you must have the `s3:PutObject`
    /// permission", `PutObject`; "you must have the `READ` access to the object" —
    /// `s3:GetObject`, `GetObject`; "you must have the `s3:ListBucket` permission",
    /// `ListObjectsV2`).
    fn action(self) -> &'static str {
        match self {
            Self::ListBucket => "s3:ListBucket",
            Self::GetObject => "s3:GetObject",
            Self::PutObject => "s3:PutObject",
        }
    }

    /// The resource a refusal of this call names: the bucket for the listing, the object for
    /// the two object calls.
    fn resource(self) -> String {
        match self {
            Self::ListBucket => format!("arn:aws:s3:::{BUCKET}"),
            Self::GetObject | Self::PutObject => format!("arn:aws:s3:::{BUCKET}/{KEY}"),
        }
    }

    /// Make the call as `access_key_id`.
    async fn drive(self, emulator: &Emulator, access_key_id: &str) -> Answer {
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
        }
    }
}

/// What the documents in force answer one call: served, or refused with the clause naming the
/// document that decided it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// The call is authorized.
    Served,
    /// Refused by an explicit `Deny` in one of the identity's own documents.
    IdentityDeny,
    /// Refused by an explicit `Deny` in the bucket's own policy.
    ResourceDeny,
    /// Refused because nothing in the identity's documents granted the action.
    NoIdentityGrant,
    /// Refused by an explicit `Deny` in a session policy.
    SessionPolicyDeny,
    /// Refused because the session policies the credential was minted with do not allow the
    /// action, whatever the identity's documents allow.
    NoSessionGrant,
}

impl Verdict {
    /// The reason clause S3's refusal sentence carries for this verdict, for the IAM `action`
    /// asked about. These are IAM's own published clauses, one per document that can withhold
    /// or withdraw a grant.
    fn clause(self, action: &str) -> Option<String> {
        match self {
            Self::Served => None,
            Self::IdentityDeny => {
                Some("with an explicit deny in an identity-based policy".to_string())
            }
            Self::ResourceDeny => {
                Some("with an explicit deny in a resource-based policy".to_string())
            }
            Self::NoIdentityGrant => Some(format!(
                "because no identity-based policy allows the {action} action"
            )),
            Self::SessionPolicyDeny => {
                Some("with an explicit deny in a session policy".to_string())
            }
            Self::NoSessionGrant => Some(format!(
                "because no session policy allows the {action} action"
            )),
        }
    }
}

/// Assert that `answer` is what `verdict` says, for the call `call` made by `principal`.
fn assert_verdict(case: &str, answer: &Answer, verdict: Verdict, call: Call, principal: &str) {
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
                call.resource()
            );
            let message = answer.error_message();
            assert_eq!(
                message, expected,
                "case {case:?} ({call:?}): expected {expected:?}, got {message:?}"
            );
        }
    }
}

/// One row of the MFA table: the conditioned statement the user's only policy carries, and
/// what each of the three credentials is answered.
struct MfaRow {
    /// What to call the case in a failure.
    case: &'static str,
    /// The `Effect` of the CONDITIONED statement.
    effect: &'static str,
    /// Its `Condition` object, as JSON text.
    condition: &'static str,
    /// Whether the document also carries a broad unconditioned `Allow`, which is what makes a
    /// conditioned `Deny` row a decision about the deny rather than about an absent grant.
    broad_allow: bool,
    /// The call the row makes.
    call: Call,
    /// What each credential is answered, in [`PRESENTED`] order.
    verdicts: [Verdict; 3],
}

/// The credentials every row of the MFA table is driven with, in the order its verdicts are
/// written in.
const PRESENTED: [Presented; 3] = [
    Presented::LongTermKey,
    Presented::PlainSession,
    Presented::MfaSession,
];

/// The MFA table.
const MFA_ROWS: &[MfaRow] = &[
    MfaRow {
        case: "a deny unless MFA is present, above a broad grant",
        effect: "Deny",
        condition: r#"{"BoolIfExists":{"aws:MultiFactorAuthPresent":"false"}}"#,
        broad_allow: true,
        call: Call::PutObject,
        verdicts: [
            Verdict::IdentityDeny,
            Verdict::IdentityDeny,
            Verdict::Served,
        ],
    },
    MfaRow {
        case: "the same deny, on an object read",
        effect: "Deny",
        condition: r#"{"BoolIfExists":{"aws:MultiFactorAuthPresent":"false"}}"#,
        broad_allow: true,
        call: Call::GetObject,
        verdicts: [
            Verdict::IdentityDeny,
            Verdict::IdentityDeny,
            Verdict::Served,
        ],
    },
    MfaRow {
        case: "the same deny, on a bucket listing",
        effect: "Deny",
        condition: r#"{"BoolIfExists":{"aws:MultiFactorAuthPresent":"false"}}"#,
        broad_allow: true,
        call: Call::ListBucket,
        verdicts: [
            Verdict::IdentityDeny,
            Verdict::IdentityDeny,
            Verdict::Served,
        ],
    },
    MfaRow {
        // Without the suffix the clause compares a value, and a credential that carries none
        // is not compared at all: the trap `…IfExists` exists for.
        case: "a deny whose clause does not tolerate a missing key",
        effect: "Deny",
        condition: r#"{"Bool":{"aws:MultiFactorAuthPresent":"false"}}"#,
        broad_allow: true,
        call: Call::PutObject,
        verdicts: [Verdict::Served, Verdict::IdentityDeny, Verdict::Served],
    },
    MfaRow {
        case: "a grant that requires MFA",
        effect: "Allow",
        condition: r#"{"Bool":{"aws:MultiFactorAuthPresent":"true"}}"#,
        broad_allow: false,
        call: Call::GetObject,
        verdicts: [
            Verdict::NoIdentityGrant,
            Verdict::NoIdentityGrant,
            Verdict::Served,
        ],
    },
    MfaRow {
        case: "a grant that tolerates a missing key",
        effect: "Allow",
        condition: r#"{"BoolIfExists":{"aws:MultiFactorAuthPresent":"false"}}"#,
        broad_allow: false,
        call: Call::GetObject,
        verdicts: [
            Verdict::Served,
            Verdict::Served,
            Verdict::NoIdentityGrant,
        ],
    },
    MfaRow {
        case: "a grant demanding the key be absent",
        effect: "Allow",
        condition: r#"{"Null":{"aws:MultiFactorAuthPresent":"true"}}"#,
        broad_allow: false,
        call: Call::PutObject,
        verdicts: [
            Verdict::Served,
            Verdict::NoIdentityGrant,
            Verdict::NoIdentityGrant,
        ],
    },
    MfaRow {
        case: "a deny over every credential that answers the key, above a broad grant",
        effect: "Deny",
        condition: r#"{"Null":{"aws:MultiFactorAuthPresent":"false"}}"#,
        broad_allow: true,
        call: Call::ListBucket,
        verdicts: [
            Verdict::Served,
            Verdict::IdentityDeny,
            Verdict::IdentityDeny,
        ],
    },
];

/// The two ARNs a statement about [`BUCKET`] and its objects names.
fn bucket_resources() -> serde_json::Value {
    json!([
        format!("arn:aws:s3:::{BUCKET}"),
        format!("arn:aws:s3:::{BUCKET}/*")
    ])
}

/// The identity document one MFA row's user carries: its conditioned statement over the bucket
/// and its objects, preceded by the broad grant the row asks for.
fn mfa_document(row: &MfaRow) -> String {
    let condition: serde_json::Value =
        serde_json::from_str(row.condition).expect("the row's condition must be JSON");
    let conditioned = json!({
        "Effect": row.effect,
        "Action": "s3:*",
        "Resource": bucket_resources(),
        "Condition": condition,
    });
    let mut statements = Vec::new();
    if row.broad_allow {
        statements.push(json!({
            "Effect": "Allow",
            "Action": "s3:*",
            "Resource": bucket_resources(),
        }));
    }
    statements.push(conditioned);
    json!({"Version": "2012-10-17", "Statement": statements}).to_string()
}

/// RULE: a statement conditioned on `aws:MultiFactorAuthPresent` is decided by the credential
/// that signed the request — the key is `true` for a session whose mint satisfied an MFA
/// challenge, `false` for every other session and absent for a long-term key — so the deny
/// refuses exactly the credentials its clause holds for, and the conditioned grant authorizes
/// exactly those.
#[tokio::test]
async fn a_clause_over_the_mfa_key_is_decided_by_the_credential() {
    for (index, row) in MFA_ROWS.iter().enumerate() {
        let emulator = Emulator::new();
        emulator.seeded_bucket().await;
        let credentials = emulator
            .user_with_document(&format!("scoped{index}"), &mfa_document(row))
            .await;
        for (presented, verdict) in PRESENTED.into_iter().zip(row.verdicts) {
            let case = format!("{} / {presented:?}", row.case);
            let answer = row
                .call
                .drive(&emulator, presented.access_key_id(&credentials))
                .await;
            assert_verdict(&case, &answer, verdict, row.call, &credentials.arn);
            // A served write replaces the fixture the read rows address, so the next
            // credential of the row starts from the same object.
            if row.call == Call::PutObject && verdict == Verdict::Served {
                emulator.write_fixture().await;
            }
        }
    }
}

/// RULE, on the resource-based half: the BUCKET's own policy is judged over the same context,
/// so a statement naming the principal and conditioned on the MFA key refuses the credentials
/// its clause holds for — and a refused write leaves no object behind.
#[tokio::test]
async fn a_bucket_policy_deny_unless_mfa_refuses_and_writes_nothing() {
    let emulator = Emulator::new();
    let created = emulator
        .s3(ACCOUNT_KEY, "PUT", &format!("/{BUCKET}"), b"")
        .await;
    assert_eq!(
        created.status, 200,
        "the bucket must be created first, got {} with {}",
        created.status, created.body
    );
    let grant_everything = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": "s3:*",
            "Resource": bucket_resources(),
        }],
    })
    .to_string();
    let credentials = emulator
        .user_with_document("bucket-policy-subject", &grant_everything)
        .await;
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Deny",
            "Principal": {"AWS": credentials.arn.clone()},
            "Action": "s3:*",
            "Resource": bucket_resources(),
            "Condition": {"BoolIfExists": {"aws:MultiFactorAuthPresent": "false"}},
        }],
    })
    .to_string();
    emulator.put_bucket_policy(&policy).await;

    for (presented, verdict) in PRESENTED.into_iter().zip([
        Verdict::ResourceDeny,
        Verdict::ResourceDeny,
        Verdict::Served,
    ]) {
        let case = format!("a bucket-policy deny unless MFA / {presented:?}");
        let answer = Call::PutObject
            .drive(&emulator, presented.access_key_id(&credentials))
            .await;
        assert_verdict(&case, &answer, verdict, Call::PutObject, &credentials.arn);
        // The write either happened or did not: the account's own HEAD is what says which,
        // and a refused write must leave the key absent.
        let head = emulator
            .s3(ACCOUNT_KEY, "HEAD", &format!("/{BUCKET}/{KEY}"), b"")
            .await;
        let expected = if verdict == Verdict::Served { 200 } else { 404 };
        assert_eq!(
            head.status, expected,
            "case {case:?}: expected the owner's HEAD to answer {expected}, got {} with {}",
            head.status, head.body
        );
    }
}

/// One row of the session-policy table: what the ROLE's own document grants, what the mint
/// scoped the credential with, the call made and what it is answered.
struct ScopeRow {
    case: &'static str,
    /// The actions the role's inline policy allows on the bucket and its objects.
    role_actions: &'static str,
    /// The session policy the mint passed, as JSON text, or `None` for a mint that passed
    /// none.
    scope: Option<&'static str>,
    call: Call,
    verdict: Verdict,
}

/// The session-policy table.
const SCOPE_ROWS: &[ScopeRow] = &[
    ScopeRow {
        case: "a mint that passed no session policy",
        role_actions: "s3:*",
        scope: None,
        call: Call::PutObject,
        verdict: Verdict::Served,
    },
    ScopeRow {
        case: "a session policy allowing just the read, on the read",
        role_actions: "s3:*",
        scope: Some(r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#),
        call: Call::GetObject,
        verdict: Verdict::Served,
    },
    ScopeRow {
        case: "a session policy allowing just the read, on the write",
        role_actions: "s3:*",
        scope: Some(r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#),
        call: Call::PutObject,
        verdict: Verdict::NoSessionGrant,
    },
    ScopeRow {
        case: "a session policy denying the write above its own broad grant",
        role_actions: "s3:*",
        scope: Some(r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:*","Resource":"*"},{"Effect":"Deny","Action":"s3:PutObject","Resource":"*"}]}"#),
        call: Call::PutObject,
        verdict: Verdict::SessionPolicyDeny,
    },
    ScopeRow {
        case: "a session policy scoped to another bucket's objects",
        role_actions: "s3:*",
        scope: Some(r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:*","Resource":"arn:aws:s3:::somewhere-else/*"}]}"#),
        call: Call::GetObject,
        verdict: Verdict::NoSessionGrant,
    },
    ScopeRow {
        // A session policy grants nothing of its own: what the role may not do, the session
        // may not do either, however wide the scope is written.
        case: "a session policy wider than the role's own grant",
        role_actions: "s3:GetObject",
        scope: Some(r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:*","Resource":"*"}]}"#),
        call: Call::PutObject,
        verdict: Verdict::NoIdentityGrant,
    },
    ScopeRow {
        // The scope is judged over the same request context as every other document, so a
        // clause over the MFA key decides it too: this session's mint satisfied no challenge.
        case: "a session policy conditioned on MFA, under a session minted without it",
        role_actions: "s3:*",
        scope: Some(r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:*","Resource":"*","Condition":{"Bool":{"aws:MultiFactorAuthPresent":"true"}}}]}"#),
        call: Call::GetObject,
        verdict: Verdict::NoSessionGrant,
    },
    ScopeRow {
        case: "a session policy whose listing grant covers the bucket",
        role_actions: "s3:*",
        scope: Some(r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:ListBucket","Resource":"*"}]}"#),
        call: Call::ListBucket,
        verdict: Verdict::Served,
    },
];

/// RULE: the session policies a credential was minted with cap what the documents in force on
/// its identity grant and grant nothing of their own — an explicit deny in one refuses, a
/// request they do not allow is refused even where the identity allows it, and a request only
/// they allow is refused as if they had allowed nothing.
#[tokio::test]
async fn a_session_policy_caps_what_the_identity_grants() {
    for (index, row) in SCOPE_ROWS.iter().enumerate() {
        let emulator = Emulator::new();
        emulator.seeded_bucket().await;
        let document = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Action": row.role_actions,
                "Resource": bucket_resources(),
            }],
        })
        .to_string();
        let session = emulator
            .assumed_role(&format!("scoped-role{index}"), &document, row.scope)
            .await;
        let answer = row.call.drive(&emulator, &session.access_key_id).await;
        assert_verdict(row.case, &answer, row.verdict, row.call, &session.arn);
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

/// The text of the first `<name>…</name>` element of a response body, with the XML escaping a
/// serializer applies undone.
///
/// `pub(crate)`: the federated-session suite ([`super::federated_session_authorization`]) reads
/// a refusal's own `<Code>` and `<Message>` — whose resource ARN is quoted, so the escaping has
/// to come off — through this one reader rather than slicing the document a second way.
pub(crate) fn element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let (_, rest) = body.split_once(&open)?;
    let (text, _) = rest.split_once(&close)?;
    Some(
        text.replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&"),
    )
}

/// How long one time step of a virtual MFA device's pass code lasts: RFC 6238's recommended
/// default, which an authenticator app provisioned from an `otpauth://totp/` URI naming no
/// period uses.
const TIME_STEP: Duration = Duration::from_secs(30);

/// The time step this instant falls in — the TOTP counter.
fn current_time_step() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is past the epoch")
        .as_secs()
        / TIME_STEP.as_secs()
}

/// The pass code a device provisioned with `seed` emits in the time step `step`, computed here
/// rather than asked of either service: RFC 6238's TOTP over RFC 4226's HOTP — HMAC-SHA-1 of
/// the counter, dynamically truncated and taken to the six digits an authentication code
/// carries.
fn pass_code(seed: &str, step: u64) -> String {
    use hmac::Mac;

    let key = data_encoding::BASE32_NOPAD
        .decode(seed.trim_end_matches('=').as_bytes())
        .unwrap_or_else(|error| panic!("the seed must be base32, got {seed:?}: {error}"));
    let mut mac =
        hmac::Hmac::<sha1::Sha1>::new_from_slice(&key).expect("HMAC takes a key of any length");
    mac.update(&step.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = usize::from(digest[digest.len() - 1] & 0x0f);
    let truncated = u32::from_be_bytes([
        digest[offset] & 0x7f,
        digest[offset + 1],
        digest[offset + 2],
        digest[offset + 3],
    ]);
    format!("{:06}", truncated % 1_000_000)
}
