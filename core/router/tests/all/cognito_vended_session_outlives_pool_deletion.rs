// SPDX-License-Identifier: CC0-1.0

//! An Amazon S3 read made with credentials an Amazon Cognito identity pool vended, after the
//! pool that vended them is torn down — on an emulator that VERIFIES request signatures.
//!
//! RULE: what `GetCredentialsForIdentity` hands back is an `sts:AssumeRoleWithWebIdentity`
//! session of the role the pool resolved, and that session is bound to the ROLE and to its
//! own `Expiration` — not to the pool record or the identity record the exchange went
//! through. So no call of the identity pool's own lifecycle revokes it: a request signed with
//! it keeps authenticating, keeps resolving to the same assumed-role principal, and keeps
//! being authorized exactly as the documents in force on that role allow, until the session
//! expires. Removing the pool stops a further VEND, which is a different effect from
//! revoking a credential already in a caller's hands. Where that same session IS refused —
//! once the role's only `Allow` is gone — the refusal is Amazon S3's own authorization
//! answer: HTTP 403 `AccessDenied` whose sentence names the session's principal, the action
//! and the object.
//!
//! The rows differ along WHICH call of the lifecycle follows the vend: the pool deleted, and
//! the identity the session was vended for deleted. Each row reads the object three times
//! afterwards, because a read of a session is not a consumption of it, and then — with the
//! role's only grant removed — once more, where a refusal is finally due.
//!
//! Only the assembled router can pin this, and only with request authentication ON: the
//! credential the read signs with is verified by the EMULATOR's own credential store, the
//! principal behind it is resolved from the token service's session registry, the role and
//! its inline document are IAM's, the pool and its roles are Amazon Cognito Identity's, and
//! the decision is Amazon S3's. Every setup call signs with the key this process publishes;
//! the vend legs carry no credential scope at all, as their model lists no auth scheme; and
//! the reads are signed with the vended session, as a client built from those credentials
//! signs them.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread.

use std::sync::Arc;
use std::time::SystemTime;

use aws_credential_types::Credentials;
use aws_emulator_router::{build_router, InMemoryEngine};
use aws_sigv4::http_request::{sign, SignableBody, SignableRequest, SigningSettings};
use aws_sigv4::sign::v4::SigningParams;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, elements, form_encode};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The Region every call below is signed for, and so the Region the pool lives in.
const REGION: &str = "us-east-1";

/// The emulator's own address: an address, so the `Host` header of an unsigned vend names no
/// service and the request carries its service name in `X-Amz-Target` alone.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// The account this emulator serves, which the principal in every refusal carries.
const ACCOUNT: &str = ::engine::EMULATOR_ACCOUNT_ID;

/// The `RoleSessionName` a session an identity pool vends carries, written out here as AWS
/// publishes it in such a session's caller ARN rather than read back from the implementation.
const VENDED_ROLE_SESSION_NAME: &str = "CognitoIdentityCredentials";

/// The bucket and the one object every row reads.
const BUCKET: &str = "xsvc-cognito-outlives-delete";
const KEY: &str = "reports/the-vended-read.txt";
const PAYLOAD: &str = "the bytes the vended session is allowed to read";

/// The inline policy name the row's role carries its only grant under.
const POLICY_NAME: &str = "the-only-allow";

/// The code and status Amazon S3 refuses an unauthorized read with (Amazon S3 "Error
/// responses").
const ACCESS_DENIED: &str = "AccessDenied";

/// The IAM action a read of an object's current version is authorized under.
const GET_OBJECT_ACTION: &str = "s3:GetObject";

/// The clause IAM's refusal sentence carries when no document grants the action at all.
const NO_GRANT: &str = "because no identity-based policy allows the s3:GetObject action";

/// The identity-provider name a login carrying a token this service minted is handed over
/// under (`GetCredentialsForIdentityInput$Logins`).
const COGNITO_IDENTITY_PROVIDER: &str = "cognito-identity.amazonaws.com";

/// The IAM API version every awsQuery call to it carries.
const IAM_API_VERSION: &str = "2010-05-08";

