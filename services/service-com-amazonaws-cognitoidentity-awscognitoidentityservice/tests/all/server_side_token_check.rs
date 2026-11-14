// SPDX-License-Identifier: CC0-1.0

//! Server-side validation of a user-pool login, over the same `MultiServiceRouter` the
//! production router builds, with the Amazon Cognito user pools service mounted on the SAME
//! engine — so the user a presented id token stands for is the one `cognito-idp` holds.
//!
//! RULE: an identity pool whose `CognitoIdentityProviders` entry for a presented login's
//! issuer sets `ServerSideTokenCheck` to TRUE revalidates that login with the integrated
//! user pool on every call that is handed one, and refuses it with the modelled
//! `NotAuthorizedException` ("Token is inactive") whenever that pool reports the token's
//! user signed out, deactivated or deleted — "Once you set `ServerSideTokenCheck` to TRUE
//! for an identity pool, that identity pool will check with the integrated user pools to
//! make sure that the user has not been globally signed out or deleted before the identity
//! pool provides an OIDC token or Amazon Web Services credentials for the user. If the user
//! is signed out or deleted, the identity pool will return a 400 Not Authorized error"
//! (`smithy_query(query="doc",shape="CognitoIdentityProvider",member="ServerSideTokenCheck")`).
//! An entry that does not ask for the check is decided by its issuer alone and keeps
//! vending for the very same token, which is what makes the flag the only variable.
//!
//! The served half of that rule is here too: the check decides WHETHER a login is honoured
//! and never which role a honoured one runs as, so a token whose user the integrated pool
//! still holds signed in federates normally — the credentials vended are a session of the
//! role the identity pool's own configuration resolves, which `sts:GetCallerIdentity` under
//! them reports as
//! `arn:<partition>:sts::<account>:assumed-role/<role>/CognitoIdentityCredentials`.
//!
//! The rows differ along what the rule generalises over: WHICH user-pool call takes the
//! user's sign-in away (a deactivation, a deactivation the caller has since reversed, a
//! global sign-out, a delete, a deactivation followed by a delete), and — across the two
//! suites — which operation of this service the login is presented to.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::identity_pool_flow::{cognito_trust_policy, error_code, seed_role, seeded_role_arn};
use crate::vended_role_session::{element, expected_session_arn};
use service_com_amazonaws_cognitoidentity_awscognitoidentityservice::state::base64_url;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615 as sts;

/// The Region every call below is signed for, and the one the user pool's issuer names.
const REGION: &str = "us-east-1";

/// The access-key id every setup call carries; nothing here is really signed.
const ACCOUNT_KEY: &str = "EMULATORTESTKEY12345";

/// A password satisfying Amazon Cognito's default policy (8 characters with an uppercase
/// letter, a lowercase letter, a number and a symbol).
const PASSWORD: &str = "Str0ng!pass";

/// The wire error code of every refusal this suite expects.
const NOT_AUTHORIZED: &str = "NotAuthorizedException";

/// The HTTP status this service answers a modelled client error with (`awsJson1_1`
/// discriminates the failure by the error type in the body), and the status the
/// `ServerSideTokenCheck` documentation states for this refusal ("the identity pool will
/// return a 400 Not Authorized error").
const REFUSED_STATUS: u16 = 400;

/// The sentence the refusal carries, spelled out here so the wire-visible text is pinned by
/// the test rather than read back out of the crate. Fixed text: nothing in it is taken from
/// the request or from stored state.
const INACTIVE_MESSAGE: &str = "Token is inactive";

