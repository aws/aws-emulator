// SPDX-License-Identifier: CC0-1.0

//! The AUTHORIZATION of an Amazon S3 read made with credentials an Amazon Cognito identity
//! pool vended: the bucket's resource-based policy and the mapped role's identity-based
//! policy, evaluated together.
//!
//! RULE: a request signed with a credential `GetCredentialsForIdentity` vended is authorized
//! as the principal of the role session the pool resolved for that exchange, against the
//! UNION of the documents in force on that role and the addressed bucket's own policy — "the
//! resource-based policy and the identity-based policy are evaluated together, and access is
//! granted if either allows it", while "an explicit deny in any policy overrides any allows"
//! (IAM User Guide, *Policy evaluation logic*). So a grant that lives ONLY in the bucket
//! policy admits the read, an explicit `Deny` in EITHER document refuses it with HTTP 403
//! `AccessDenied`, and a request no document grants falls to IAM's implicit deny — each
//! refusal naming the vended principal
//! (`arn:<partition>:sts::<account>:assumed-role/<mapped-role>/CognitoIdentityCredentials`),
//! the action and the object's ARN, with the clause that says which document decided it.
//!
//! Only a cross-service suite can pin this: the role and its inline policy are written by
//! `iam:CreateRole`/`iam:PutRolePolicy`, the pool and its role mapping by Amazon Cognito
//! Identity, the session is minted by the vend, and the decision is taken by Amazon S3 —
//! four crates over ONE engine, as `core/router/src/lib.rs` mounts them. Each crate's own
//! tests can assert its own half; none of them can make a vended session reach a read whose
//! outcome a bucket policy decides.
//!
//! What the rounds vary is what the rule generalises over: WHICH document carries the grant
//! (the bucket policy alone, the role's own policy alone, neither), WHICH document carries
//! the explicit deny (the bucket's, the role's), the key prefix the deny is scoped to, and
//! WHICH role the session is of — the pool's unauthenticated role for a vend presenting no
//! login and its authenticated role for one that does, which is also what makes the
//! principal each refusal names differ.
//!
//! The second suite reads the same rule along the PRINCIPAL axis: a bucket policy reaches a
//! vended session only where it names the role that exchange resolved, so it varies which
//! role the grant and the explicit deny name — one of the pool's two, a role the pool
//! resolves for neither flow, both — while no role carries a document of its own, and reads
//! each session's principal back through `sts:GetCallerIdentity` first.
//!
//! Every call goes over the wire. The vend legs carry no credential scope at all (their model
//! lists no auth scheme); the setup legs and the read legs carry one, on a router with
//! request authentication off as every suite in this crate runs it.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, elements, form_encode};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The Region every call below is signed for, and so the Region the pool lives in.
const REGION: &str = "us-east-1";

/// The emulator's own address: an address, so the `Host` header names no service and the
/// unsigned vend legs carry their service name in `X-Amz-Target` alone.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// The access key id every signed SETUP call carries. A key no session and no IAM user
/// holds speaks for the account itself, which is what lets the setup write the bucket, the
/// objects and the policies the rounds below are about.
const SETUP_KEY_ID: &str = "AKIDTEST";

/// The account this emulator serves, which every ARN in a refusal carries.
const ACCOUNT: &str = ::engine::EMULATOR_ACCOUNT_ID;

/// The `RoleSessionName` every session an identity pool vends carries, written out here as
/// AWS publishes it in such a session's caller ARN rather than read back from the
/// implementation.
const VENDED_ROLE_SESSION_NAME: &str = "CognitoIdentityCredentials";

/// The bucket both prefixes live in, and the two objects the rounds read.
const BUCKET: &str = "xsvc-cognito-bucket-policy";
const GRANTED_KEY: &str = "granted/report.txt";
const GRANTED_BODY: &str = "the object only the bucket policy grants";
const RESTRICTED_KEY: &str = "restricted/secret.txt";
const RESTRICTED_BODY: &str = "the object behind the denied prefix";

/// The two roles the pool resolves: the unauthenticated role a vend presenting no login
/// gets a session of, and the authenticated role a vend presenting one gets.
const GUEST_ROLE: &str = "xsvc-cognito-guest-reader";
const AUTH_ROLE: &str = "xsvc-cognito-auth-reader";

/// The inline policy name the guest role's document is written under, overwritten per round.
const GUEST_POLICY_NAME: &str = "guest-reader-policy";

/// The code and status S3 refuses an unauthorized read with (Amazon S3 "Error responses").
const ACCESS_DENIED: &str = "AccessDenied";

