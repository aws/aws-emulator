// SPDX-License-Identifier: CC0-1.0

//! Group-driven role selection, end to end across the two Cognito services.
//!
//! RULE: a user pool puts the roles its groups grant into the identity token a sign-in
//! issues — `cognito:roles`, best `Precedence` first, and `cognito:preferred_role` for the
//! best-precedence group where its groups name that role unambiguously (`CreateGroup`'s own
//! `Precedence` and `RoleArn` documentation) — and an identity pool whose `RoleMappings`
//! entry for that provider is of `Type` `Token` resolves the vended role from exactly those
//! claims: the preferred role where the token names one, the single granted role otherwise,
//! and, where the claims name SEVERAL roles with no preference among them, whatever the
//! mapping's `AmbiguousRoleResolution` states for that case — the pool's own authenticated
//! role, or a refusal. A `CustomRoleArn` names one of the roles the token carries.
//!
//! Only a cross-service suite can pin this: the groups, the precedences and the sign-in are
//! `cognito-idp`'s, the role mapping and the vend are `cognito-identity`'s, the roles are
//! `iam:CreateRole`'s, and which role a vended session stands for is read from
//! `sts:GetCallerIdentity` — four services over ONE engine, as `core/router/src/lib.rs`
//! mounts them.
//!
//! The rows differ along what the rule generalises over: what the token's claims name (one
//! role, several with a preference, several without one), which group wins on precedence
//! (and the same two groups with the precedences flipped), the resolution the mapping
//! states, and whether the request names a `CustomRoleArn`.
//!
//! Every call goes over the wire. The sign-in and the vend legs carry no credential scope at
//! all (their models list no auth scheme, or make it optional); the setup legs carry one, on
//! a router with request authentication off as every suite in this crate runs it.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread.
//!
//! The wire harness below — the dispatch helpers, the role and caller-identity helpers and
//! the constants they read — is `pub(crate)` because the sibling suite that pins the OTHER
//! role-mapping type (`cognito_rules_role_mapping.rs`) drives the same four services and
//! reads it from here rather than holding a second copy.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
pub(crate) const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The Region every call below is signed for, and so the Region the pools live in.
pub(crate) const REGION: &str = "us-east-1";

/// The emulator's own address: an address, so the `Host` header names no service and the
/// unsigned legs carry their service name in `X-Amz-Target` alone.
pub(crate) const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// The access key id every signed setup call carries.
pub(crate) const SETUP_KEY_ID: &str = "AKIDTEST";

/// The `RoleSessionName` every session an identity pool vends carries, as AWS publishes it
/// in the caller ARN of such a session.
pub(crate) const VENDED_ROLE_SESSION_NAME: &str = "CognitoIdentityCredentials";

/// The STS API version every `awsQuery` call to STS carries.
pub(crate) const STS_API_VERSION: &str = "2011-06-15";

/// The IAM API version every `awsQuery` call to IAM carries.
pub(crate) const IAM_API_VERSION: &str = "2010-05-08";

/// A password satisfying a default user pool's password policy.
pub(crate) const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// What one call answered.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) body: bytes::Bytes,
}

impl Answer {
    pub(crate) fn json(&self) -> Value {
        if self.body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&self.body).unwrap_or(Value::Null)
        }
    }

    pub(crate) fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// Dispatch one request, signed with `access_key_id` for `signing_name`, or unsigned when