/// This service beside the Amazon Cognito user pools service on ONE shared engine: the id
/// token a login presents is minted by `cognito-idp`, and the user it stands for is that
/// service's own record.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_cognitoidentity_awscognitoidentityservice::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice
                ::EmulatedService::new(Arc::clone(&engine)),
        );
        // The token service too: what the credentials a vend hands back STAND FOR is the
        // principal the service that holds the session reports for them, not a second copy
        // of that registry kept by this suite.
        router.register_service(sts::EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// One `awsJson1_1` call, with its status and parsed body.
    async fn call(&self, host: &str, target: &str, body: &Value) -> (u16, Value) {
        let signing_name = host.split('.').next().unwrap_or_default();
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{host}/"))
            .header("host", host)
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", target)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/\
                     {signing_name}/aws4_request, SignedHeaders=host;x-amz-target, Signature=0"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let payload = response.into_body();
        let parsed = if payload.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&payload).unwrap_or_else(|error| {
                panic!(
                    "{target} answered a body that is not JSON ({error}): {}",
                    String::from_utf8_lossy(&payload)
                )
            })
        };
        (status, parsed)
    }

    /// `sts:GetCallerIdentity` under `access_key_id`: an awsQuery POST, as a client holding
    /// the vended credential makes it, carrying the key in the SigV4 credential scope.
    /// Answers the status and the response body as text.
    async fn caller_identity(&self, access_key_id: &str) -> (u16, String) {
        let request = http::Request::builder()
            .method("POST")
            .uri("https://sts.amazonaws.com/")
            .header("host", "sts.amazonaws.com")
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential={access_key_id}/20260101/{REGION}/sts/aws4_request, \
                     SignedHeaders=host, Signature=0"
                ),
            )
            .body(bytes::Bytes::from(
                "Action=GetCallerIdentity&Version=2011-06-15",
            ))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        (
            status,
            String::from_utf8_lossy(&response.into_body()).to_string(),
        )
    }

    /// One call to THIS service (cognito-identity).
    async fn identity(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.call(
            &format!("cognito-identity.{REGION}.amazonaws.com"),
            &format!("AWSCognitoIdentityService.{operation}"),
            body,
        )
        .await
    }

    /// One call to this service that must succeed.
    async fn identity_ok(&self, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.identity(operation, body).await;
        assert_eq!(
            status, 200,
            "setup {operation} on {body} expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }

    /// One call to the user pools service (cognito-idp) that must succeed.
    async fn user_pools_ok(&self, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self
            .call(
                &format!("cognito-idp.{REGION}.amazonaws.com"),
                &format!("AWSCognitoIdentityProviderService.{operation}"),
                body,
            )
            .await;
        assert_eq!(
            status, 200,
            "setup cognito-idp {operation} on {body} expected HTTP 200, got {status} with \
             {parsed}"
        );
        parsed
    }

    /// An identity pool fronted by the user pool `provider_name`, with the app client
    /// `client_id`, whose `CognitoIdentityProviders` entry carries
    /// `server_side_token_check` (left out when `None`); its authenticated role is seeded
    /// into the IAM store of the same engine and configured on the pool, so the pool is in
    /// the shape it has to be in to vend at all. Answers its `IdentityPoolId`.
    async fn identity_pool(
        &self,
        name: &str,
        provider_name: &str,
        client_id: &str,
        server_side_token_check: Option<bool>,
    ) -> String {
        let mut provider = json!({"ProviderName": provider_name, "ClientId": client_id});
        if let (Some(check), Some(members)) =
            (server_side_token_check, provider.as_object_mut())
        {
            members.insert("ServerSideTokenCheck".to_string(), json!(check));
        }
        let created = self
            .identity_ok(
                "CreateIdentityPool",
                &json!({
                    "IdentityPoolName": name,
                    "AllowUnauthenticatedIdentities": false,
                    "CognitoIdentityProviders": [provider],
                }),
            )
            .await;
        let pool_id = created["IdentityPoolId"]
            .as_str()
            .unwrap_or_else(|| panic!("CreateIdentityPool answers an IdentityPoolId: {created}"))
            .to_string();
        let role_arn = seed_role(
            &self.engine,
            &seeded_role_arn(name, "auth"),
            cognito_trust_policy(&pool_id),
        );
        self.identity_ok(
            "SetIdentityPoolRoles",
            &json!({
                "IdentityPoolId": &pool_id,
                "Roles": {"authenticated": role_arn},
            }),
        )
        .await;
        pool_id
    }
}

/// A user pool with one app client, as the fixture of every row: the provider name an
/// identity pool fronts it by, and the app client its tokens are issued for.
struct UserPool {
    provider_name: String,
    user_pool_id: String,
    client_id: String,
}

/// Create a user pool and an app client that serves the administrative password sign-in.
async fn user_pool(emulator: &Emulator, name: &str) -> UserPool {
    let created = emulator
        .user_pools_ok("CreateUserPool", &json!({"PoolName": name}))
        .await;
    let user_pool_id = created["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateUserPool answers the pool's Id: {created}"))
        .to_string();
    let client = emulator
        .user_pools_ok(
            "CreateUserPoolClient",
            &json!({
                "UserPoolId": &user_pool_id,
                "ClientName": format!("{name}-client"),
                // The flow the sign-in below uses, plus the refresh flow AWS requires
                // beside any other `ALLOW_` flow.
                "ExplicitAuthFlows": ["ALLOW_ADMIN_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"],
            }),
        )
        .await;
    let client_id = client["UserPoolClient"]["ClientId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateUserPoolClient answers a ClientId: {client}"))
        .to_string();
    UserPool {
        // "Amazon Cognito user pool: `cognito-idp.<region>.amazonaws.com/<YOUR_USER_POOL_ID>`"
        // (`GetId.Logins`, this service's model).
        provider_name: format!("cognito-idp.{REGION}.amazonaws.com/{user_pool_id}"),
        user_pool_id,
        client_id,
    }
}