/// The IAM action a read of an object's current version is authorized under ("to use
/// `GetObject`, you must have the `READ` access to the object", and
/// `s3:GetObject` is the permission its documentation names).
const GET_OBJECT_ACTION: &str = "s3:GetObject";

/// The three reason clauses IAM publishes in the refusal sentence, one per way the decision
/// can go against the caller: an explicit deny in the resource's own policy, an explicit deny
/// in a document in force on the identity, and no grant anywhere.
const RESOURCE_DENY: &str = "with an explicit deny in a resource-based policy";
const IDENTITY_DENY: &str = "with an explicit deny in an identity-based policy";
const NO_GRANT: &str = "because no identity-based policy allows the s3:GetObject action";

/// The identity-provider name a login carrying a token THIS service minted is handed over
/// under: "if the token is for `cognito-identity.amazonaws.com`, it will be passed through to
/// Security Token Service with the appropriate role for the token"
/// (`GetCredentialsForIdentity`'s own documentation).
const COGNITO_IDENTITY_PROVIDER: &str = "cognito-identity.amazonaws.com";

/// The IAM and STS API versions every awsQuery call to them carries.
const IAM_API_VERSION: &str = "2010-05-08";

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

/// How a request is signed: with a SigV4 credential scope naming a service and the access
/// key id it is signed with, or with no credential scope at all — which is how an SDK sends
/// an operation whose model lists no auth scheme.
#[derive(Clone, Copy, Debug)]
enum Signing<'a> {
    As {
        signing_name: &'a str,
        access_key_id: &'a str,
    },
    Anonymous,
}

/// A setup call's signing, for the service whose signing name is `signing_name`.
fn as_setup(signing_name: &str) -> Signing<'_> {
    Signing::As {
        signing_name,
        access_key_id: SETUP_KEY_ID,
    }
}

/// Dispatch one request, addressed and signed as `signing` says.
fn dispatch(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing: Signing<'_>,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: bytes::Bytes,
) -> Answer {
    let host = match signing {
        Signing::As { signing_name, .. } => format!("{signing_name}.{REGION}.amazonaws.com"),
        Signing::Anonymous => EMULATOR_HOST.to_string(),
    };
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host.clone());
    if let Signing::As {
        signing_name,
        access_key_id,
    } = signing
    {
        // The credential scope is how a signed request names the access key id it is signed
        // with and the service it is for. The signature is never checked: this router runs
        // with request authentication off.
        builder = builder.header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential={access_key_id}/20260101/{REGION}/{signing_name}/aws4_request, \
                 SignedHeaders=host, Signature=not-verified"
            ),
        );
    }
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder.body(body).expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        body,
    }
}

/// One awsJson1_1 call, signed as `signing` says.
fn rpc(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing: Signing<'_>,
    target: &str,
    body: &Value,
) -> Answer {
    dispatch(
        router,
        runtime,
        signing,
        "POST",
        "/",
        &[
            ("content-type", "application/x-amz-json-1.1"),
            ("x-amz-target", target),
        ],
        bytes::Bytes::from(body.to_string()),
    )
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
    dispatch(
        router,
        runtime,
        as_setup("iam"),
        "POST",
        "/",
        &[("content-type", "application/x-www-form-urlencoded")],
        bytes::Bytes::from(body),
    )
}

/// One S3 call on the path-style address, signed with `access_key_id`.
fn s3(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    access_key_id: &str,
    method: &str,
    path: &str,
    body: &str,
) -> Answer {
    dispatch(
        router,
        runtime,
        Signing::As {
            signing_name: "s3",
            access_key_id,
        },
        method,
        path,
        &[("content-type", "application/xml")],
        bytes::Bytes::from(body.to_owned()),
    )
}

/// The trust policy of a role an identity pool vends sessions of, written the way the Amazon
/// Cognito Developer Guide writes one: the `Federated` Cognito Identity principal, the
/// web-identity assume, and the `:aud` condition restricting the assume to that one pool's
/// users.
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

/// One statement of a RESOURCE-based policy: a bucket policy states WHO it grants to, so
/// every statement names its principals.
fn resource_statement(effect: &str, principals: &[&str], resource: &str) -> Value {
    json!({
        "Effect": effect,
        "Principal": {"AWS": principals},
        "Action": GET_OBJECT_ACTION,
        "Resource": resource,
    })
}

/// One statement of an IDENTITY-based policy, which names no principal: the identity it is
/// attached to is the principal.
fn identity_statement(effect: &str, resource: &str) -> Value {
    json!({
        "Effect": effect,
        "Action": GET_OBJECT_ACTION,
        "Resource": resource,
    })
}

