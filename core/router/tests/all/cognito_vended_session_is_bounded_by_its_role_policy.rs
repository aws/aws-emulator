// SPDX-License-Identifier: CC0-1.0

//! What a Cognito-vended session may DO, across the services a federated caller spans.
//!
//! RULE: the credentials `GetCredentialsForIdentity` vends are an
//! `sts:AssumeRoleWithWebIdentity` session of the pool's mapped role, so every later request
//! signed with them is authorized, per action, against the identity-based policies in force
//! on THAT role: a role carrying no permission policy is granted nothing, and a role carrying
//! a grant is granted exactly the actions its documents name. A request no document allows is
//! refused with the serving service's own access-denied answer, naming the vended principal,
//! the action and IAM's reason clause — the `on resource:` clause naming the resource the
//! request addressed, and absent for an action that addresses none (`s3:ListAllMyBuckets`,
//! the account's own bucket listing).
//!
//! Only a cross-service suite can pin this: the role is written by `iam:CreateRole` and its
//! policy by `iam:PutRolePolicy`, the session is minted by the identity pool's vend, and what
//! the session may do is answered by Amazon S3 and by the Cognito Identity control plane —
//! four services over ONE engine, as `core/router/src/lib.rs` mounts them.
//!
//! The rows differ along what the rule generalises over: which service serves the call, which
//! action it asks for, whether the resource is one the action addresses at all, and whether
//! the role's documents grant that action.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The wire readers and the form encoder this suite needs are the ones this test crate keeps
// once (`all/query_wire.rs`: "Nothing new should write another one — call this module").
use crate::query_wire::{form_encode, opt_element};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The Region every call below is signed for.
const REGION: &str = "us-east-1";

/// The emulator's own address, for the unsigned identity-flow legs: an address, so the `Host`
/// header names no service and the call carries its service in `X-Amz-Target` alone.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// The access key id the signed setup calls carry — a long-term key of the account, which no
/// identity-based policy bounds.
const SETUP_KEY_ID: &str = "AKIDTEST";

/// The IAM API version every `awsQuery` call to IAM carries.
const IAM_API_VERSION: &str = "2010-05-08";

/// The `RoleSessionName` every session an identity pool vends carries.
const VENDED_ROLE_SESSION_NAME: &str = "CognitoIdentityCredentials";

/// The role every vend below assumes.
const ROLE_NAME: &str = "cognito-bounded-session";

/// The IAM action the account's own bucket listing is authorized under: "to grant IAM
/// permission to use this operation, you must add the `s3:ListAllMyBuckets` policy action"
/// (`ListBuckets`'s own documentation). It addresses no resource of its own.
const LIST_ALL_MY_BUCKETS: &str = "s3:ListAllMyBuckets";

/// The IAM action a listing of the account's identity pools is authorized under.
const LIST_IDENTITY_POOLS: &str = "cognito-identity:ListIdentityPools";

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
}

/// Dispatch one request, signed with `access_key_id` for `signing_name`, or unsigned when
/// `signing_name` is `None`.
fn dispatch(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    method: &str,
    path: &str,
    signing_name: Option<(&str, &str)>,
    headers: &[(&str, &str)],
    body: bytes::Bytes,
) -> Answer {
    let host = match signing_name {
        Some((name, _)) => format!("{name}.{REGION}.amazonaws.com"),
        None => EMULATOR_HOST.to_string(),
    };
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host.clone());
    if let Some((signing_name, access_key_id)) = signing_name {
        // The signature is never checked: this router runs with authentication off. What the
        // scope carries is the key id a handler reads the calling principal from.
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

/// One awsJson1_1 call to `target`, signed for `signing_name` with `access_key_id` or
/// unsigned.
fn rpc(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing_name: Option<(&str, &str)>,
    target: &str,
    body: &Value,
) -> Answer {
    dispatch(
        router,
        runtime,
        "POST",
        "/",
        signing_name,
        &[
            ("content-type", "application/x-amz-json-1.1"),
            ("x-amz-target", target),
        ],
        bytes::Bytes::from(body.to_string()),
    )
}

/// One awsQuery call to IAM, signed with the account's own key.
fn iam(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    body: String,
) -> Answer {
    dispatch(
        router,
        runtime,
        "POST",
        "/",
        Some(("iam", SETUP_KEY_ID)),
        &[("content-type", "application/x-www-form-urlencoded")],
        bytes::Bytes::from(body),
    )
}

/// A role trust policy admitting the Cognito Identity principal for the pool `audience`,
/// written the way the Amazon Cognito Developer Guide writes one.
fn trust_policy(audience: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Federated": "cognito-identity.amazonaws.com"},
            "Action": "sts:AssumeRoleWithWebIdentity",
            "Condition": {"StringEquals": {"cognito-identity.amazonaws.com:aud": audience}},
        }],
    })
    .to_string()
}