/// One confirmed user of `pool`, signed in: answers the ID TOKEN of that sign-in, the login
/// a client then presents to an identity pool.
async fn signed_in_user(emulator: &Emulator, pool: &UserPool, username: &str) -> String {
    emulator
        .user_pools_ok(
            "AdminCreateUser",
            &json!({"UserPoolId": &pool.user_pool_id, "Username": username}),
        )
        .await;
    emulator
        .user_pools_ok(
            "AdminSetUserPassword",
            &json!({
                "UserPoolId": &pool.user_pool_id,
                "Username": username,
                "Password": PASSWORD,
                "Permanent": true,
            }),
        )
        .await;
    let signed_in = emulator
        .user_pools_ok(
            "AdminInitiateAuth",
            &json!({
                "UserPoolId": &pool.user_pool_id,
                "ClientId": &pool.client_id,
                "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
                "AuthParameters": {"USERNAME": username, "PASSWORD": PASSWORD},
            }),
        )
        .await;
    signed_in["AuthenticationResult"]["IdToken"]
        .as_str()
        .unwrap_or_else(|| panic!("AdminInitiateAuth answers an IdToken: {signed_in}"))
        .to_string()
}

/// The `Logins` map presenting `id_token` under the user pool's provider name.
fn logins(pool: &UserPool, id_token: &str) -> Value {
    json!({&pool.provider_name: id_token})
}

/// `id_token` with its `sub` claim replaced by `subject`, every other claim and its
/// signature segment kept — so the only difference from the minted token is WHICH user it
/// names ("the principal that is the subject of the JWT", RFC 7519 §4.1.2).
fn with_subject(id_token: &str, subject: &str) -> String {
    let segments: Vec<&str> = id_token.split('.').collect();
    assert_eq!(
        segments.len(),
        3,
        "a minted id token is a compact JWS of three segments, got {}",
        segments.len()
    );
    let mut padded = segments[1].replace('-', "+").replace('_', "/");
    while !padded.len().is_multiple_of(4) {
        padded.push('=');
    }
    let decoded = aws_smithy_types::base64::decode(&padded)
        .unwrap_or_else(|error| panic!("a minted token's claims are base64url ({error})"));
    let mut claims: Value = serde_json::from_slice(&decoded)
        .unwrap_or_else(|error| panic!("a minted token's claims are JSON ({error})"));
    claims
        .as_object_mut()
        .expect("a claim set is an object")
        .insert("sub".to_string(), json!(subject));
    format!(
        "{}.{}.{}",
        segments[0],
        base64_url(claims.to_string().as_bytes()),
        segments[2]
    )
}