/// How many times a row reads the object back after its lifecycle call.
const READS_AFTER: usize = 3;

/// What one call answered.
struct Answer {
    status: u16,
    body: bytes::Bytes,
}

impl Answer {
    fn json(&self) -> Value {
        if self.body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&self.body).unwrap_or(Value::Null)
        }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    /// One element's text with the serializer's escaping undone — a refusal's `<Message>`
    /// quotes the resource, and those quotes travel as `&quot;`.
    fn element(&self, name: &str) -> Option<String> {
        elements(&self.text(), name).into_iter().next()
    }
}

/// The credential the setup calls sign with: the pair this process publishes once request
/// authentication is on.
fn published() -> &'static (String, String) {
    static PUBLISHED: std::sync::OnceLock<(String, String)> = std::sync::OnceLock::new();
    PUBLISHED.get_or_init(|| {
        let minted = ::emulator_server::SigV4Authenticator::minted_credentials()
            .expect("turning request authentication on publishes this process's keys");
        (
            minted.access_key_id.clone(),
            minted.secret_access_key.clone(),
        )
    })
}

/// The credential a request is signed with: the key pair this process publishes, or a session
/// a vend handed back — which is the pair plus the session token that travels with it.
#[derive(Clone)]
enum SignedWith {
    Published,
    Session {
        access_key_id: String,
        secret_key: String,
        session_token: String,
    },
}

impl SignedWith {
    fn identity(&self) -> Credentials {
        match self {
            Self::Published => {
                let (key_id, secret) = published();
                Credentials::new(key_id, secret, None, None, "test")
            }
            Self::Session {
                access_key_id,
                secret_key,
                session_token,
            } => Credentials::new(
                access_key_id,
                secret_key,
                Some(session_token.clone()),
                None,
                "test",
            ),
        }
    }
}

/// One request to make: the service it is addressed to and signed for, and what it carries.
struct Wire<'a> {
    signing_name: &'a str,
    method: &'a str,
    path: &'a str,
    headers: Vec<(&'a str, String)>,
    body: Vec<u8>,
}

/// Dispatch one request SIGNED for its service with `credential`, the way an SDK signs it:
/// the signer writes the `Authorization` header, the `x-amz-date` and — for a session — the
/// `x-amz-security-token` the emulator checks beside the signature.
fn signed_call(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    credential: &SignedWith,
    wire: Wire<'_>,
) -> Answer {
    let signing_name = wire.signing_name;
    let host = format!("{signing_name}.{REGION}.amazonaws.com");
    let mut builder = http::Request::builder()
        .method(wire.method)
        .uri(format!("https://{host}{}", wire.path))
        .header("host", host.clone());
    for (name, value) in &wire.headers {
        builder = builder.header(*name, value.as_str());
    }
    let mut request = builder.body(wire.body).expect("a well-formed request");
    let identity = credential.identity().into();
    let params = SigningParams::builder()
        .identity(&identity)
        .region(REGION)
        .name(signing_name)
        .time(SystemTime::now())
        .settings(SigningSettings::default())
        .build()
        .expect("the signing parameters are complete")
        .into();
    let (method, uri) = (request.method().to_string(), request.uri().to_string());
    let header_pairs: Vec<(String, String)> = request
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                value.to_str().expect("a text header").to_string(),
            )
        })
        .collect();
    let signable = SignableRequest::new(
        &method,
        &uri,
        header_pairs
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str())),
        SignableBody::Bytes(request.body()),
    )
    .expect("the request is signable");
    let (instructions, _) = sign(signable, &params)
        .expect("the request signs")
        .into_parts();
    instructions.apply_to_request_http1x(&mut request);
    let (parts, payload) = request.into_parts();
    let response = runtime.block_on(
        router.dispatch(http::Request::from_parts(parts, bytes::Bytes::from(payload))),
    );
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        body,
    }
}