/// An identity-based document allowing `action` on every resource — the only `Resource`
/// element a grant for an action that addresses no resource can carry.
fn allow(action: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": "Allow", "Action": action, "Resource": "*"}],
    })
    .to_string()
}

/// The sentence a serving service refuses a request no identity-based document allows with,
/// for an action that addresses NO resource of its own: the principal that signed, the action
/// and IAM's clause for an absent grant, and no resource clause, there being no resource the
/// request addressed. Written out here rather than built from any service's own helper.
fn no_grant_for_account_wide(principal: &str, action: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} because no identity-based \
         policy allows the {action} action"
    )
}

/// `GET /?x-id=ListBuckets` — the account's own bucket listing, signed with `access_key_id`.
fn list_buckets(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    access_key_id: &str,
) -> Answer {
    dispatch(
        router,
        runtime,
        "GET",
        "/?x-id=ListBuckets",
        Some(("s3", access_key_id)),
        &[],
        bytes::Bytes::new(),
    )
}

/// `cognito-identity:ListIdentityPools` signed with `access_key_id`.
fn list_identity_pools(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    access_key_id: &str,
) -> Answer {
    rpc(
        router,
        runtime,
        Some(("cognito-identity", access_key_id)),
        "AWSCognitoIdentityService.ListIdentityPools",
        &json!({"MaxResults": 10}),
    )
}

/// The `message` an error body carries: the member an awsJson envelope writes it as, under
/// either casing, and the element a REST-XML error document carries it in — read through this
/// crate's one XML reader.
fn message_of(answer: &Answer) -> String {
    let body = answer.json();
    if let Some(message) = body["message"].as_str().or_else(|| body["Message"].as_str()) {
        return message.to_string();
    }
    let text = answer.text();
    opt_element(&text, "Message").unwrap_or(text)
}

/// The error code an error body carries: `__type` on the awsJson protocols, `<Code>` in a
/// REST-XML error document.
fn code_of(answer: &Answer) -> String {
    let body = answer.json();
    if let Some(code) = body["__type"].as_str() {
        return code.to_string();
    }
    opt_element(&answer.text(), "Code").unwrap_or_default()
}

/// One row of the table: a call made with the vended credential, and what the policies in
/// force on the vended role must answer it with.
struct Row<'a> {
    case: &'a str,
    /// The call itself.
    call: fn(&MultiServiceRouter, &tokio::runtime::Runtime, &str) -> Answer,
    /// The status and error code a refusal carries for the service serving it.
    refusal: (u16, &'a str),
    /// The SENTENCE that refusal states, when the row pins it. `None` for a row whose
    /// service's own wording is pinned by its own suite rather than here.
    message: Option<String>,
    /// The document that grants the call, as an inline policy of the vended role.
    grant: String,
}

