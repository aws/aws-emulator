// SPDX-License-Identifier: CC0-1.0

//! What a GUEST session an identity pool vends may DO, across the services it reaches.
//!
//! RULE: the enhanced flow (`cognito-identity:GetCredentialsForIdentity`) makes the assume
//! itself, so it is the flow that passes the session policies the session it answers with
//! carries: the credentials an UNAUTHENTICATED (guest) exchange hands an anonymous caller are
//! scoped to the services an identity pool limits a guest to, so an action outside that scope
//! is refused `AccessDenied` by the service that receives the call — the session policies
//! being read before the resource the request names is resolved, which is why a probe for a
//! resource that does not exist is answered the refusal and not the absence — while an action
//! inside it is left to the unauthenticated role's own documents exactly as before. The basic
//! flow (`cognito-identity:GetOpenIdToken` and then `sts:AssumeRoleWithWebIdentity`) passes
//! no session policy of the pool's, so the same probes under a session minted that way are
//! answered by the serving service's own decision.
//!
//! Only a cross-service suite can pin this: the role and its permission policy are written by
//! IAM, the scoped session is minted by the identity pool's vend and the unscoped one by STS's
//! own exchange, and what each session may do is answered by IAM, Amazon S3 and Amazon
//! DynamoDB — five services over ONE engine, as `core/router/src/lib.rs` mounts them.
//!
//! The rows differ along what the rule generalises over: which service serves the probe,
//! whether its action namespace is inside the guest scope, whether the resource the probe
//! names exists, and whether the probe addresses one resource at all. The role carries a
//! grant of every action on every resource throughout, so the only thing that can refuse a
//! probe is the scope.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The wire readers and the form encoder this suite needs are the ones this test crate keeps
// once (`all/query_wire.rs`: "Nothing new should write another one — call this module").
use crate::query_wire::{element, form_encode, opt_element};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The Region every call below is signed for.
const REGION: &str = "us-east-1";

/// The emulator's own address, for the unsigned legs of both flows: an address, so the `Host`
/// header names no service.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// The access key id the signed setup calls carry — a long-term key of the account, which no
/// identity-based policy bounds.
const SETUP_KEY_ID: &str = "AKIDTEST";

/// IAM's awsQuery API version.
const IAM_API_VERSION: &str = "2010-05-08";

/// STS's awsQuery API version.
const STS_API_VERSION: &str = "2011-06-15";

/// The `RoleSessionName` every session an identity pool vends carries, which is what the
/// vended principal's ARN ends in.
const VENDED_ROLE_SESSION_NAME: &str = "CognitoIdentityCredentials";

/// The `RoleSessionName` the basic flow's own exchange names, which no part of this service
/// chooses — the caller does.
const BASIC_FLOW_SESSION_NAME: &str = "BasicFlowSession";

/// The role both flows below assume: the pool's unauthenticated role.
const ROLE_NAME: &str = "cognito-guest-scope";

/// A role the account HOLDS, for the probe that reads an existing entity.
const PRESENT_ROLE_NAME: &str = "cognito-guest-scope-present";

/// A role name the account holds NO role under, for the probe the issue's own check makes:
/// the one whose answer tells a scoped session from an unscoped one, since the scope is read
/// before the entity is resolved.
const ABSENT_ROLE_NAME: &str = "cognito-guest-scope-absent";

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

/// One awsQuery call — IAM's and STS's wire form — signed for `signing_name` with
/// `access_key_id`, or unsigned.
fn query(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing_name: Option<(&str, &str)>,
    body: String,
) -> Answer {
    dispatch(
        router,
        runtime,
        "POST",
        "/",
        signing_name,
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

/// An identity-based document granting EVERY action on every resource: what the role carries,
/// so a probe that is refused is refused by the session's scope and by nothing else.
fn allow_everything() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": "Allow", "Action": "*", "Resource": "*"}],
    })
    .to_string()
}