/// `signing_name` is `None` — which is how an SDK sends an operation a client calls before
/// it holds any credential.
pub(crate) fn dispatch(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing_name: Option<(&str, &str)>,
    headers: &[(&str, &str)],
    body: bytes::Bytes,
) -> Answer {
    let host = match signing_name {
        Some((name, _)) => format!("{name}.{REGION}.amazonaws.com"),
        None => EMULATOR_HOST.to_string(),
    };
    let mut builder = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", host.clone());
    if let Some((signing_name, access_key_id)) = signing_name {
        // The signature is never checked: this router runs with authentication off. What
        // the scope carries is the key id and the Region a handler reads.
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

/// One awsJson1_1 call to `target`, signed for `signing_name` with [`SETUP_KEY_ID`] or
/// unsigned.
pub(crate) fn rpc(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing_name: Option<&str>,
    target: &str,
    body: &Value,
) -> Answer {
    dispatch(
        router,
        runtime,
        signing_name.map(|name| (name, SETUP_KEY_ID)),
        &[
            ("content-type", "application/x-amz-json-1.1"),
            ("x-amz-target", target),
        ],
        bytes::Bytes::from(body.to_string()),
    )
}

/// One awsJson1_1 call that must be served.
pub(crate) fn ok(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing_name: Option<&str>,
    target: &str,
    body: &Value,
) -> Value {
    let answered = rpc(router, runtime, signing_name, target, body);
    assert_eq!(
        answered.status,
        200,
        "{target} must be served, got {} with {:?}",
        answered.status,
        answered.text()
    );
    answered.json()
}

/// One awsQuery call, which is how IAM and STS are called, signed with `access_key_id`.
pub(crate) fn query(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing_name: &str,
    access_key_id: &str,
    body: String,
) -> Answer {
    dispatch(
        router,
        runtime,
        Some((signing_name, access_key_id)),
        &[("content-type", "application/x-www-form-urlencoded")],
        bytes::Bytes::from(body),
    )
}

/// The text of the first `<name>…</name>` element of an XML body.
pub(crate) fn xml_element(body: &str, name: &str) -> String {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body
        .find(&open)
        .unwrap_or_else(|| panic!("no <{name}> in {body}"))
        + open.len();
    let end = body[start..]
        .find(&close)
        .unwrap_or_else(|| panic!("no </{name}> in {body}"))
        + start;
    body[start..end].to_string()
}

/// A base64url JWS segment's bytes (RFC 7515 §2).
pub(crate) fn decode_base64url(segment: &str) -> Vec<u8> {
    let mut standard = segment.replace('-', "+").replace('_', "/");
    while !standard.len().is_multiple_of(4) {
        standard.push('=');
    }
    aws_smithy_types::base64::decode(&standard)
        .unwrap_or_else(|error| panic!("{segment:?} must be base64url ({error})"))
}

/// The claim set of a compact-serialized JWS.
pub(crate) fn claims_of(token: &str) -> Value {
    let segments: Vec<&str> = token.split('.').collect();
    assert_eq!(
        segments.len(),
        3,
        "a JWS compact serialization has three segments, got {token}"
    );
    serde_json::from_slice(&decode_base64url(segments[1]))
        .unwrap_or_else(|error| panic!("the claims must be JSON ({error}): {token}"))
}

/// A role trust policy admitting the Cognito Identity principal for the identity pool
/// `audience`, written the way the Amazon Cognito Developer Guide writes one.
///
/// Compact and free of the characters that are significant in an
/// `application/x-www-form-urlencoded` body (`&`, `=`, `+`, `%`, space), so it needs no
/// encoding.
pub(crate) fn trust_policy(audience: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Principal\":{{\"Federated\":\"cognito-identity.amazonaws.com\"}},\
         \"Action\":\"sts:AssumeRoleWithWebIdentity\",\"Condition\":{{\"StringEquals\":\
         {{\"cognito-identity.amazonaws.com:aud\":\"{audience}\"}}}}}}]}}"
    )
}

/// `iam:CreateRole` with a trust policy admitting `audience`'s users, answering its ARN.
pub(crate) fn create_role(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    role_name: &str,
    audience: &str,
) -> String {
    let document = trust_policy(audience);
    let created = query(
        router,
        runtime,
        "iam",
        SETUP_KEY_ID,
        format!(
            "Action=CreateRole&Version={IAM_API_VERSION}&RoleName={role_name}\
             &AssumeRolePolicyDocument={document}"
        ),
    );
    assert_eq!(
        created.status,
        200,
        "setup: CreateRole({role_name:?}) must succeed, got {} with {:?}",
        created.status,
        created.text()
    );
    xml_element(&created.text(), "Arn")
}

/// `sts:GetCallerIdentity` under `access_key_id`, answering the caller ARN.
pub(crate) fn caller_arn(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    case: &str,
    access_key_id: &str,
) -> String {
    let answered = query(
        router,
        runtime,
        "sts",
        access_key_id,
        format!("Action=GetCallerIdentity&Version={STS_API_VERSION}"),
    );
    assert_eq!(
        answered.status,
        200,
        "case {case:?}: GetCallerIdentity under the vended credential must answer 200, got \
         {} with {:?}",
        answered.status,
        answered.text()
    );
    xml_element(&answered.text(), "Arn")
}

/// The assumed-role ARN a session of `role_name` speaks for.
pub(crate) fn session_arn(role_name: &str) -> String {
    format!(
        "arn:aws:sts::{}:assumed-role/{role_name}/{VENDED_ROLE_SESSION_NAME}",
        engine::emulator_account_id()
    )
}

/// What a row expects of its vend.
enum Outcome<'a> {
    /// A session of the role of this name is vended.
    Session(&'a str),
    /// The mapping denies the exchange: HTTP 403 `NotAuthorizedException`, nothing vended.
    Denied,
}

/// One row: the case, the `AmbiguousRoleResolution` the mapping states, the precedence each
/// of the user's two groups carries, the `CustomRoleArn` the vend names, the two claims the
/// identity token must carry, and what the vend must come to.
struct Row<'a> {
    case: &'a str,
    resolution: &'a str,
    precedences: (i64, i64),
    custom_role: Option<usize>,
    expected_roles: Vec<usize>,
    expected_preferred: Option<usize>,
    outcome: Outcome<'a>,
}