/// One awsJson1_1 call to Amazon Cognito Identity, signed with the published key.
fn cognito(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    operation: &str,
    body: &Value,
) -> Answer {
    signed_call(
        router,
        runtime,
        &SignedWith::Published,
        Wire {
            signing_name: "cognito-identity",
            method: "POST",
            path: "/",
            headers: vec![
                ("content-type", "application/x-amz-json-1.1".to_string()),
                (
                    "x-amz-target",
                    format!("AWSCognitoIdentityService.{operation}"),
                ),
            ],
            body: body.to_string().into_bytes(),
        },
    )
}

/// One UNSIGNED awsJson1_1 call to Amazon Cognito Identity: no `Authorization` header at all,
/// addressed by the emulator's own authority, which is how a client holding no Amazon Web
/// Services credential sends an operation whose model lists no auth scheme.
fn unsigned_cognito(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    operation: &str,
    body: &Value,
) -> Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{EMULATOR_HOST}/"))
        .header("host", EMULATOR_HOST)
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityService.{operation}"),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        body,
    }
}

/// One awsQuery call to IAM, with every component percent-encoded so a policy document
/// travels verbatim.
fn iam(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    action: &str,
    members: &[(&str, &str)],
) -> Answer {
    let mut body = format!("Action={action}&Version={IAM_API_VERSION}");
    for (name, value) in members {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    signed_call(
        router,
        runtime,
        &SignedWith::Published,
        Wire {
            signing_name: "iam",
            method: "POST",
            path: "/",
            headers: vec![(
                "content-type",
                "application/x-www-form-urlencoded".to_string(),
            )],
            body: body.into_bytes(),
        },
    )
}

/// One Amazon S3 call on the path-style address, signed with `credential`.
fn s3(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    credential: &SignedWith,
    method: &str,
    path: &str,
    body: &str,
) -> Answer {
    signed_call(
        router,
        runtime,
        credential,
        Wire {
            signing_name: "s3",
            method,
            path,
            headers: vec![("content-type", "application/xml".to_string())],
            body: body.as_bytes().to_vec(),
        },
    )
}

/// The trust policy of a role an identity pool vends sessions of, written the way the Amazon
/// Cognito Developer Guide writes one: the `Federated` Cognito Identity principal, the
/// web-identity assume, and the `:aud` condition restricting the assume to that pool's users.
fn pool_trust_policy(pool_id: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Federated": COGNITO_IDENTITY_PROVIDER},
            "Action": "sts:AssumeRoleWithWebIdentity",
            "Condition": {"StringEquals": {
                format!("{COGNITO_IDENTITY_PROVIDER}:aud"): pool_id,
            }},
        }],
    })
    .to_string()
}

/// The role's ONE inline grant: `s3:GetObject` on the one object the rows read.
fn only_allow() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": GET_OBJECT_ACTION,
            "Resource": object_arn(),
        }],
    })
    .to_string()
}

/// The ARN of the object the rows read, as a policy and a refusal name it.
fn object_arn() -> String {
    format!("arn:aws:s3:::{BUCKET}/{KEY}")
}

/// Which call of the lifecycle a row makes after the vend.
#[derive(Clone, Copy, Debug)]
enum Lifecycle {
    /// `DeleteIdentityPool` on the pool the session was vended by.
    PoolDeleted,
    /// `DeleteIdentities` naming the identity the session was vended for.
    IdentityDeleted,
}

/// The session a vend handed back, as a client reaches it: `GetId` then
/// `GetCredentialsForIdentity`, both sent with NO credential scope.
fn vend(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    case: &str,
    pool_id: &str,
) -> (String, SignedWith) {
    let identity = unsigned_cognito(
        router,
        runtime,
        "GetId",
        &json!({"IdentityPoolId": pool_id}),
    );
    assert_eq!(
        identity.status,
        200,
        "case {case:?}: an unsigned GetId must be served, got {} with {:?}",
        identity.status,
        identity.text()
    );
    let identity_id = identity.json()["IdentityId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: GetId reports an IdentityId"))
        .to_string();

    let vended = unsigned_cognito(
        router,
        runtime,
        "GetCredentialsForIdentity",
        &json!({"IdentityId": &identity_id}),
    );
    assert_eq!(
        vended.status,
        200,
        "case {case:?}: an unsigned GetCredentialsForIdentity must be served, got {} with {:?}",
        vended.status,
        vended.text()
    );
    let credentials = vended.json();
    let credentials = &credentials["Credentials"];
    let member = |name: &str| {
        credentials[name]
            .as_str()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the vend must carry a non-empty Credentials.{name}, got {}",
                    vended.text()
                )
            })
            .to_string()
    };
    (
        identity_id,
        SignedWith::Session {
            access_key_id: member("AccessKeyId"),
            secret_key: member("SecretKey"),
            session_token: member("SessionToken"),
        },
    )
}

