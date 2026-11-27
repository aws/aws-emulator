// SPDX-License-Identifier: CC0-1.0

//! The WEB IDENTITY behind a Cognito-vended session, as event history names it.
//!
//! RULE: a session minted by exchanging a web identity token is federated from that token,
//! so every CloudTrail record of a call made with the session carries
//! `userIdentity.sessionContext.webIdFederationData` — `federatedProvider`, the issuer of
//! the token, and `attributes`, the token's claims under the `<provider>:<claim>` keys the
//! exchange published them as (`:aud` the audience the credentials were vended for, `:sub`
//! the identity they were vended to, `:amr` the authentication methods of the session, a
//! multi-valued claim carried as its JSON array). The block describes the CALLER, not the
//! call, so a record of a REFUSED call carries it exactly as a served one does; a session no
//! web-identity exchange minted carries no such member at all, which is what separates the
//! two in event history.
//!
//! For a session an identity pool vends, the provider is Amazon Cognito Identity's own
//! principal, the audience is the identity pool and the subject is the `IdentityId` — so an
//! auditor reading a burst of `AccessDenied` records can say which end user, under which
//! pool, was denied, and whether that session was a guest or an authenticated one.
//!
//! Only a cross-service suite can pin this: the role is written by `iam:CreateRole` and its
//! grant by `iam:PutRolePolicy`, the session is minted by the identity pool's vend (and, for
//! the contrast row, by `sts:AssumeRole`), the call is served and recorded by Amazon S3, and
//! the record is read back through `cloudtrail:LookupEvents` — five services over ONE
//! engine, as `core/router/src/lib.rs` mounts them.
//!
//! The rows differ along what the rule generalises over: which flow minted the session
//! (guest, developer-authenticated, plainly assumed), whose identity it is, and whether the
//! audited call was served or refused.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The wire readers and the form encoder this test crate keeps once (`all/query_wire.rs`).
use crate::query_wire::{element, form_encode, opt_element};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The Region every call below is signed for.
const REGION: &str = "us-east-1";

/// The emulator's own address, for the unsigned identity-flow legs: an address, so the `Host`
/// header names no service and the call carries its service in `X-Amz-Target` alone.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// The access key id the signed setup calls carry — a long-term key of the account.
const SETUP_KEY_ID: &str = "AKIDTEST";

/// The API version every `awsQuery` call to IAM carries.
const IAM_API_VERSION: &str = "2010-05-08";

/// The API version every `awsQuery` call to STS carries.
const STS_API_VERSION: &str = "2011-06-15";

/// The `RoleSessionName` every session an identity pool vends carries.
const VENDED_SESSION_NAME: &str = "CognitoIdentityCredentials";

/// The issuer of every token an identity pool exchanges, which is the `federatedProvider`
/// of a session it vends and the name its claims are keyed by.
const COGNITO_PROVIDER: &str = "cognito-identity.amazonaws.com";

/// The pool's developer provider name, which is what authenticated a
/// developer-authenticated identity and so one of its session's authentication methods.
const DEVELOPER_PROVIDER: &str = "login.fedaudit.example";

/// The user of that developer directory every developer-authenticated leg speaks for.
const DEVELOPER_USER: &str = "fed-audit-user";

/// The IAM action the account's own bucket listing is authorized under: "to grant IAM
/// permission to use this operation, you must add the `s3:ListAllMyBuckets` policy action"
/// (`ListBuckets`'s own documentation).
const LIST_ALL_MY_BUCKETS: &str = "s3:ListAllMyBuckets";

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

/// Dispatch one request, signed with an access key id for `signing_name`, or unsigned when
/// `signing_name` is `None` — which is how an SDK sends an operation whose model lists no
/// auth scheme.
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

/// One awsJson1_1 call to `target`, signed for `signing_name` or unsigned.
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

/// One awsQuery call, which is how IAM and STS are called, signed with the account's own key.
fn query(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing_name: &str,
    body: String,
) -> Answer {
    dispatch(
        router,
        runtime,
        "POST",
        "/",
        Some((signing_name, SETUP_KEY_ID)),
        &[("content-type", "application/x-www-form-urlencoded")],
        bytes::Bytes::from(body),
    )
}

/// One awsQuery call made ANONYMOUSLY, which is how a client holding no credential at all
/// exchanges a web identity token for a session.
fn anonymous_query(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    body: String,
) -> Answer {
    dispatch(
        router,
        runtime,
        "POST",
        "/",
        None,
        &[("content-type", "application/x-www-form-urlencoded")],
        bytes::Bytes::from(body),
    )
}