#[test]
fn a_vended_session_is_bounded_by_the_policies_in_force_on_its_role() {
    std::thread::Builder::new()
        .name("cognito-bounded-vended-session".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            // A pool that serves the unauthenticated flow, mapped to a role whose trust
            // policy admits its users and which carries NO permission policy at all.
            let created = rpc(
                &router,
                &runtime,
                Some(("cognito-identity", SETUP_KEY_ID)),
                "AWSCognitoIdentityService.CreateIdentityPool",
                &json!({
                    "IdentityPoolName": "bounded-session",
                    "AllowUnauthenticatedIdentities": true,
                }),
            );
            assert_eq!(
                created.status,
                200,
                "setup: CreateIdentityPool must succeed, got {} with {:?}",
                created.status,
                created.text()
            );
            let pool_id = created.json()["IdentityPoolId"]
                .as_str()
                .unwrap_or_else(|| panic!("CreateIdentityPool reports an IdentityPoolId"))
                .to_string();

            let document = form_encode(&trust_policy(&pool_id));
            let role = iam(
                &router,
                &runtime,
                format!(
                    "Action=CreateRole&Version={IAM_API_VERSION}&RoleName={ROLE_NAME}\
                     &AssumeRolePolicyDocument={document}"
                ),
            );
            assert_eq!(
                role.status,
                200,
                "setup: CreateRole must succeed, got {} with {:?}",
                role.status,
                role.text()
            );
            let role_arn = format!(
                "arn:aws:iam::{}:role/{ROLE_NAME}",
                engine::emulator_account_id()
            );
            let configured = rpc(
                &router,
                &runtime,
                Some(("cognito-identity", SETUP_KEY_ID)),
                "AWSCognitoIdentityService.SetIdentityPoolRoles",
                &json!({
                    "IdentityPoolId": &pool_id,
                    "Roles": {"unauthenticated": &role_arn},
                }),
            );
            assert_eq!(
                configured.status,
                200,
                "setup: SetIdentityPoolRoles must succeed, got {} with {:?}",
                configured.status,
                configured.text()
            );

            // The vend itself: the unauthenticated identity flow, which carries no
            // credential at all.
            let minted = rpc(
                &router,
                &runtime,
                None,
                "AWSCognitoIdentityService.GetId",
                &json!({"IdentityPoolId": &pool_id}),
            );
            assert_eq!(
                minted.status,
                200,
                "setup: GetId must be served, got {} with {:?}",
                minted.status,
                minted.text()
            );
            let identity_id = minted.json()["IdentityId"]
                .as_str()
                .unwrap_or_else(|| panic!("GetId reports an IdentityId"))
                .to_string();
            let vended = rpc(
                &router,
                &runtime,
                None,
                "AWSCognitoIdentityService.GetCredentialsForIdentity",
                &json!({"IdentityId": &identity_id}),
            );
            assert_eq!(
                vended.status,
                200,
                "setup: the vend must be served, got {} with {:?}",
                vended.status,
                vended.text()
            );
            let access_key_id = vended.json()["Credentials"]["AccessKeyId"]
                .as_str()
                .unwrap_or_else(|| panic!("the vend reports an AccessKeyId"))
                .to_string();
            let principal = format!(
                "arn:aws:sts::{}:assumed-role/{ROLE_NAME}/{VENDED_ROLE_SESSION_NAME}",
                engine::emulator_account_id()
            );

            // The rows span the services a vended session reaches: what each is refused, and
            // what grants it. The SENTENCE is pinned for the S3 row, the form a refusal of an
            // action that addresses no resource takes; the Cognito Identity row pins the
            // status and the code, its own service's wording being that service's suite to
            // pin.
            let rows = vec![
                Row {
                    case: "s3:ListAllMyBuckets — an action that addresses no resource",
                    call: list_buckets,
                    refusal: (403, "AccessDenied"),
                    message: Some(no_grant_for_account_wide(
                        &principal,
                        LIST_ALL_MY_BUCKETS,
                    )),
                    grant: allow(LIST_ALL_MY_BUCKETS),
                },
                Row {
                    case: "cognito-identity:ListIdentityPools — the account's own pools",
                    call: list_identity_pools,
                    refusal: (403, "AccessDeniedException"),
                    message: None,
                    grant: allow(LIST_IDENTITY_POOLS),
                },
            ];

            // First leg: the role carries no document, so every row is refused.
            for row in &rows {
                let case = row.case;
                let answered = (row.call)(&router, &runtime, &access_key_id);
                let (status, code) = row.refusal;
                assert_eq!(
                    answered.status,
                    status,
                    "case {case:?}: a session of a role with no policy must be refused \
                     {status}, got {} with {:?}",
                    answered.status,
                    answered.text()
                );
                assert_eq!(
                    code_of(&answered),
                    code,
                    "case {case:?}: expected the code {code}, got {:?}",
                    answered.text()
                );
                // Whatever the sentence, it names the VENDED principal: that is what makes
                // the refusal one of this session rather than of the caller that set the pool
                // up.
                let stated = message_of(&answered);
                assert!(
                    stated.contains(&principal),
                    "case {case:?}: the refusal must name the vended principal {principal}, \
                     got {stated:?}"
                );
                if let Some(expected) = &row.message {
                    assert_eq!(
                        &stated, expected,
                        "case {case:?}: expected the refusal {expected:?}, got {stated:?}"
                    );
                }
            }

            // Second leg: the role carries a grant for ONE of the rows, so that row is
            // served and the other stays refused — the decision is per action.
            for granted in &rows {
                let document = form_encode(&granted.grant);
                let written = iam(
                    &router,
                    &runtime,
                    format!(
                        "Action=PutRolePolicy&Version={IAM_API_VERSION}&RoleName={ROLE_NAME}\
                         &PolicyName=granted&PolicyDocument={document}"
                    ),
                );
                assert_eq!(
                    written.status,
                    200,
                    "setup: PutRolePolicy for {:?} must succeed, got {} with {:?}",
                    granted.case,
                    written.status,
                    written.text()
                );
                for row in &rows {
                    let case = row.case;
                    let granted_case = granted.case;
                    let answered = (row.call)(&router, &runtime, &access_key_id);
                    let expected = if std::ptr::eq(row, granted) {
                        200
                    } else {
                        row.refusal.0
                    };
                    assert_eq!(
                        answered.status,
                        expected,
                        "case {case:?} under the grant for {granted_case:?}: expected HTTP \
                         {expected}, got {} with {:?}",
                        answered.status,
                        answered.text()
                    );
                }
            }
        })
        .expect("spawn the bounded-vended-session thread")
        .join()
        .expect("a vended session must be bounded by its role's policies");
}