/// Read the object with `credential` and assert it is answered the payload.
fn read_is_served(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    label: &str,
    credential: &SignedWith,
) {
    let answer = s3(
        router,
        runtime,
        credential,
        "GET",
        &format!("/{BUCKET}/{KEY}"),
        "",
    );
    assert_eq!(
        (answer.status, answer.text().as_str()),
        (200, PAYLOAD),
        "case {label:?}: the read must be served the object's bytes, got {} with {:?}",
        answer.status,
        answer.text()
    );
}

/// Read the object with `credential` and assert Amazon S3's own authorization refusal,
/// naming the session `role_name` is a session of.
fn read_is_refused(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    label: &str,
    credential: &SignedWith,
    role_name: &str,
) {
    let answer = s3(
        router,
        runtime,
        credential,
        "GET",
        &format!("/{BUCKET}/{KEY}"),
        "",
    );
    assert_eq!(
        answer.status,
        403,
        "case {label:?}: the read must be refused 403, got {} with {:?}",
        answer.status,
        answer.text()
    );
    assert_eq!(
        answer.element("Code").as_deref(),
        Some(ACCESS_DENIED),
        "case {label:?}: expected the code {ACCESS_DENIED}, got {:?}",
        answer.text()
    );
    let expected = format!(
        "User: arn:aws:sts::{ACCOUNT}:assumed-role/{role_name}/{VENDED_ROLE_SESSION_NAME} is \
         not authorized to perform: {GET_OBJECT_ACTION} on resource: \"{}\" {NO_GRANT}",
        object_arn()
    );
    assert_eq!(
        answer.element("Message").as_deref(),
        Some(expected.as_str()),
        "case {label:?}: expected the refusal {expected:?}, got {:?}",
        answer.text()
    );
}