/// A role trust policy admitting the Cognito Identity principal for the pool `audience`,
/// written the way the Amazon Cognito Developer Guide writes one. `authenticated_only`
/// conditions the assume on the authentication methods too, which is how a pool's
/// authenticated role keeps its guests out.
fn pool_trust_policy(audience: &str, authenticated_only: bool) -> String {
    let mut condition = serde_json::Map::new();
    let mut equals = serde_json::Map::new();
    equals.insert(format!("{COGNITO_PROVIDER}:aud"), json!(audience));
    condition.insert("StringEquals".to_string(), equals.into());
    if authenticated_only {
        let mut like = serde_json::Map::new();
        like.insert(format!("{COGNITO_PROVIDER}:amr"), json!("authenticated"));
        condition.insert("ForAnyValue:StringLike".to_string(), like.into());
    }
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Federated": COGNITO_PROVIDER},
            "Action": "sts:AssumeRoleWithWebIdentity",
            "Condition": condition,
        }],
    })
    .to_string()
}

/// A role trust policy admitting this account's own principals, which is what a PLAIN
/// `sts:AssumeRole` session — the contrast row, federated from no web identity — is minted
/// under.
fn account_trust_policy() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"AWS": format!("arn:aws:iam::{}:root", engine::emulator_account_id())},
            "Action": "sts:AssumeRole",
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