/// A policy document carrying `statements`.
fn document(statements: Vec<Value>) -> String {
    json!({"Version": "2012-10-17", "Statement": statements}).to_string()
}

/// The ARN of an object of [`BUCKET`], as a policy and a refusal name it.
fn object_arn(key: &str) -> String {
    format!("arn:aws:s3:::{BUCKET}/{key}")
}

/// `iam:CreateRole` for a role the pool `pool_id` vends sessions of, answering its ARN.
fn create_role(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    role_name: &str,
    pool_id: &str,
) -> String {
    let created = iam(
        router,
        runtime,
        "CreateRole",
        &[
            ("RoleName", role_name),
            ("AssumeRolePolicyDocument", &pool_trust_policy(pool_id)),
        ],
    );
    assert_eq!(
        created.status,
        200,
        "setup: CreateRole({role_name:?}) must succeed, got {} with {:?}",
        created.status,
        created.text()
    );
    element(&created.text(), "Arn")
}

/// `iam:PutRolePolicy` — the inline document in force on the guest role for one round.
fn put_role_policy(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    role_name: &str,
    policy: &str,
) {
    let attached = iam(
        router,
        runtime,
        "PutRolePolicy",
        &[
            ("RoleName", role_name),
            ("PolicyName", GUEST_POLICY_NAME),
            ("PolicyDocument", policy),
        ],
    );
    assert_eq!(
        attached.status,
        200,
        "setup: PutRolePolicy({role_name:?}) must succeed, got {} with {:?}",
        attached.status,
        attached.text()
    );
}

/// `cognito-identity:CreateIdentityPool` for a pool that admits the guest flow, answering
/// its id.
fn create_identity_pool(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    pool_name: &str,
) -> String {
    let created = rpc(
        router,
        runtime,
        as_setup("cognito-identity"),
        "AWSCognitoIdentityService.CreateIdentityPool",
        &json!({"IdentityPoolName": pool_name, "AllowUnauthenticatedIdentities": true}),
    );
    assert_eq!(
        created.status,
        200,
        "setup: CreateIdentityPool({pool_name:?}) must succeed, got {} with {:?}",
        created.status,
        created.text()
    );
    created.json()["IdentityPoolId"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "setup: CreateIdentityPool reports an IdentityPoolId, got {}",
                created.text()
            )
        })
        .to_string()
}

/// `cognito-identity:SetIdentityPoolRoles` — the role each flow of the pool resolves.
fn set_identity_pool_roles(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    pool_id: &str,
    authenticated: &str,
    unauthenticated: &str,
) {
    let configured = rpc(
        router,
        runtime,
        as_setup("cognito-identity"),
        "AWSCognitoIdentityService.SetIdentityPoolRoles",
        &json!({
            "IdentityPoolId": pool_id,
            "Roles": {"authenticated": authenticated, "unauthenticated": unauthenticated},
        }),
    );
    assert_eq!(
        configured.status,
        200,
        "setup: SetIdentityPoolRoles must succeed, got {} with {:?}",
        configured.status,
        configured.text()
    );
}

/// Which exchange a vend is, which is what the pool resolves its role from: "Logins should
/// not be specified when trying to get credentials for an unauthenticated identity"
/// (`GetCredentialsForIdentityInput$Logins`), so a vend presenting a login is the
/// AUTHENTICATED exchange and one presenting none is the guest exchange.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Flow {
    Guest,
    Authenticated,
}

/// An unsigned `cognito-identity:GetOpenIdToken`, answering the token an authenticated
/// exchange presents as its login under this service's own provider name.
fn open_id_token(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    identity_id: &str,
) -> String {
    let issued = rpc(
        router,
        runtime,
        Signing::Anonymous,
        "AWSCognitoIdentityService.GetOpenIdToken",
        &json!({"IdentityId": identity_id}),
    );
    assert_eq!(
        issued.status,
        200,
        "setup: an unsigned GetOpenIdToken must be served, got {} with {:?}",
        issued.status,
        issued.text()
    );
    issued.json()["Token"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "setup: GetOpenIdToken reports a Token, got {}",
                issued.text()
            )
        })
        .to_string()
}