/// `iam:CreateRole` with `document` as its trust policy.
fn create_role(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    role_name: &str,
    document: &str,
) {
    let encoded = form_encode(document);
    let created = query(
        router,
        runtime,
        Some(("iam", SETUP_KEY_ID)),
        format!(
            "Action=CreateRole&Version={IAM_API_VERSION}&RoleName={role_name}\
             &AssumeRolePolicyDocument={encoded}"
        ),
    );
    assert_eq!(
        created.status,
        200,
        "setup: CreateRole({role_name:?}) must succeed, got {} with {:?}",
        created.status,
        created.text()
    );
}

/// `iam:GetRole` for `role_name`, signed with `access_key_id`.
fn get_role(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    access_key_id: &str,
    role_name: &str,
) -> Answer {
    query(
        router,
        runtime,
        Some(("iam", access_key_id)),
        format!("Action=GetRole&Version={IAM_API_VERSION}&RoleName={role_name}"),
    )
}

/// `iam:GetRole` for the role name the account holds NO role under.
fn get_absent_role(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    access_key_id: &str,
) -> Answer {
    get_role(router, runtime, access_key_id, ABSENT_ROLE_NAME)
}

/// `iam:GetRole` for a role the account DOES hold.
fn get_present_role(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    access_key_id: &str,
) -> Answer {
    get_role(router, runtime, access_key_id, PRESENT_ROLE_NAME)
}

/// `iam:ListRoles` — an IAM read that addresses no single role.
fn list_roles(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    access_key_id: &str,
) -> Answer {
    query(
        router,
        runtime,
        Some(("iam", access_key_id)),
        format!("Action=ListRoles&Version={IAM_API_VERSION}"),
    )
}

/// `GET /?x-id=ListBuckets` — the account's own bucket listing, authorized under
/// `s3:ListAllMyBuckets`, whose namespace the guest scope names.
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

/// `dynamodb:ListTables`, whose namespace the guest scope names too — a second service that
/// reads the session's scope, so "inside the scope" is pinned on more than one.
fn list_tables(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    access_key_id: &str,
) -> Answer {
    dispatch(
        router,
        runtime,
        "POST",
        "/",
        Some(("dynamodb", access_key_id)),
        &[
            // DynamoDB speaks `awsJson1_0`, unlike the Cognito Identity calls above.
            ("content-type", "application/x-amz-json-1.0"),
            ("x-amz-target", "DynamoDB_20120810.ListTables"),
        ],
        bytes::Bytes::from(json!({}).to_string()),
    )
}

/// The `message` an error body carries: the member an awsJson envelope writes it as, under
/// either casing, and the element an awsQuery or REST-XML error document carries it in.
fn message_of(answer: &Answer) -> String {
    let body = answer.json();
    if let Some(message) = body["message"].as_str().or_else(|| body["Message"].as_str()) {
        return message.to_string();
    }
    let text = answer.text();
    opt_element(&text, "Message").unwrap_or(text)
}

/// The error code an error body carries: `__type` on the awsJson protocols, `<Code>` in an
/// awsQuery or REST-XML error document.
fn code_of(answer: &Answer) -> String {
    let body = answer.json();
    if let Some(code) = body["__type"].as_str() {
        return code.to_string();
    }
    opt_element(&answer.text(), "Code").unwrap_or_default()
}

/// What a probe is answered with.
#[derive(Debug, Clone)]
enum Outcome {
    /// Served: the scope does not withhold the action and the role's documents grant it.
    Served,
    /// Refused with that status and error code, and — where the row pins it — a message the
    /// answer must START with, the pointer a refusal closes with being per-request.
    Refused {
        status: u16,
        code: &'static str,
        message: Option<String>,
    },
}

/// One row: a probe made with each flow's credential, and what each flow's session must be
/// answered with.
struct Probe<'a> {
    case: &'a str,
    call: fn(&MultiServiceRouter, &tokio::runtime::Runtime, &str) -> Answer,
    /// What the SCOPED session — the enhanced flow's guest credential — is answered.
    scoped: Outcome,
    /// What the UNSCOPED session — the basic flow's own exchange — is answered.
    unscoped: Outcome,
}