/// `iam:CreateRole` with `document` as its trust policy, answering the role's ARN.
fn create_role(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    role_name: &str,
    document: &str,
) -> String {
    let encoded = form_encode(document);
    let created = query(
        router,
        runtime,
        "iam",
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
    element(&created.text(), "Arn")
}

/// `iam:PutRolePolicy` embedding `document` in `role_name` as its one inline policy.
fn put_role_policy(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    role_name: &str,
    document: &str,
) {
    let encoded = form_encode(document);
    let written = query(
        router,
        runtime,
        "iam",
        format!(
            "Action=PutRolePolicy&Version={IAM_API_VERSION}&RoleName={role_name}\
             &PolicyName=granted&PolicyDocument={encoded}"
        ),
    );
    assert_eq!(
        written.status,
        200,
        "setup: PutRolePolicy({role_name:?}) must succeed, got {} with {:?}",
        written.status,
        written.text()
    );
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

/// The `ListBuckets` record event history holds for the call `access_key_id` signed, read
/// the way a client reads it: `cloudtrail:LookupEvents` filtered on that key id.
fn recorded_list_buckets(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    case: &str,
    access_key_id: &str,
) -> Value {
    let answered = rpc(
        router,
        runtime,
        Some(("cloudtrail", SETUP_KEY_ID)),
        "CloudTrail_20131101.LookupEvents",
        &json!({
            "LookupAttributes": [{
                "AttributeKey": "AccessKeyId",
                "AttributeValue": access_key_id,
            }],
        }),
    );
    assert_eq!(
        answered.status,
        200,
        "case {case:?}: LookupEvents must succeed, got {} with {:?}",
        answered.status,
        answered.text()
    );
    let events = answered.json();
    let documents: Vec<Value> = events["Events"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: LookupEvents reports Events, got {events}"))
        .iter()
        .filter_map(|event| event["CloudTrailEvent"].as_str())
        .filter_map(|document| serde_json::from_str::<Value>(document).ok())
        .filter(|record| record["eventName"] == json!("ListBuckets"))
        .collect();
    assert_eq!(
        documents.len(),
        1,
        "case {case:?}: exactly one ListBuckets was made with this credential, got \
         {documents:?}"
    );
    documents
        .into_iter()
        .next()
        .expect("the one ListBuckets record")
}

/// The `webIdFederationData` a record must carry for a session an identity pool vended for
/// `identity_id` of `identity_pool_id` under `methods`: the Cognito Identity provider and
/// the exchange's own claims, the multi-valued authentication methods as their JSON array.
fn expected_federation(identity_pool_id: &str, identity_id: &str, methods: &[&str]) -> Value {
    let mut attributes = serde_json::Map::new();
    attributes.insert(
        format!("{COGNITO_PROVIDER}:aud"),
        json!(identity_pool_id),
    );
    attributes.insert(format!("{COGNITO_PROVIDER}:sub"), json!(identity_id));
    attributes.insert(
        format!("{COGNITO_PROVIDER}:amr"),
        json!(Value::Array(methods.iter().map(|method| json!(method)).collect()).to_string()),
    );
    json!({"federatedProvider": COGNITO_PROVIDER, "attributes": attributes})
}

/// Which identity flow a session was minted through, which is what its authentication
/// methods — and so its `:amr` attribute — follow from.
#[derive(Clone, Copy, Debug)]
enum Flow {
    /// `GetId` then an unauthenticated vend: a guest of the pool.
    Guest,
    /// `GetOpenIdTokenForDeveloperIdentity` then a vend presenting that token.
    Developer,
}

/// Mint a FRESH session of `identity_id` through `flow`, answering its access key id.
///
/// A developer-authenticated vend presents a token this service minted, and an issuance is
/// spendable once, so each vend of that flow asks for its own token.
fn vend(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    case: &str,
    flow: Flow,
    identity_pool_id: &str,
    identity_id: &str,
) -> String {
    let mut request = json!({"IdentityId": identity_id});
    if matches!(flow, Flow::Developer) {
        let (_, token) = developer_identity(router, runtime, case, identity_pool_id);
        request["Logins"] = json!({COGNITO_PROVIDER: token});
    }
    let vended = rpc(
        router,
        runtime,
        None,
        "AWSCognitoIdentityService.GetCredentialsForIdentity",
        &request,
    );
    assert_eq!(
        vended.status,
        200,
        "case {case:?}: the vend must be served, got {} with {:?}",
        vended.status,
        vended.text()
    );
    let access_key_id = vended.json()["Credentials"]["AccessKeyId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !access_key_id.is_empty(),
        "case {case:?}: the vend must carry an AccessKeyId, got {:?}",
        vended.text()
    );
    access_key_id
}

/// `GetOpenIdTokenForDeveloperIdentity` for the pool's one developer user, answering the
/// identity it names and the token it issued.
fn developer_identity(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    case: &str,
    identity_pool_id: &str,
) -> (String, String) {
    let issued = rpc(
        router,
        runtime,
        Some(("cognito-identity", SETUP_KEY_ID)),
        "AWSCognitoIdentityService.GetOpenIdTokenForDeveloperIdentity",
        &json!({
            "IdentityPoolId": identity_pool_id,
            "Logins": {DEVELOPER_PROVIDER: DEVELOPER_USER},
            // The window the issuance is good for, which the caller of a backend mint states.
            "TokenDuration": 900,
        }),
    );
    assert_eq!(
        issued.status,
        200,
        "case {case:?}: GetOpenIdTokenForDeveloperIdentity must be served, got {} with {:?}",
        issued.status,
        issued.text()
    );
    let body = issued.json();
    let identity_id = body["IdentityId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the issuance reports an IdentityId"))
        .to_string();
    let token = body["Token"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the issuance reports a Token"))
        .to_string();
    (identity_id, token)
}

/// One row: a session, the identity behind it, and the federation its records must name.
struct Row {
    case: &'static str,
    flow: Flow,
    identity_id: String,
    role_name: &'static str,
    federation: Value,
}

#[test]
fn a_vended_sessions_web_identity_federation_is_audited_on_every_record() {
    std::thread::Builder::new()
        .name("cognito-vended-session-federation-audit".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            // One pool serving both flows: the guest flow its
            // `AllowUnauthenticatedIdentities` admits, and the developer-authenticated flow
            // its `DeveloperProviderName` names.
            let created = rpc(
                &router,
                &runtime,
                Some(("cognito-identity", SETUP_KEY_ID)),
                "AWSCognitoIdentityService.CreateIdentityPool",
                &json!({
                    "IdentityPoolName": "federation-audit",
                    "AllowUnauthenticatedIdentities": true,
                    "DeveloperProviderName": DEVELOPER_PROVIDER,
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

            // A role per flow, each admitting this pool's users and carrying NO permission
            // policy yet, so the first call each session makes is refused.
            let guest_role = "fed-audit-guest";
            let authenticated_role = "fed-audit-authenticated";
            let guest_role_arn = create_role(
                &router,
                &runtime,
                guest_role,
                &pool_trust_policy(&pool_id, false),
            );
            let authenticated_role_arn = create_role(
                &router,
                &runtime,
                authenticated_role,
                &pool_trust_policy(&pool_id, true),
            );
            let configured = rpc(
                &router,
                &runtime,
                Some(("cognito-identity", SETUP_KEY_ID)),
                "AWSCognitoIdentityService.SetIdentityPoolRoles",
                &json!({
                    "IdentityPoolId": &pool_id,
                    "Roles": {
                        "unauthenticated": &guest_role_arn,
                        "authenticated": &authenticated_role_arn,
                    },
                }),
            );
            assert_eq!(
                configured.status,
                200,
                "setup: SetIdentityPoolRoles must succeed, got {} with {:?}",
                configured.status,
                configured.text()
            );

            // The two identities: a guest of the pool, and the developer-authenticated one.
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
            let guest_identity = minted.json()["IdentityId"]
                .as_str()
                .unwrap_or_else(|| panic!("GetId reports an IdentityId"))
                .to_string();
            let (developer_identity_id, _) =
                developer_identity(&router, &runtime, "setup", &pool_id);

            let rows = vec![
                Row {
                    case: "a guest session",
                    flow: Flow::Guest,
                    // A vend presenting no login at all is the unauthenticated exchange, so
                    // the one authentication method is that.
                    federation: expected_federation(
                        &pool_id,
                        &guest_identity,
                        &["unauthenticated"],
                    ),
                    identity_id: guest_identity.clone(),
                    role_name: guest_role,
                },
                Row {
                    case: "a developer-authenticated session",
                    flow: Flow::Developer,
                    // An authenticated exchange names `authenticated`, the service that
                    // minted the token it presented, the provider that authenticated the
                    // user and — the developer directory being the user's own — that user,
                    // named `<provider>:<pool>:<user>`: the values the token's own `amr`
                    // claim carries, in the order the exchange publishes them.
                    federation: expected_federation(
                        &pool_id,
                        &developer_identity_id,
                        &[
                            "authenticated",
                            COGNITO_PROVIDER,
                            DEVELOPER_PROVIDER,
                            &format!("{DEVELOPER_PROVIDER}:{pool_id}:{DEVELOPER_USER}"),
                        ],
                    ),
                    identity_id: developer_identity_id.clone(),
                    role_name: authenticated_role,
                },
            ];

            // First leg: the role carries no document, so the call is REFUSED — and the
            // refused record names the federation all the same.
            for row in &rows {
                let case = row.case;
                let access_key_id =
                    vend(&router, &runtime, case, row.flow, &pool_id, &row.identity_id);
                let answered = list_buckets(&router, &runtime, &access_key_id);
                assert_eq!(
                    answered.status,
                    403,
                    "case {case:?}: a session of a role with no policy must be refused, got \
                     {} with {:?}",
                    answered.status,
                    answered.text()
                );
                assert_eq!(
                    opt_element(&answered.text(), "Code").as_deref(),
                    Some("AccessDenied"),
                    "case {case:?}: expected the AccessDenied code, got {:?}",
                    answered.text()
                );
                let record =
                    recorded_list_buckets(&router, &runtime, case, &access_key_id);
                assert_eq!(
                    record.get("errorCode"),
                    Some(&json!("AccessDenied")),
                    "case {case:?}: the refused call's record carries its error code, got \
                     {record}"
                );
                assert_eq!(
                    record.pointer("/userIdentity/sessionContext/webIdFederationData"),
                    Some(&row.federation),
                    "case {case:?}: a refused call's record must carry the federation {}, \
                     got {record}",
                    row.federation
                );
                // The session it names is the one the vend minted, whose own ARN the record
                // carries beside the federation.
                assert_eq!(
                    record.pointer("/userIdentity/arn"),
                    Some(&json!(format!(
                        "arn:aws:sts::{}:assumed-role/{}/{VENDED_SESSION_NAME}",
                        engine::emulator_account_id(),
                        row.role_name
                    ))),
                    "case {case:?}: the record names the vended session, got {record}"
                );
            }

            // Second leg: the grant is written, a FRESH session is vended for the same
            // identity, and the SERVED call's record names the same federation — the block
            // describes the caller, not the outcome.
            for row in &rows {
                let case = row.case;
                put_role_policy(
                    &router,
                    &runtime,
                    row.role_name,
                    &allow(LIST_ALL_MY_BUCKETS),
                );
                let access_key_id =
                    vend(&router, &runtime, case, row.flow, &pool_id, &row.identity_id);
                let answered = list_buckets(&router, &runtime, &access_key_id);
                assert_eq!(
                    answered.status,
                    200,
                    "case {case:?}: the granted call must be served, got {} with {:?}",
                    answered.status,
                    answered.text()
                );
                let record =
                    recorded_list_buckets(&router, &runtime, case, &access_key_id);
                assert_eq!(
                    record.get("errorCode"),
                    None,
                    "case {case:?}: a served call's record carries no error code, got \
                     {record}"
                );
                assert_eq!(
                    record.pointer("/userIdentity/sessionContext/webIdFederationData"),
                    Some(&row.federation),
                    "case {case:?}: a served call's record must carry the federation {}, \
                     got {record}",
                    row.federation
                );
            }

            // The rule is the EXCHANGE's, not the identity pool's: a session minted by
            // calling `sts:AssumeRoleWithWebIdentity` with a token of the same pool is
            // federated from the same web identity, so its records name that federation
            // too — under the session name the exchange asked for rather than the pool's.
            let direct_case = "a direct web-identity exchange";
            let (direct_identity, direct_token) =
                developer_identity(&router, &runtime, direct_case, &pool_id);
            let encoded_token = form_encode(&direct_token);
            let encoded_authenticated_role = form_encode(&authenticated_role_arn);
            let exchanged = anonymous_query(
                &router,
                &runtime,
                format!(
                    "Action=AssumeRoleWithWebIdentity&Version={STS_API_VERSION}\
                     &RoleArn={encoded_authenticated_role}&RoleSessionName=direct-exchange\
                     &WebIdentityToken={encoded_token}"
                ),
            );
            assert_eq!(
                exchanged.status,
                200,
                "case {direct_case:?}: the exchange must be served, got {} with {:?}",
                exchanged.status,
                exchanged.text()
            );
            let exchanged_key_id = element(&exchanged.text(), "AccessKeyId");
            let answered = list_buckets(&router, &runtime, &exchanged_key_id);
            assert_eq!(
                answered.status,
                200,
                "case {direct_case:?}: the granted call must be served, got {} with {:?}",
                answered.status,
                answered.text()
            );
            let record =
                recorded_list_buckets(&router, &runtime, direct_case, &exchanged_key_id);
            let expected = expected_federation(
                &pool_id,
                &direct_identity,
                &[
                    "authenticated",
                    COGNITO_PROVIDER,
                    DEVELOPER_PROVIDER,
                    &format!("{DEVELOPER_PROVIDER}:{pool_id}:{DEVELOPER_USER}"),
                ],
            );
            assert_eq!(
                record.pointer("/userIdentity/sessionContext/webIdFederationData"),
                Some(&expected),
                "case {direct_case:?}: a session the token service federated must carry the \
                 federation {expected}, got {record}"
            );

            // The contrast row: a session minted by a PLAIN `sts:AssumeRole` is federated
            // from no web identity, so its records carry the session block without the
            // member — which is what tells the two apart in event history.
            let plain_role = "fed-audit-plain";
            let plain_role_arn =
                create_role(&router, &runtime, plain_role, &account_trust_policy());
            let encoded_role_arn = form_encode(&plain_role_arn);
            let assumed = query(
                &router,
                &runtime,
                "sts",
                format!(
                    "Action=AssumeRole&Version={STS_API_VERSION}&RoleArn={encoded_role_arn}\
                     &RoleSessionName=plain-session"
                ),
            );
            assert_eq!(
                assumed.status,
                200,
                "setup: AssumeRole must succeed, got {} with {:?}",
                assumed.status,
                assumed.text()
            );
            let plain_key_id = element(&assumed.text(), "AccessKeyId");
            let answered = list_buckets(&router, &runtime, &plain_key_id);
            assert_eq!(
                answered.status,
                403,
                "case a plainly assumed session: a role with no policy must be refused, got \
                 {} with {:?}",
                answered.status,
                answered.text()
            );
            let record = recorded_list_buckets(
                &router,
                &runtime,
                "a plainly assumed session",
                &plain_key_id,
            );
            assert_eq!(
                record.pointer("/userIdentity/sessionContext/sessionIssuer/arn"),
                Some(&json!(plain_role_arn)),
                "case a plainly assumed session: the record names the role it is a session \
                 of, got {record}"
            );
            assert_eq!(
                record.pointer("/userIdentity/sessionContext/webIdFederationData"),
                None,
                "case a plainly assumed session: no web identity federated it, so its record \
                 carries no such member, got {record}"
            );
        })
        .expect("spawn the vended-session federation audit thread")
        .join()
        .expect("a vended session's web identity federation must be audited");
}