/// The access key id of a session vended to a fresh identity of `pool_id` for the exchange
/// `flow` is, reached the way a client reaches it: `GetId`, the `GetOpenIdToken` an
/// authenticated exchange presents as its login, and `GetCredentialsForIdentity` — each sent
/// with NO credential scope, the way a client holding no Amazon Web Services credential
/// sends them.
fn vended_access_key_id(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    pool_id: &str,
    flow: Flow,
) -> String {
    let identity = rpc(
        router,
        runtime,
        Signing::Anonymous,
        "AWSCognitoIdentityService.GetId",
        &json!({"IdentityPoolId": pool_id}),
    );
    assert_eq!(
        identity.status,
        200,
        "case {flow:?}: an unsigned GetId must be served, got {} with {:?}",
        identity.status,
        identity.text()
    );
    let identity_id = identity.json()["IdentityId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {flow:?}: GetId reports an IdentityId"))
        .to_string();

    let mut request = json!({"IdentityId": &identity_id});
    if flow == Flow::Authenticated {
        request["Logins"] = json!({
            COGNITO_IDENTITY_PROVIDER: open_id_token(router, runtime, &identity_id),
        });
    }
    let vended = rpc(
        router,
        runtime,
        Signing::Anonymous,
        "AWSCognitoIdentityService.GetCredentialsForIdentity",
        &request,
    );
    assert_eq!(
        vended.status,
        200,
        "case {flow:?}: an unsigned GetCredentialsForIdentity must be served, got {} with \
         {:?}",
        vended.status,
        vended.text()
    );
    let access_key_id = vended.json()["Credentials"]["AccessKeyId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !access_key_id.is_empty(),
        "case {flow:?}: the vend must carry an AccessKeyId, but the member was absent or \
         empty"
    );
    access_key_id
}

/// One session the rounds read with: what it is, the key id it signs with, and the role it
/// is a session of — which is what every refusal names it by.
struct Session {
    label: &'static str,
    access_key_id: String,
    role_name: &'static str,
}

impl Session {
    /// The principal a refusal names this session by: the assumed-role session ARN AWS
    /// publishes for a session an identity pool vended.
    fn principal(&self) -> String {
        format!(
            "arn:aws:sts::{ACCOUNT}:assumed-role/{}/{VENDED_ROLE_SESSION_NAME}",
            self.role_name
        )
    }
}

/// What one read must be answered with.
#[derive(Clone, Copy, Debug)]
enum Expected {
    /// The object's bytes.
    Served(&'static str),
    /// 403 `AccessDenied`, whose sentence carries this reason clause.
    Denied(&'static str),
}

/// One read of one key by one session, and what the policies in force must make of it.
struct Probe<'a> {
    session: &'a Session,
    key: &'static str,
    expected: Expected,
}

/// One round: the documents in force, and the reads they decide.
struct Round<'a> {
    /// What this round is, named in every assertion it drives.
    case: &'static str,
    /// The bucket's own policy, or `None` for a round that removes it.
    bucket_policy: Option<String>,
    /// The inline document in force on the guest role for this round.
    guest_role_policy: String,
    probes: Vec<Probe<'a>>,
}

/// `PUT /<bucket>?policy` — S3's `PutBucketPolicy`.
fn put_bucket_policy(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    policy: &str,
    case: &str,
) {
    let stored = s3(
        router,
        runtime,
        SETUP_KEY_ID,
        "PUT",
        &format!("/{BUCKET}?policy"),
        policy,
    );
    assert_eq!(
        stored.status,
        204,
        "case {case:?}: PutBucketPolicy must succeed, got {} with {:?}",
        stored.status,
        stored.text()
    );
    // What the bucket carries is read back through the service's own reader, so a round
    // decides against the document S3 actually stored.
    let read_back = s3(
        router,
        runtime,
        SETUP_KEY_ID,
        "GET",
        &format!("/{BUCKET}?policy"),
        "",
    );
    assert_eq!(
        (read_back.status, read_back.text().as_str()),
        (200, policy),
        "case {case:?}: the stored policy must read back, got {} with {:?}",
        read_back.status,
        read_back.text()
    );
}

/// `DELETE /<bucket>?policy` — S3's `DeleteBucketPolicy`.
fn delete_bucket_policy(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    case: &str,
) {
    let removed = s3(
        router,
        runtime,
        SETUP_KEY_ID,
        "DELETE",
        &format!("/{BUCKET}?policy"),
        "",
    );
    assert_eq!(
        removed.status,
        204,
        "case {case:?}: DeleteBucketPolicy must succeed, got {} with {:?}",
        removed.status,
        removed.text()
    );
}