/// The refusal a session no session policy allows the action is answered with: the vended
/// principal, the action, the resource the request addressed as IAM names it, and the clause
/// naming the document that withheld the grant. Written out here rather than built from any
/// service's own helper.
fn no_session_policy_allows(principal: &str, action: &str, named_as: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {named_as} \
         because no session policy allows the {action} action"
    )
}

/// Assert one probe under one flow's credential.
fn check(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    probe: &Probe<'_>,
    flow: &str,
    access_key_id: &str,
    expected: &Outcome,
) {
    let case = probe.case;
    let answered = (probe.call)(router, runtime, access_key_id);
    match expected {
        Outcome::Served => assert_eq!(
            answered.status,
            200,
            "case {case:?} under the {flow} flow: expected HTTP 200, got {} with {:?}",
            answered.status,
            answered.text()
        ),
        Outcome::Refused {
            status,
            code,
            message,
        } => {
            assert_eq!(
                answered.status,
                *status,
                "case {case:?} under the {flow} flow: expected HTTP {status}, got {} with {:?}",
                answered.status,
                answered.text()
            );
            let answered_code = code_of(&answered);
            assert_eq!(
                &answered_code,
                code,
                "case {case:?} under the {flow} flow: expected the code {code}, got \
                 {answered_code:?} in {:?}",
                answered.text()
            );
            if let Some(expected) = message {
                let stated = message_of(&answered);
                assert!(
                    stated.starts_with(expected),
                    "case {case:?} under the {flow} flow: expected a refusal starting \
                     {expected:?}, got {stated:?}"
                );
            }
        }
    }
}