/// Assert that a vend was SERVED: a complete credential set for `identity_id`.
fn assert_vended(case: &str, pool_kind: &str, identity_id: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 200,
        "case {case:?}: the {pool_kind} pool must still vend for this token, got {status} with \
         {body}"
    );
    assert_eq!(
        body["IdentityId"].as_str(),
        Some(identity_id),
        "case {case:?}: the {pool_kind} pool answers the identity the vend was made for, got \
         {body}"
    );
    for member in ["AccessKeyId", "SecretKey", "SessionToken", "Expiration"] {
        assert!(
            !body["Credentials"][member].is_null(),
            "case {case:?}: a served vend carries Credentials.{member}, got {body}"
        );
    }
}

/// Assert that a call was refused as the inactive token it presented, with no credential in
/// the answer.
fn assert_refused_inactive(case: &str, what: &str, status: u16, body: &Value) {
    assert_eq!(
        status, REFUSED_STATUS,
        "case {case:?}: {what} must be refused HTTP {REFUSED_STATUS}, got {status} with {body}"
    );
    let code = error_code(body);
    assert!(
        code.ends_with(NOT_AUTHORIZED),
        "case {case:?}: {what} must be refused {NOT_AUTHORIZED}, got {code:?} in {body}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert_eq!(
        message, INACTIVE_MESSAGE,
        "case {case:?}: {what} must carry the message {INACTIVE_MESSAGE:?}, got {message:?} in \
         {body}"
    );
    assert!(
        body.get("Credentials").is_none(),
        "case {case:?}: a refused call carries no credential, got {body}"
    );
}