/// Drive one probe and assert what it was answered.
fn check(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    case: &str,
    probe: &Probe<'_>,
) {
    let Probe {
        session,
        key,
        expected,
    } = probe;
    let label = format!("{case} / {} reading {key}", session.label);
    let answer = s3(
        router,
        runtime,
        &session.access_key_id,
        "GET",
        &format!("/{BUCKET}/{key}"),
        "",
    );
    match expected {
        Expected::Served(body) => {
            assert_eq!(
                (answer.status, answer.text().as_str()),
                (200, *body),
                "case {label:?}: the read must be served the object, got {} with {:?}",
                answer.status,
                answer.text()
            );
        }
        Expected::Denied(reason) => {
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
            let expected_message = format!(
                "User: {} is not authorized to perform: {GET_OBJECT_ACTION} on resource: \
                 \"{}\" {reason}",
                session.principal(),
                object_arn(key)
            );
            assert_eq!(
                answer.element("Message").as_deref(),
                Some(expected_message.as_str()),
                "case {label:?}: expected the refusal {expected_message:?}, got {:?}",
                answer.text()
            );
        }
    }
}

#[test]
fn a_bucket_policy_decides_a_cognito_vended_session_read() {
    std::thread::Builder::new()
        .name("cognito-vended-session-bucket-policy".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            // The bucket and the two objects, written with the account's own credential.
            let created = s3(&router, &runtime, SETUP_KEY_ID, "PUT", &format!("/{BUCKET}"), "");
            assert_eq!(
                created.status,
                200,
                "setup: CreateBucket({BUCKET:?}) must succeed, got {} with {:?}",
                created.status,
                created.text()
            );
            for (key, body) in [(GRANTED_KEY, GRANTED_BODY), (RESTRICTED_KEY, RESTRICTED_BODY)] {
                let written = s3(
                    &router,
                    &runtime,
                    SETUP_KEY_ID,
                    "PUT",
                    &format!("/{BUCKET}/{key}"),
                    body,
                );
                assert_eq!(
                    written.status,
                    200,
                    "setup: PutObject({key:?}) must succeed, got {} with {:?}",
                    written.status,
                    written.text()
                );
            }

            // The pool and its two roles: the guest role carries whatever document each
            // round puts on it, and the AUTHENTICATED role carries NONE at all — which is
            // what makes a grant that lives only in the bucket policy observable.
            let pool_id = create_identity_pool(&router, &runtime, "xsvc-bucket-policy-callers");
            let guest_role_arn = create_role(&router, &runtime, GUEST_ROLE, &pool_id);
            let auth_role_arn = create_role(&router, &runtime, AUTH_ROLE, &pool_id);
            set_identity_pool_roles(
                &router,
                &runtime,
                &pool_id,
                &auth_role_arn,
                &guest_role_arn,
            );

            let guest = Session {
                label: "the pool's unauthenticated role session",
                access_key_id: vended_access_key_id(&router, &runtime, &pool_id, Flow::Guest),
                role_name: GUEST_ROLE,
            };
            let authenticated = Session {
                label: "the pool's authenticated role session",
                access_key_id: vended_access_key_id(
                    &router,
                    &runtime,
                    &pool_id,
                    Flow::Authenticated,
                ),
                role_name: AUTH_ROLE,
            };
            assert_ne!(
                guest.access_key_id, authenticated.access_key_id,
                "setup: the two flows must be vended sessions of their own roles"
            );

            let both_roles = [guest_role_arn.as_str(), auth_role_arn.as_str()];
            let whole_bucket = format!("arn:aws:s3:::{BUCKET}/*");
            let restricted_prefix = format!("arn:aws:s3:::{BUCKET}/restricted/*");
            // The guest role's own grant for most rounds: the RESTRICTED prefix only, so
            // what the `granted/` prefix is readable by is the bucket policy alone.
            let guest_grants_restricted =
                document(vec![identity_statement("Allow", &restricted_prefix)]);

            let rounds = vec![
                Round {
                    case: "a bucket allow over the bucket with an explicit deny on the \
                           restricted prefix",
                    bucket_policy: Some(document(vec![
                        resource_statement("Allow", &both_roles, &whole_bucket),
                        resource_statement("Deny", &both_roles, &restricted_prefix),
                    ])),
                    guest_role_policy: guest_grants_restricted.clone(),
                    probes: vec![
                        // The bucket policy is the ONLY grant that covers this key for
                        // either role.
                        Probe {
                            session: &guest,
                            key: GRANTED_KEY,
                            expected: Expected::Served(GRANTED_BODY),
                        },
                        Probe {
                            session: &authenticated,
                            key: GRANTED_KEY,
                            expected: Expected::Served(GRANTED_BODY),
                        },
                        // And the explicit deny wins over both allows that cover this one —
                        // the bucket's own, and the guest role's.
                        Probe {
                            session: &guest,
                            key: RESTRICTED_KEY,
                            expected: Expected::Denied(RESOURCE_DENY),
                        },
                        Probe {
                            session: &authenticated,
                            key: RESTRICTED_KEY,
                            expected: Expected::Denied(RESOURCE_DENY),
                        },
                    ],
                },
                Round {
                    case: "the same bucket allow with the deny removed",
                    bucket_policy: Some(document(vec![resource_statement(
                        "Allow",
                        &both_roles,
                        &whole_bucket,
                    )])),
                    guest_role_policy: guest_grants_restricted.clone(),
                    probes: vec![
                        Probe {
                            session: &guest,
                            key: RESTRICTED_KEY,
                            expected: Expected::Served(RESTRICTED_BODY),
                        },
                        Probe {
                            session: &authenticated,
                            key: RESTRICTED_KEY,
                            expected: Expected::Served(RESTRICTED_BODY),
                        },
                        Probe {
                            session: &guest,
                            key: GRANTED_KEY,
                            expected: Expected::Served(GRANTED_BODY),
                        },
                    ],
                },
                Round {
                    case: "the deny alone, with the bucket allow removed",
                    bucket_policy: Some(document(vec![resource_statement(
                        "Deny",
                        &both_roles,
                        &restricted_prefix,
                    )])),
                    guest_role_policy: guest_grants_restricted.clone(),
                    probes: vec![
                        // Nothing grants the `granted/` prefix any more: the bucket policy
                        // carried that grant and the roles never did.
                        Probe {
                            session: &guest,
                            key: GRANTED_KEY,
                            expected: Expected::Denied(NO_GRANT),
                        },
                        Probe {
                            session: &authenticated,
                            key: GRANTED_KEY,
                            expected: Expected::Denied(NO_GRANT),
                        },
                        Probe {
                            session: &guest,
                            key: RESTRICTED_KEY,
                            expected: Expected::Denied(RESOURCE_DENY),
                        },
                    ],
                },
                Round {
                    case: "no bucket policy at all",
                    bucket_policy: None,
                    guest_role_policy: guest_grants_restricted.clone(),
                    probes: vec![
                        // The guest role's OWN document is now the only grant, and it
                        // covers the restricted prefix alone.
                        Probe {
                            session: &guest,
                            key: RESTRICTED_KEY,
                            expected: Expected::Served(RESTRICTED_BODY),
                        },
                        Probe {
                            session: &guest,
                            key: GRANTED_KEY,
                            expected: Expected::Denied(NO_GRANT),
                        },
                        Probe {
                            session: &authenticated,
                            key: RESTRICTED_KEY,
                            expected: Expected::Denied(NO_GRANT),
                        },
                    ],
                },
                Round {
                    case: "a bucket allow over the bucket with the ROLE's own document \
                           denying the restricted prefix",
                    bucket_policy: Some(document(vec![resource_statement(
                        "Allow",
                        &both_roles,
                        &whole_bucket,
                    )])),
                    guest_role_policy: document(vec![
                        identity_statement("Allow", &whole_bucket),
                        identity_statement("Deny", &restricted_prefix),
                    ]),
                    probes: vec![
                        // The deny the IDENTITY carries refuses the read the bucket policy
                        // allows, and the refusal names that document rather than the
                        // bucket's.
                        Probe {
                            session: &guest,
                            key: RESTRICTED_KEY,
                            expected: Expected::Denied(IDENTITY_DENY),
                        },
                        Probe {
                            session: &guest,
                            key: GRANTED_KEY,
                            expected: Expected::Served(GRANTED_BODY),
                        },
                        // The authenticated role carries no document of its own, so the
                        // bucket's allow still reaches every key for it.
                        Probe {
                            session: &authenticated,
                            key: RESTRICTED_KEY,
                            expected: Expected::Served(RESTRICTED_BODY),
                        },
                    ],
                },
            ];

            for round in &rounds {
                put_role_policy(&router, &runtime, GUEST_ROLE, &round.guest_role_policy);
                match &round.bucket_policy {
                    Some(policy) => put_bucket_policy(&router, &runtime, policy, round.case),
                    None => delete_bucket_policy(&router, &runtime, round.case),
                }
                for probe in &round.probes {
                    check(&router, &runtime, round.case, probe);
                }
            }
        })
        .expect("spawn the vended-session bucket-policy thread")
        .join()
        .expect("a bucket policy must decide a Cognito-vended session's read");
}