#[test]
fn a_guest_vend_is_scoped_down_and_the_basic_flows_exchange_is_not() {
    std::thread::Builder::new()
        .name("cognito-guest-session-scope".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            // A pool that serves the unauthenticated flow.
            let created = rpc(
                &router,
                &runtime,
                Some(("cognito-identity", SETUP_KEY_ID)),
                "AWSCognitoIdentityService.CreateIdentityPool",
                &json!({
                    "IdentityPoolName": "guest-scope",
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

            // The unauthenticated role, admitting this pool's users, and a second role for
            // the probe that reads an entity the account holds. Both trust policies are the
            // same one: what the second role is for is existing, not being assumable.
            let document = trust_policy(&pool_id);
            create_role(&router, &runtime, ROLE_NAME, &document);
            create_role(&router, &runtime, PRESENT_ROLE_NAME, &document);

            // The role may do EVERYTHING, so nothing a probe is refused can be the role's
            // own documents withholding it.
            let policy = form_encode(&allow_everything());
            let written = query(
                &router,
                &runtime,
                Some(("iam", SETUP_KEY_ID)),
                format!(
                    "Action=PutRolePolicy&Version={IAM_API_VERSION}&RoleName={ROLE_NAME}\
                     &PolicyName=everything&PolicyDocument={policy}"
                ),
            );
            assert_eq!(
                written.status,
                200,
                "setup: PutRolePolicy must succeed, got {} with {:?}",
                written.status,
                written.text()
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

            // One guest identity, and the two flows' credentials for it.
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

            // The ENHANCED flow: one unsigned call, which assumes the role itself.
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
                "setup: the enhanced-flow vend must be served, got {} with {:?}",
                vended.status,
                vended.text()
            );
            let scoped_key_id = vended.json()["Credentials"]["AccessKeyId"]
                .as_str()
                .unwrap_or_else(|| panic!("the vend reports an AccessKeyId"))
                .to_string();

            // The BASIC flow: a token for the same identity, exchanged by the caller's own
            // `sts:AssumeRoleWithWebIdentity`, which passes no session policy.
            let issued = rpc(
                &router,
                &runtime,
                None,
                "AWSCognitoIdentityService.GetOpenIdToken",
                &json!({"IdentityId": &identity_id}),
            );
            assert_eq!(
                issued.status,
                200,
                "setup: GetOpenIdToken must be served, got {} with {:?}",
                issued.status,
                issued.text()
            );
            let token = issued.json()["Token"]
                .as_str()
                .unwrap_or_else(|| panic!("GetOpenIdToken reports a Token"))
                .to_string();
            let assumed = query(
                &router,
                &runtime,
                None,
                format!(
                    "Action=AssumeRoleWithWebIdentity&Version={STS_API_VERSION}\
                     &RoleArn={role_arn}&RoleSessionName={BASIC_FLOW_SESSION_NAME}\
                     &WebIdentityToken={token}"
                ),
            );
            assert_eq!(
                assumed.status,
                200,
                "setup: the basic-flow exchange must be served, got {} with {:?}",
                assumed.status,
                assumed.text()
            );
            let unscoped_key_id = element(&assumed.text(), "AccessKeyId");
            assert!(
                !unscoped_key_id.is_empty(),
                "setup: the basic-flow exchange must report an AccessKeyId, got {:?}",
                assumed.text()
            );

            let scoped_principal = format!(
                "arn:aws:sts::{}:assumed-role/{ROLE_NAME}/{VENDED_ROLE_SESSION_NAME}",
                engine::emulator_account_id()
            );

            // The rows: two IAM probes whose namespace the guest scope does NOT name — one
            // for an absent role, one for a role the account holds, so the refusal is seen to
            // precede the lookup — an IAM probe addressing no single role, and two probes
            // whose namespaces the scope DOES name, served by two further services.
            let rows = vec![
                Probe {
                    case: "iam:GetRole for a role name nothing holds",
                    call: get_absent_role,
                    scoped: Outcome::Refused {
                        status: 403,
                        code: "AccessDenied",
                        message: Some(no_session_policy_allows(
                            &scoped_principal,
                            "iam:GetRole",
                            &format!("role {ABSENT_ROLE_NAME}"),
                        )),
                    },
                    unscoped: Outcome::Refused {
                        status: 404,
                        code: "NoSuchEntity",
                        message: Some(format!(
                            "The role with name {ABSENT_ROLE_NAME} cannot be found."
                        )),
                    },
                },
                Probe {
                    case: "iam:GetRole for a role the account holds",
                    call: get_present_role,
                    scoped: Outcome::Refused {
                        status: 403,
                        code: "AccessDenied",
                        message: Some(no_session_policy_allows(
                            &scoped_principal,
                            "iam:GetRole",
                            &format!("role {PRESENT_ROLE_NAME}"),
                        )),
                    },
                    unscoped: Outcome::Served,
                },
                Probe {
                    case: "iam:ListRoles — an IAM read addressing no single role",
                    call: list_roles,
                    scoped: Outcome::Refused {
                        status: 403,
                        code: "AccessDenied",
                        message: None,
                    },
                    unscoped: Outcome::Served,
                },
                Probe {
                    case: "s3:ListAllMyBuckets — a namespace the guest scope names",
                    call: list_buckets,
                    scoped: Outcome::Served,
                    unscoped: Outcome::Served,
                },
                Probe {
                    case: "dynamodb:ListTables — a namespace the guest scope names",
                    call: list_tables,
                    scoped: Outcome::Served,
                    unscoped: Outcome::Served,
                },
            ];

            for probe in &rows {
                check(
                    &router,
                    &runtime,
                    probe,
                    "enhanced",
                    &scoped_key_id,
                    &probe.scoped,
                );
                check(
                    &router,
                    &runtime,
                    probe,
                    "basic",
                    &unscoped_key_id,
                    &probe.unscoped,
                );
            }
        })
        .expect("spawn the guest-session-scope thread")
        .join()
        .expect("a guest session must be scoped down and a basic-flow session must not");
}