/// What a row does to the user between the vend that is served and the one that is refused.
/// Each entry is one `cognito-idp` call, with the member set it carries beside the pool and
/// the user.
type UserPoolCall = (&'static str, Value);

/// The calls that take a user's sign-in away, as the user pools model words each of them:
/// `AdminDisableUser` "deactivates a user profile and revokes all access tokens for the
/// user", `AdminUserGlobalSignOut` "signs out a user from all devices", and
/// `AdminDeleteUser` "deletes a user profile in your user pool". `AdminEnableUser`
/// "activates sign-in for a user profile" and is in a row to show that reactivating the
/// profile does not resurrect a token the deactivation revoked.
fn revoking_rows() -> Vec<(&'static str, Vec<UserPoolCall>, bool)> {
    vec![
        (
            "nothing done to the user: the control row",
            Vec::new(),
            false,
        ),
        (
            "the user deactivated",
            vec![("AdminDisableUser", json!({}))],
            true,
        ),
        (
            "the user deactivated and then reactivated",
            vec![
                ("AdminDisableUser", json!({})),
                ("AdminEnableUser", json!({})),
            ],
            true,
        ),
        (
            "the user signed out of every device",
            vec![("AdminUserGlobalSignOut", json!({}))],
            true,
        ),
        ("the user deleted", vec![("AdminDeleteUser", json!({}))], true),
        (
            "the user deactivated and then deleted",
            vec![
                ("AdminDisableUser", json!({})),
                ("AdminDeleteUser", json!({})),
            ],
            true,
        ),
    ]
}

/// RULE: the `ServerSideTokenCheck` entry is revalidated with the user pool at every vend,
/// so a token whose user that pool has signed out, deactivated or deleted buys no
/// credential — while the twin pools that ask for no check keep vending for the very same
/// token.
#[tokio::test]
async fn a_checked_pool_stops_vending_for_a_token_whose_user_the_user_pool_dropped() {
    let emulator = Emulator::new();
    let pool = user_pool(&emulator, "sstc-pool").await;

    // Three identity pools fronted by the one user pool, differing only in what the
    // provider entry says about server-side token validation: the member is optional, so a
    // pool that stored no value for it is a case of its own.
    let checked = emulator
        .identity_pool("sstc-checked", &pool.provider_name, &pool.client_id, Some(true))
        .await;
    let unchecked = emulator
        .identity_pool(
            "sstc-unchecked",
            &pool.provider_name,
            &pool.client_id,
            Some(false),
        )
        .await;
    let unset = emulator
        .identity_pool("sstc-unset", &pool.provider_name, &pool.client_id, None)
        .await;

    for (index, (case, calls, refused)) in revoking_rows().into_iter().enumerate() {
        // One user and one sign-in per row, so what a row does to its user cannot reach
        // another row's token.
        let username = format!("vend-user-{index}");
        let id_token = signed_in_user(&emulator, &pool, &username).await;
        let presented = logins(&pool, &id_token);

        // Every pool vends for the token BEFORE the row's calls, so the only thing that
        // differs between the two halves of a row is what happened to the user.
        let mut identities: Vec<(&str, String, String)> = Vec::new();
        for (pool_kind, identity_pool_id) in [
            ("checked", &checked),
            ("unchecked", &unchecked),
            ("unset", &unset),
        ] {
            let got = emulator
                .identity_ok(
                    "GetId",
                    &json!({"IdentityPoolId": identity_pool_id, "Logins": &presented}),
                )
                .await;
            let identity_id = got["IdentityId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: GetId answers an IdentityId: {got}"))
                .to_string();
            let (status, body) = emulator
                .identity(
                    "GetCredentialsForIdentity",
                    &json!({"IdentityId": &identity_id, "Logins": &presented}),
                )
                .await;
            assert_vended(case, pool_kind, &identity_id, status, &body);
            identities.push((pool_kind, identity_pool_id.clone(), identity_id));
        }

        for (operation, members) in &calls {
            let mut request = json!({"UserPoolId": &pool.user_pool_id, "Username": &username});
            if let (Some(target), Some(extra)) = (request.as_object_mut(), members.as_object()) {
                for (member, value) in extra {
                    target.insert(member.clone(), value.clone());
                }
            }
            emulator.user_pools_ok(operation, &request).await;
        }

        for (pool_kind, _, identity_id) in &identities {
            let (status, body) = emulator
                .identity(
                    "GetCredentialsForIdentity",
                    &json!({"IdentityId": identity_id, "Logins": &presented}),
                )
                .await;
            if refused && *pool_kind == "checked" {
                assert_refused_inactive(case, "the checked pool's vend", status, &body);
            } else {
                assert_vended(case, pool_kind, identity_id, status, &body);
            }
        }
    }
}