#[test]
fn a_vended_session_reads_on_after_its_pool_is_torn_down() {
    std::thread::Builder::new()
        .name("cognito-vended-session-outlives-pool-deletion".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            // ON: whether the credential a read presents is one this emulator still holds is
            // half of what this suite asks.
            router.set_request_authentication(true);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            // The bucket and the one object, written with the key this process publishes.
            let created = s3(
                &router,
                &runtime,
                &SignedWith::Published,
                "PUT",
                &format!("/{BUCKET}"),
                "",
            );
            assert_eq!(
                created.status,
                200,
                "setup: CreateBucket({BUCKET:?}) must succeed, got {} with {:?}",
                created.status,
                created.text()
            );
            let written = s3(
                &router,
                &runtime,
                &SignedWith::Published,
                "PUT",
                &format!("/{BUCKET}/{KEY}"),
                PAYLOAD,
            );
            assert_eq!(
                written.status,
                200,
                "setup: PutObject({KEY:?}) must succeed, got {} with {:?}",
                written.status,
                written.text()
            );

            let rows = [
                ("the identity pool is deleted", Lifecycle::PoolDeleted),
                (
                    "the identity the session was vended for is deleted",
                    Lifecycle::IdentityDeleted,
                ),
            ];

            for (index, (case, lifecycle)) in rows.into_iter().enumerate() {
                // One pool and one role per row, so a row's teardown reaches nothing another
                // row asserts.
                let pool = cognito(
                    &router,
                    &runtime,
                    "CreateIdentityPool",
                    &json!({
                        "IdentityPoolName": format!("xsvc-outlives-{index}"),
                        "AllowUnauthenticatedIdentities": true,
                    }),
                );
                assert_eq!(
                    pool.status,
                    200,
                    "case {case:?}: CreateIdentityPool must succeed, got {} with {:?}",
                    pool.status,
                    pool.text()
                );
                let pool_id = pool.json()["IdentityPoolId"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: a created pool has an id"))
                    .to_string();

                let role_name = format!("xsvc-outlives-reader-{index}");
                let created_role = iam(
                    &router,
                    &runtime,
                    "CreateRole",
                    &[
                        ("RoleName", &role_name),
                        ("AssumeRolePolicyDocument", &pool_trust_policy(&pool_id)),
                    ],
                );
                assert_eq!(
                    created_role.status,
                    200,
                    "case {case:?}: CreateRole({role_name:?}) must succeed, got {} with {:?}",
                    created_role.status,
                    created_role.text()
                );
                let role_arn = element(&created_role.text(), "Arn");
                let granted = iam(
                    &router,
                    &runtime,
                    "PutRolePolicy",
                    &[
                        ("RoleName", &role_name),
                        ("PolicyName", POLICY_NAME),
                        ("PolicyDocument", &only_allow()),
                    ],
                );
                assert_eq!(
                    granted.status,
                    200,
                    "case {case:?}: PutRolePolicy must succeed, got {} with {:?}",
                    granted.status,
                    granted.text()
                );
                let configured = cognito(
                    &router,
                    &runtime,
                    "SetIdentityPoolRoles",
                    &json!({
                        "IdentityPoolId": &pool_id,
                        "Roles": {"unauthenticated": &role_arn, "authenticated": &role_arn},
                    }),
                );
                assert_eq!(
                    configured.status,
                    200,
                    "case {case:?}: SetIdentityPoolRoles must succeed, got {} with {:?}",
                    configured.status,
                    configured.text()
                );

                let (identity_id, session) = vend(&router, &runtime, case, &pool_id);

                // The positive control: the session reads the object while the pool stands.
                read_is_served(
                    &router,
                    &runtime,
                    &format!("{case} / before the lifecycle call"),
                    &session,
                );

                let torn_down = match lifecycle {
                    Lifecycle::PoolDeleted => cognito(
                        &router,
                        &runtime,
                        "DeleteIdentityPool",
                        &json!({"IdentityPoolId": &pool_id}),
                    ),
                    Lifecycle::IdentityDeleted => cognito(
                        &router,
                        &runtime,
                        "DeleteIdentities",
                        &json!({"IdentityIdsToDelete": [&identity_id]}),
                    ),
                };
                assert_eq!(
                    torn_down.status,
                    200,
                    "case {case:?}: the lifecycle call must succeed, got {} with {:?}",
                    torn_down.status,
                    torn_down.text()
                );

                // The reads the rule is about: the same session, the same object, after the
                // teardown — repeated, because resolving a session does not consume it.
                for read in 1..=READS_AFTER {
                    read_is_served(
                        &router,
                        &runtime,
                        &format!("{case} / read {read} of {READS_AFTER} afterwards"),
                        &session,
                    );
                }

                // Where a refusal IS due: the role's only grant removed, which is the one
                // thing that stops this session short of its expiry — and the refusal is
                // Amazon S3's own, naming the principal the session speaks for.
                let revoked = iam(
                    &router,
                    &runtime,
                    "DeleteRolePolicy",
                    &[("RoleName", &role_name), ("PolicyName", POLICY_NAME)],
                );
                assert_eq!(
                    revoked.status,
                    200,
                    "case {case:?}: DeleteRolePolicy must succeed, got {} with {:?}",
                    revoked.status,
                    revoked.text()
                );
                read_is_refused(
                    &router,
                    &runtime,
                    &format!("{case} / after the role's only Allow is gone"),
                    &session,
                    &role_name,
                );
            }
        })
        .expect("spawn the vended-session pool-teardown thread")
        .join()
        .expect("a vended session must outlive the teardown of the pool that vended it");
}