/// A role IAM holds that the pool resolves for NEITHER flow. A grant naming it reaches no
/// session this pool vends: a vended session speaks for the role the pool resolved for that
/// exchange and for no other.
const UNMAPPED_ROLE: &str = "xsvc-cognito-unmapped-reader";

/// The API version every awsQuery call to the token service carries, as
/// `sts:GetCallerIdentity`'s own `Version` parameter.
const STS_API_VERSION: &str = "2011-06-15";

/// `sts:GetCallerIdentity` under `access_key_id`: the caller ARN this emulator resolves that
/// credential to, which is the principal a bucket policy's `Principal` member is matched
/// against.
fn caller_arn(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    access_key_id: &str,
) -> String {
    let answered = dispatch(
        router,
        runtime,
        Signing::As {
            signing_name: "sts",
            access_key_id,
        },
        "POST",
        "/",
        &[("content-type", "application/x-www-form-urlencoded")],
        bytes::Bytes::from(format!("Action=GetCallerIdentity&Version={STS_API_VERSION}")),
    );
    assert_eq!(
        answered.status,
        200,
        "setup: GetCallerIdentity under a vended credential must answer 200, got {} with {:?}",
        answered.status,
        answered.text()
    );
    answered.element("Arn").unwrap_or_default()
}

/// One round of the principal-scoped table: the bucket policy in force, and the reads the
/// principals it names decide. No role carries a document of its own in these rounds, so the
/// bucket policy's `Principal` sets are the only thing that can grant — which is what makes
/// WHICH role a vended session speaks for observable.
struct PrincipalRound<'a> {
    /// What this round is, named in every assertion it drives.
    case: &'static str,
    /// The bucket's own policy for this round.
    bucket_policy: String,
    probes: Vec<Probe<'a>>,
}