/// RULE, at the other calls handed the same login: the check is part of VALIDATING a
/// user-pool login, so `GetId` and `GetOpenIdToken` answer the inactive token the same way,
/// and the pools that ask for no check keep serving it.
#[tokio::test]
async fn a_checked_pool_refuses_an_inactive_token_wherever_the_login_is_validated() {
    let emulator = Emulator::new();
    let pool = user_pool(&emulator, "sstc-operations").await;
    let checked = emulator
        .identity_pool(
            "sstc-ops-checked",
            &pool.provider_name,
            &pool.client_id,
            Some(true),
        )
        .await;
    let unchecked = emulator
        .identity_pool(
            "sstc-ops-unchecked",
            &pool.provider_name,
            &pool.client_id,
            Some(false),
        )
        .await;

    for (index, (case, calls, refused)) in revoking_rows().into_iter().enumerate() {
        let username = format!("token-user-{index}");
        let id_token = signed_in_user(&emulator, &pool, &username).await;
        let presented = logins(&pool, &id_token);

        let mut identities: Vec<(&str, String, String)> = Vec::new();
        for (pool_kind, identity_pool_id) in [("checked", &checked), ("unchecked", &unchecked)] {
            let got = emulator
                .identity_ok(
                    "GetId",
                    &json!({"IdentityPoolId": identity_pool_id, "Logins": &presented}),
                )
                .await;
            let identity_id = got["IdentityId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: GetId answers an IdentityId: {got}"))
                .to_string();
            identities.push((pool_kind, identity_pool_id.clone(), identity_id));
        }

        for (operation, _) in &calls {
            emulator
                .user_pools_ok(
                    operation,
                    &json!({"UserPoolId": &pool.user_pool_id, "Username": &username}),
                )
                .await;
        }

        for (pool_kind, identity_pool_id, identity_id) in &identities {
            let (status, body) = emulator
                .identity(
                    "GetId",
                    &json!({"IdentityPoolId": identity_pool_id, "Logins": &presented}),
                )
                .await;
            if refused && *pool_kind == "checked" {
                assert_refused_inactive(case, "the checked pool's GetId", status, &body);
            } else {
                assert_eq!(
                    status, 200,
                    "case {case:?}: the {pool_kind} pool's GetId must still resolve this token, \
                     got {status} with {body}"
                );
            }

            // The basic (classic) flow mints an OpenID Connect token for the identity, which
            // the documented rule covers in the same words as the credential vend.
            let (status, body) = emulator
                .identity(
                    "GetOpenIdToken",
                    &json!({"IdentityId": identity_id, "Logins": &presented}),
                )
                .await;
            if refused && *pool_kind == "checked" {
                assert_refused_inactive(case, "the checked pool's GetOpenIdToken", status, &body);
            } else {
                assert_eq!(
                    status, 200,
                    "case {case:?}: the {pool_kind} pool's GetOpenIdToken must still mint for \
                     this token, got {status} with {body}"
                );
                assert!(
                    body["Token"].as_str().is_some_and(|token| !token.is_empty()),
                    "case {case:?}: a served GetOpenIdToken carries a Token, got {body}"
                );
            }
        }
    }
}

/// RULE, the other half of "checks WITH the integrated user pools": the user the token
/// names is resolved in the user pool the provider entry names, so an ACTIVE user's token
/// keeps buying credentials however many times it is presented — the check reports on that
/// pool's state and consumes nothing — while a token whose subject that pool holds no user
/// for stands for nobody and is refused, which the twin asking for no check still serves.
#[tokio::test]
async fn a_checked_pool_decides_the_token_against_the_user_pool_it_names() {
    let emulator = Emulator::new();
    let pool = user_pool(&emulator, "sstc-decided").await;
    let checked = emulator
        .identity_pool(
            "sstc-decided-pool",
            &pool.provider_name,
            &pool.client_id,
            Some(true),
        )
        .await;
    let unchecked = emulator
        .identity_pool(
            "sstc-decided-twin",
            &pool.provider_name,
            &pool.client_id,
            Some(false),
        )
        .await;
    let id_token = signed_in_user(&emulator, &pool, "live-user").await;
    let presented = logins(&pool, &id_token);
    let got = emulator
        .identity_ok(
            "GetId",
            &json!({"IdentityPoolId": &checked, "Logins": &presented}),
        )
        .await;
    let identity_id = got["IdentityId"]
        .as_str()
        .unwrap_or_else(|| panic!("GetId answers an IdentityId: {got}"))
        .to_string();

    // An active user's token is spent as often as it is presented: the check reports on the
    // user pool's state and consumes nothing.
    for attempt in 1..=3 {
        let (status, body) = emulator
            .identity(
                "GetCredentialsForIdentity",
                &json!({"IdentityId": &identity_id, "Logins": &presented}),
            )
            .await;
        assert_vended(
            &format!("attempt {attempt} with an active user"),
            "checked",
            &identity_id,
            status,
            &body,
        );
    }

    // The same token with another SUBJECT: of the pool's own issuer, of the right form, and
    // naming a user that pool holds none of — so the pool it is checked with stands behind
    // nobody for it.
    let stranger = with_subject(&id_token, "7f3a1c2e-0000-4000-8000-0123456789ab");
    let (status, body) = emulator
        .identity(
            "GetId",
            &json!({"IdentityPoolId": &checked, "Logins": {&pool.provider_name: &stranger}}),
        )
        .await;
    assert_refused_inactive(
        "a token naming no user of the integrated pool",
        "the checked pool's GetId",
        status,
        &body,
    );
    // The twin asks for no check, so the very same value still resolves there: the flag is
    // the only variable.
    let (status, body) = emulator
        .identity(
            "GetId",
            &json!({"IdentityPoolId": &unchecked, "Logins": {&pool.provider_name: &stranger}}),
        )
        .await;
    assert_eq!(
        status, 200,
        "the unchecked twin resolves the same value, got {status} with {body}"
    );

    // And the token of the user the integrated pool DOES hold still vends afterwards:
    // nothing about the refusal above changed what that pool holds.
    let (status, body) = emulator
        .identity(
            "GetCredentialsForIdentity",
            &json!({"IdentityId": &identity_id, "Logins": &presented}),
        )
        .await;
    assert_vended("after the refusals", "checked", &identity_id, status, &body);
}