#[test]
fn group_precedence_and_role_mapping_select_the_vended_role() {
    std::thread::Builder::new()
        .name("cognito-group-role-mapping".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");
            let idp = "AWSCognitoIdentityProviderService";
            let identity = "AWSCognitoIdentityService";

            // ---- the user pool, its app client and the identity pool in front of it ----
            let user_pool_id = ok(
                &router,
                &runtime,
                Some("cognito-idp"),
                &format!("{idp}.CreateUserPool"),
                &json!({"PoolName": "group-role-mapping"}),
            )["UserPool"]["Id"]
                .as_str()
                .expect("CreateUserPool reports the pool's Id")
                .to_string();
            let client_id = ok(
                &router,
                &runtime,
                Some("cognito-idp"),
                &format!("{idp}.CreateUserPoolClient"),
                &json!({
                    "UserPoolId": &user_pool_id,
                    "ClientName": "group-role-mapping-app",
                    "ExplicitAuthFlows": ["ALLOW_USER_PASSWORD_AUTH"],
                }),
            )["UserPoolClient"]["ClientId"]
                .as_str()
                .expect("CreateUserPoolClient reports the client's ClientId")
                .to_string();
            let provider_name = format!("cognito-idp.{REGION}.amazonaws.com/{user_pool_id}");
            let pool_id = ok(
                &router,
                &runtime,
                Some("cognito-identity"),
                &format!("{identity}.CreateIdentityPool"),
                &json!({
                    "IdentityPoolName": "group-role-mapping",
                    "AllowUnauthenticatedIdentities": false,
                    "CognitoIdentityProviders": [
                        {"ProviderName": &provider_name, "ClientId": &client_id},
                    ],
                }),
            )["IdentityPoolId"]
                .as_str()
                .expect("CreateIdentityPool reports an IdentityPoolId")
                .to_string();

            // ---- the roles: one per group, plus the pool's own two ----
            let group_role_names = ["grp-readers-role", "grp-writers-role"];
            let group_roles: Vec<String> = group_role_names
                .iter()
                .map(|name| create_role(&router, &runtime, name, &pool_id))
                .collect();
            let authenticated = create_role(&router, &runtime, "grp-pool-auth", &pool_id);
            let unauthenticated = create_role(&router, &runtime, "grp-pool-unauth", &pool_id);

            // ---- the two groups, and one user in both of them ----
            let group_names = ["readers", "writers"];
            for (group_name, role_arn) in group_names.iter().zip(&group_roles) {
                ok(
                    &router,
                    &runtime,
                    Some("cognito-idp"),
                    &format!("{idp}.CreateGroup"),
                    &json!({
                        "UserPoolId": &user_pool_id,
                        "GroupName": group_name,
                        "RoleArn": role_arn,
                        "Precedence": 5,
                    }),
                );
            }
            ok(
                &router,
                &runtime,
                Some("cognito-idp"),
                &format!("{idp}.AdminCreateUser"),
                &json!({"UserPoolId": &user_pool_id, "Username": "grouped"}),
            );
            ok(
                &router,
                &runtime,
                Some("cognito-idp"),
                &format!("{idp}.AdminSetUserPassword"),
                &json!({
                    "UserPoolId": &user_pool_id,
                    "Username": "grouped",
                    "Password": GOOD_PASSWORD,
                    "Permanent": true,
                }),
            );
            for group_name in group_names {
                ok(
                    &router,
                    &runtime,
                    Some("cognito-idp"),
                    &format!("{idp}.AdminAddUserToGroup"),
                    &json!({
                        "UserPoolId": &user_pool_id,
                        "Username": "grouped",
                        "GroupName": group_name,
                    }),
                );
            }

            // The rows: the two groups' precedences decide the claims, and the mapping's
            // resolution and the request's `CustomRoleArn` decide what the vend makes of
            // them.
            let rows = vec![
                Row {
                    case: "equal precedences, no preference: the pool's authenticated role",
                    resolution: "AuthenticatedRole",
                    precedences: (5, 5),
                    custom_role: None,
                    expected_roles: vec![0, 1],
                    expected_preferred: None,
                    outcome: Outcome::Session("grp-pool-auth"),
                },
                Row {
                    case: "equal precedences, no preference, under Deny: nothing is vended",
                    resolution: "Deny",
                    precedences: (5, 5),
                    custom_role: None,
                    expected_roles: vec![0, 1],
                    expected_preferred: None,
                    outcome: Outcome::Denied,
                },
                Row {
                    case: "equal precedences, a CustomRoleArn the token's roles name",
                    resolution: "Deny",
                    precedences: (5, 5),
                    custom_role: Some(1),
                    expected_roles: vec![0, 1],
                    expected_preferred: None,
                    outcome: Outcome::Session(group_role_names[1]),
                },
                Row {
                    case: "the first group wins on precedence: its role is vended",
                    resolution: "Deny",
                    precedences: (1, 10),
                    custom_role: None,
                    expected_roles: vec![0, 1],
                    expected_preferred: Some(0),
                    outcome: Outcome::Session(group_role_names[0]),
                },
                Row {
                    case: "the precedences flipped: the other group's role is vended",
                    resolution: "Deny",
                    precedences: (10, 1),
                    custom_role: None,
                    expected_roles: vec![1, 0],
                    expected_preferred: Some(1),
                    outcome: Outcome::Session(group_role_names[1]),
                },
            ];

            for row in &rows {
                let case = row.case;
                // The precedences this row asserts, written through the group's own API.
                for (group_name, precedence) in group_names
                    .iter()
                    .zip([row.precedences.0, row.precedences.1])
                {
                    ok(
                        &router,
                        &runtime,
                        Some("cognito-idp"),
                        &format!("{idp}.UpdateGroup"),
                        &json!({
                            "UserPoolId": &user_pool_id,
                            "GroupName": group_name,
                            "Precedence": precedence,
                        }),
                    );
                }
                // The identity pool's role configuration for this row: its two roles, and a
                // `Token` mapping of the user-pool provider stating the row's resolution.
                ok(
                    &router,
                    &runtime,
                    Some("cognito-identity"),
                    &format!("{identity}.SetIdentityPoolRoles"),
                    &json!({
                        "IdentityPoolId": &pool_id,
                        "Roles": {
                            "authenticated": &authenticated,
                            "unauthenticated": &unauthenticated,
                        },
                        "RoleMappings": {
                            &provider_name: {
                                "Type": "Token",
                                "AmbiguousRoleResolution": row.resolution,
                            },
                        },
                    }),
                );

                // ---- the sign-in: an UNSIGNED InitiateAuth, as a client makes it ----
                let signed_in = ok(
                    &router,
                    &runtime,
                    None,
                    &format!("{idp}.InitiateAuth"),
                    &json!({
                        "ClientId": &client_id,
                        "AuthFlow": "USER_PASSWORD_AUTH",
                        "AuthParameters": {
                            "USERNAME": "grouped",
                            "PASSWORD": GOOD_PASSWORD,
                        },
                    }),
                );
                let id_token = signed_in["AuthenticationResult"]["IdToken"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: the sign-in must issue an IdToken: {signed_in}")
                    })
                    .to_string();
                let claims = claims_of(&id_token);

                let expected_roles = Value::Array(
                    row.expected_roles
                        .iter()
                        .map(|index| Value::String(group_roles[*index].clone()))
                        .collect(),
                );
                let actual_roles = claims.get("cognito:roles").cloned();
                assert_eq!(
                    actual_roles,
                    Some(expected_roles.clone()),
                    "case {case:?}: expected cognito:roles {expected_roles}, got \
                     {actual_roles:?}"
                );
                let expected_preferred = row
                    .expected_preferred
                    .map(|index| Value::String(group_roles[index].clone()));
                let actual_preferred = claims.get("cognito:preferred_role").cloned();
                assert_eq!(
                    actual_preferred, expected_preferred,
                    "case {case:?}: expected cognito:preferred_role {expected_preferred:?}, \
                     got {actual_preferred:?}"
                );

                // ---- the exchange: an UNSIGNED GetId, then the vend ----
                let logins = json!({&provider_name: &id_token});
                let identity_id = ok(
                    &router,
                    &runtime,
                    None,
                    &format!("{identity}.GetId"),
                    &json!({"IdentityPoolId": &pool_id, "Logins": &logins}),
                )["IdentityId"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: GetId reports an IdentityId"))
                    .to_string();
                let mut request = json!({"IdentityId": &identity_id, "Logins": &logins});
                if let Some(index) = row.custom_role {
                    request["CustomRoleArn"] = json!(&group_roles[index]);
                }
                let vended = rpc(
                    &router,
                    &runtime,
                    None,
                    &format!("{identity}.GetCredentialsForIdentity"),
                    &request,
                );

                match row.outcome {
                    Outcome::Session(role_name) => {
                        assert_eq!(
                            vended.status,
                            200,
                            "case {case:?}: the vend must be served, got {} with {:?}",
                            vended.status,
                            vended.text()
                        );
                        let body = vended.json();
                        let access_key_id = body["Credentials"]["AccessKeyId"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string();
                        assert!(
                            !access_key_id.is_empty(),
                            "case {case:?}: the vend must carry an AccessKeyId, got {body}"
                        );
                        let arn = caller_arn(&router, &runtime, case, &access_key_id);
                        assert_eq!(
                            arn,
                            session_arn(role_name),
                            "case {case:?}: expected a session of {role_name}, got {arn}"
                        );
                    }
                    Outcome::Denied => {
                        assert_eq!(
                            vended.status,
                            400,
                            "case {case:?}: a denied exchange is an HTTP 400, got {} with \
                             {:?}",
                            vended.status,
                            vended.text()
                        );
                        let body = vended.json();
                        let code = body["__type"].as_str().unwrap_or_default();
                        assert!(
                            code.ends_with("NotAuthorizedException"),
                            "case {case:?}: expected NotAuthorizedException, got {code:?} in \
                             {body}"
                        );
                        assert!(
                            body.get("Credentials").is_none(),
                            "case {case:?}: a denied exchange vends nothing, got {body}"
                        );
                    }
                }
            }
        })
        .expect("spawn the cognito group role-mapping thread")
        .join()
        .expect("group precedence and role mapping must select the vended role");
}