/// The same rule read along the principal axis: a bucket policy reaches a vended session only
/// where it names the role THAT exchange resolved, so a grant naming one of the pool's roles
/// leaves a session of the other to IAM's implicit deny, a grant naming a role the pool
/// resolves for neither flow reaches neither session, and a `Deny` scoped to one role refuses
/// that role's session alone.
///
/// The rows vary what the rule generalises over: which role the grant names (the
/// unauthenticated one, the authenticated one, a role the pool maps for neither, both), which
/// role an explicit `Deny` names, and which session reads. Before any of them, each session's
/// own principal is read back through `sts:GetCallerIdentity`, because that ARN is what the
/// `Principal` sets below are matched against.
#[test]
fn a_bucket_policy_reaches_only_the_sessions_of_the_roles_it_names() {
    std::thread::Builder::new()
        .name("cognito-vended-session-bucket-policy-principals".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            let created = s3(&router, &runtime, SETUP_KEY_ID, "PUT", &format!("/{BUCKET}"), "");
            assert_eq!(
                created.status,
                200,
                "setup: CreateBucket({BUCKET:?}) must succeed, got {} with {:?}",
                created.status,
                created.text()
            );
            for (key, body) in [(GRANTED_KEY, GRANTED_BODY), (RESTRICTED_KEY, RESTRICTED_BODY)] {
                let written = s3(
                    &router,
                    &runtime,
                    SETUP_KEY_ID,
                    "PUT",
                    &format!("/{BUCKET}/{key}"),
                    body,
                );
                assert_eq!(
                    written.status,
                    200,
                    "setup: PutObject({key:?}) must succeed, got {} with {:?}",
                    written.status,
                    written.text()
                );
            }

            // The pool's two roles and a third the pool names for neither flow. NONE of them
            // carries an inline document in this suite: every grant below lives in the
            // bucket's own policy, so what decides a read is which role the session the vend
            // minted is a session of.
            let pool_id = create_identity_pool(&router, &runtime, "xsvc-bucket-policy-principals");
            let guest_role_arn = create_role(&router, &runtime, GUEST_ROLE, &pool_id);
            let auth_role_arn = create_role(&router, &runtime, AUTH_ROLE, &pool_id);
            let unmapped_role_arn = create_role(&router, &runtime, UNMAPPED_ROLE, &pool_id);
            set_identity_pool_roles(
                &router,
                &runtime,
                &pool_id,
                &auth_role_arn,
                &guest_role_arn,
            );

            let guest = Session {
                label: "the pool's unauthenticated role session",
                access_key_id: vended_access_key_id(&router, &runtime, &pool_id, Flow::Guest),
                role_name: GUEST_ROLE,
            };
            let authenticated = Session {
                label: "the pool's authenticated role session",
                access_key_id: vended_access_key_id(
                    &router,
                    &runtime,
                    &pool_id,
                    Flow::Authenticated,
                ),
                role_name: AUTH_ROLE,
            };
            // The premise the rounds rest on: each vended credential speaks for a session of
            // the role ITS exchange resolved, which is the principal spelling a bucket policy
            // naming that role grants to.
            for session in [&guest, &authenticated] {
                let arn = caller_arn(&router, &runtime, &session.access_key_id);
                assert_eq!(
                    arn,
                    session.principal(),
                    "case {:?}: the vended session must resolve to a session of {}, got {arn:?}",
                    session.label,
                    session.role_name
                );
            }

            let whole_bucket = format!("arn:aws:s3:::{BUCKET}/*");
            let restricted_prefix = format!("arn:aws:s3:::{BUCKET}/restricted/*");
            let guest_only = [guest_role_arn.as_str()];
            let authenticated_only = [auth_role_arn.as_str()];
            let unmapped_only = [unmapped_role_arn.as_str()];
            let both_roles = [guest_role_arn.as_str(), auth_role_arn.as_str()];

            let rounds = vec![
                PrincipalRound {
                    case: "a bucket allow naming the unauthenticated role alone, with its own \
                           deny on the restricted prefix",
                    bucket_policy: document(vec![
                        resource_statement("Allow", &guest_only, &whole_bucket),
                        resource_statement("Deny", &guest_only, &restricted_prefix),
                    ]),
                    probes: vec![
                        Probe {
                            session: &guest,
                            key: GRANTED_KEY,
                            expected: Expected::Served(GRANTED_BODY),
                        },
                        Probe {
                            session: &guest,
                            key: RESTRICTED_KEY,
                            expected: Expected::Denied(RESOURCE_DENY),
                        },
                        // The grant names the other role, so nothing allows this session.
                        Probe {
                            session: &authenticated,
                            key: GRANTED_KEY,
                            expected: Expected::Denied(NO_GRANT),
                        },
                        Probe {
                            session: &authenticated,
                            key: RESTRICTED_KEY,
                            expected: Expected::Denied(NO_GRANT),
                        },
                    ],
                },
                PrincipalRound {
                    case: "a bucket allow naming the authenticated role alone",
                    bucket_policy: document(vec![resource_statement(
                        "Allow",
                        &authenticated_only,
                        &whole_bucket,
                    )]),
                    probes: vec![
                        Probe {
                            session: &authenticated,
                            key: GRANTED_KEY,
                            expected: Expected::Served(GRANTED_BODY),
                        },
                        Probe {
                            session: &authenticated,
                            key: RESTRICTED_KEY,
                            expected: Expected::Served(RESTRICTED_BODY),
                        },
                        Probe {
                            session: &guest,
                            key: GRANTED_KEY,
                            expected: Expected::Denied(NO_GRANT),
                        },
                    ],
                },
                PrincipalRound {
                    case: "a bucket allow naming a role the pool resolves for neither flow",
                    bucket_policy: document(vec![resource_statement(
                        "Allow",
                        &unmapped_only,
                        &whole_bucket,
                    )]),
                    probes: vec![
                        Probe {
                            session: &guest,
                            key: GRANTED_KEY,
                            expected: Expected::Denied(NO_GRANT),
                        },
                        Probe {
                            session: &authenticated,
                            key: GRANTED_KEY,
                            expected: Expected::Denied(NO_GRANT),
                        },
                    ],
                },
                PrincipalRound {
                    case: "a bucket allow naming both roles with the deny scoped to the \
                           unauthenticated one",
                    bucket_policy: document(vec![
                        resource_statement("Allow", &both_roles, &whole_bucket),
                        resource_statement("Deny", &guest_only, &restricted_prefix),
                    ]),
                    probes: vec![
                        Probe {
                            session: &guest,
                            key: RESTRICTED_KEY,
                            expected: Expected::Denied(RESOURCE_DENY),
                        },
                        // The deny names the other role, so the prefix is still the
                        // bucket allow's to grant for this session.
                        Probe {
                            session: &authenticated,
                            key: RESTRICTED_KEY,
                            expected: Expected::Served(RESTRICTED_BODY),
                        },
                        Probe {
                            session: &guest,
                            key: GRANTED_KEY,
                            expected: Expected::Served(GRANTED_BODY),
                        },
                    ],
                },
            ];

            for round in &rounds {
                put_bucket_policy(&router, &runtime, &round.bucket_policy, round.case);
                for probe in &round.probes {
                    check(&router, &runtime, round.case, probe);
                }
            }
        })
        .expect("spawn the vended-session bucket-policy principals thread")
        .join()
        .expect("a bucket policy must reach only the sessions of the roles it names");
}