/// Vend for an identity of `identity_pool_id` presenting `presented`, and answer the ARN
/// `sts:GetCallerIdentity` reports under the vended credential — which is the principal that
/// credential STANDS FOR, read from the service that holds the session.
async fn vended_session_arn(
    emulator: &Emulator,
    case: &str,
    pool_kind: &str,
    identity_pool_id: &str,
    presented: &Value,
) -> String {
    let resolved = emulator
        .identity_ok(
            "GetId",
            &json!({"IdentityPoolId": identity_pool_id, "Logins": presented}),
        )
        .await;
    let identity_id = resolved["IdentityId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: GetId answers an IdentityId: {resolved}"))
        .to_string();
    let (status, body) = emulator
        .identity(
            "GetCredentialsForIdentity",
            &json!({"IdentityId": &identity_id, "Logins": presented}),
        )
        .await;
    assert_vended(case, pool_kind, &identity_id, status, &body);
    let access_key_id = body["Credentials"]["AccessKeyId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a served vend carries an AccessKeyId: {body}"));
    let (status, reported) = emulator.caller_identity(access_key_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetCallerIdentity under the vended credential expected HTTP 200, got \
         {status} with {reported}"
    );
    element(&reported, "Arn")
}

/// RULE, the half of the check that SERVES — the control every server-side-token-check
/// scenario rests on: a provider entry asking for the check revalidates the login and,
/// where the integrated user pool still holds the token's user signed in, federates it
/// NORMALLY, so the credentials vended are a session of the role that identity pool's own
/// configuration resolves — `sts:GetCallerIdentity` under them answers
/// `arn:<partition>:sts::<account>:assumed-role/<role>/CognitoIdentityCredentials` for THAT
/// pool's role, never the principal the request itself was served under. The check decides
/// WHETHER the login is honoured; it never decides which role an honoured one runs as.
///
/// The rows differ along what the rule generalises over: the `ServerSideTokenCheck` the
/// provider entry carries (asked for, declined, left out), and with it the pool the very
/// same live token is presented to — each pool carrying its own authenticated role, so the
/// vended principal can only match by following the pool's configuration.
#[tokio::test]
async fn a_live_user_pool_token_vends_a_session_of_the_pools_authenticated_role() {
    let emulator = Emulator::new();
    let pool = user_pool(&emulator, "sstc-principal").await;
    let id_token = signed_in_user(&emulator, &pool, "principal-user").await;
    let presented = logins(&pool, &id_token);

    for (case, pool_kind, pool_name, check) in [
        ("the check asked for", "checked", "sstc-vend-on", Some(true)),
        ("the check declined", "unchecked", "sstc-vend-off", Some(false)),
        ("no check stated", "unstated", "sstc-vend-absent", None),
    ] {
        let identity_pool_id = emulator
            .identity_pool(pool_name, &pool.provider_name, &pool.client_id, check)
            .await;
        let arn =
            vended_session_arn(&emulator, case, pool_kind, &identity_pool_id, &presented).await;
        // The role `Emulator::identity_pool` seeded and configured on THIS pool.
        let expected = expected_session_arn(&format!("{pool_name}-auth"));
        assert_eq!(
            arn, expected,
            "case {case:?}: the vend must be a session of the pool's own authenticated role \
             ({expected}), got {arn:?}"
        );
    }
}

/// RULE, the same rule read over a pool whose configuration CHANGES: the role a vend stands
/// for is the one the pool resolves at the time of the vend, so re-pointing a checked pool's
/// `authenticated` role moves the principal the very same live token vends to the new role,
/// and a pool that resolves no role at all vends nothing rather than some other principal
/// (`InvalidIdentityPoolConfigurationException`, the error this operation models for a pool
/// with "no authenticated role configured").
#[tokio::test]
async fn a_checked_pools_vend_follows_the_role_its_configuration_names() {
    let emulator = Emulator::new();
    let pool = user_pool(&emulator, "sstc-reconfigured").await;
    let id_token = signed_in_user(&emulator, &pool, "reconfigured-user").await;
    let presented = logins(&pool, &id_token);
    let identity_pool_id = emulator
        .identity_pool(
            "sstc-moved",
            &pool.provider_name,
            &pool.client_id,
            Some(true),
        )
        .await;

    let first = vended_session_arn(
        &emulator,
        "the role first configured",
        "checked",
        &identity_pool_id,
        &presented,
    )
    .await;
    assert_eq!(
        first,
        expected_session_arn("sstc-moved-auth"),
        "the first vend must be a session of the role the pool was created with, got {first:?}"
    );

    // A second role of the same account, admitting the same pool's users, configured in
    // place of the first.
    let moved = seed_role(
        &emulator.engine,
        &seeded_role_arn("sstc-moved", "second"),
        cognito_trust_policy(&identity_pool_id),
    );
    emulator
        .identity_ok(
            "SetIdentityPoolRoles",
            &json!({"IdentityPoolId": &identity_pool_id, "Roles": {"authenticated": &moved}}),
        )
        .await;
    let after = vended_session_arn(
        &emulator,
        "the role now configured",
        "checked",
        &identity_pool_id,
        &presented,
    )
    .await;
    assert_eq!(
        after,
        expected_session_arn("sstc-moved-second"),
        "after the pool was re-pointed the vend must be a session of the role now \
         configured, got {after:?}"
    );

    // And a pool left resolving NO role for this exchange vends nothing at all: the
    // refusal is the pool-configuration one, with no credential in the answer.
    emulator
        .identity_ok(
            "SetIdentityPoolRoles",
            &json!({"IdentityPoolId": &identity_pool_id, "Roles": {}}),
        )
        .await;
    let resolved = emulator
        .identity_ok(
            "GetId",
            &json!({"IdentityPoolId": &identity_pool_id, "Logins": &presented}),
        )
        .await;
    let identity_id = resolved["IdentityId"]
        .as_str()
        .unwrap_or_else(|| panic!("GetId answers an IdentityId: {resolved}"))
        .to_string();
    let (status, body) = emulator
        .identity(
            "GetCredentialsForIdentity",
            &json!({"IdentityId": &identity_id, "Logins": &presented}),
        )
        .await;
    assert_eq!(
        status, 400,
        "a pool resolving no role must refuse the vend with HTTP 400, got {status} with {body}"
    );
    let code = error_code(&body);
    assert!(
        code.ends_with("InvalidIdentityPoolConfigurationException"),
        "a pool resolving no role must refuse with InvalidIdentityPoolConfigurationException, \
         got {code:?} in {body}"
    );
    assert!(
        body.get("Credentials").is_none(),
        "a refused vend carries no credential, got {body}"
    );
}
