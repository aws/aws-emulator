// SPDX-License-Identifier: CC0-1.0

//! WHICH refusal a value presented under this service's own provider name is answered with
//! at the credential exchange, and WHEN that is decided.
//!
//! RULE: `GetCredentialsForIdentity` honours a `Logins` entry made under this service's own
//! provider name only as an OpenID token THIS pool minted — "If the token is for
//! cognito-identity.amazonaws.com, it will be passed through to Security Token Service with
//! the appropriate role for the token"
//! (`smithy_query(query="doc",shape="GetCredentialsForIdentityInput")`) — so a value no
//! issuance of this pool resolves buys no credential, and WHICH of AWS's two fixed sentences
//! the modelled `NotAuthorizedException` carries follows from what the value itself claims to
//! be:
//!
//! * a value that does not even claim this service as its issuer — a developer user
//!   identifier, an opaque credential of some other provider, a token of another issuer's
//!   claim set — is no Cognito token at all: `Invalid login token. Not a Cognito token.`,
//!   which is the sentence a token ANOTHER identity pool minted carries as well;
//! * a value of this service's OWN token form, whose `iss` claim names this service, claims
//!   to carry a signature of this service that nothing here vouches for:
//!   `Invalid login token. Couldn't verify signed token.`;
//! * a token this pool DID mint whose window has closed is the expired token it is:
//!   `Invalid login token. Token is expired.` (`login_token_window`'s rule).
//!
//! And the presented login is decided BEFORE the role the vend would assume: which role a
//! request may NAME (`CustomRoleArn`, which "Only SAML providers and providers with
//! RoleMappings support"), and whether the pool has a role to resolve at all, are questions
//! about a request whose login stands up — so a request presenting one that does not is
//! answered for the LOGIN, whatever else it asked for.
//!
//! The rows span the class: the value presented (a developer user identifier, an opaque
//! string, a JWS of another issuer, a JWS of this service's own form that no mint produced, a
//! token minted for another identity of the same pool, a token minted in another pool, the
//! identity's own token), whether the map carries that entry alone or beside a
//! public-provider login, what else the request asks for (nothing, a `CustomRoleArn` the
//! pool's configuration does not permit) and what the pool's role configuration is (both
//! roles configured, none at all).
//!
//! The harness is `identity_pool_flow`'s: one shared engine behind the same
//! `MultiServiceRouter` the production router builds, with the role a vend assumes seeded
//! into the IAM store of that same engine.

use serde_json::{json, Map, Value};

use crate::developer_identity_tokens::token_request;
use crate::identity_pool_flow::{cognito_trust_policy, error_code, seeded_role_arn, Emulator};
use crate::mapped_role_resolution::identity_token;

/// The operation the rule is asserted at.
const OPERATION: &str = "GetCredentialsForIdentity";

/// The `Logins` provider name a token minted by THIS service is presented under. Spelled as a
/// literal so the wire-visible name is pinned by the test rather than by the code under test.
const COGNITO_IDENTITY_PROVIDER: &str = "cognito-identity.amazonaws.com";

/// The issuer this service stamps into the tokens it mints, as a value of its own token form
/// carries it.
const COGNITO_IDENTITY_ISSUER: &str = "https://cognito-identity.amazonaws.com";

/// The pool's own `DeveloperProviderName` — the "domain" its developer logins are made under.
const DEVELOPER_PROVIDER: &str = "login.example.backend";

/// A public login provider, for the rows whose map carries a second entry beside the one
/// under test: one of the names `GetId$Logins` publishes, whose login is an opaque OAuth
/// access token.
const PUBLIC_PROVIDER: &str = "graph.facebook.com";

/// The status every refusal of this service travels with, whatever `@httpError` the shape it
/// answers is annotated with (`crate::service` owns that rule).
const REFUSED_STATUS: u16 = 400;

/// AWS's fixed sentences, spelled as literals so the wire-visible text is pinned by the test:
/// each is fixed text, with no part taken from the request or from stored state.
const NOT_A_COGNITO_TOKEN: &str = "Invalid login token. Not a Cognito token.";
const COULD_NOT_VERIFY_TOKEN: &str = "Invalid login token. Couldn't verify signed token.";
const UNMATCHED_LOGINS: &str = "Logins don't match. Please include at least one valid login \
                                for this identity or identity pool.";
const CUSTOM_ROLE_ARN_UNSUPPORTED: &str =
    "Only SAML providers and providers with RoleMappings support custom role ARN.";

/// A pool that names the developer provider a token is minted through and the public provider
/// the rows present a second login of, and — when `with_roles` — the two roles a vend assumes,
/// seeded into the IAM store of the same engine. Answers its `IdentityPoolId`.
async fn vending_pool(emulator: &Emulator, name: &str, with_roles: bool) -> String {
    let created = emulator
        .ok(
            "CreateIdentityPool",
            &json!({
                "IdentityPoolName": name,
                "AllowUnauthenticatedIdentities": true,
                "DeveloperProviderName": DEVELOPER_PROVIDER,
                "SupportedLoginProviders": {PUBLIC_PROVIDER: "0123456789"},
            }),
        )
        .await;
    let pool_id = created["IdentityPoolId"]
        .as_str()
        .expect("CreateIdentityPool answers an IdentityPoolId")
        .to_string();
    if with_roles {
        let authenticated =
            emulator.seed_role(&seeded_role_arn(name, "auth"), cognito_trust_policy(&pool_id));
        let unauthenticated = emulator.seed_role(
            &seeded_role_arn(name, "unauth"),
            cognito_trust_policy(&pool_id),
        );
        emulator
            .ok(
                "SetIdentityPoolRoles",
                &json!({
                    "IdentityPoolId": &pool_id,
                    "Roles": {
                        "authenticated": authenticated,
                        "unauthenticated": unauthenticated,
                    },
                }),
            )
            .await;
    }
    pool_id
}

/// Register `identifier` as a developer user of `pool_id`, and answer the identity it names
/// with the OpenID token the mint handed back.
async fn register(emulator: &Emulator, pool_id: &str, identifier: &str) -> (String, String) {
    let minted = emulator
        .ok(
            "GetOpenIdTokenForDeveloperIdentity",
            &token_request(pool_id, &[(DEVELOPER_PROVIDER, identifier)], None, None),
        )
        .await;
    let identity_id = minted["IdentityId"]
        .as_str()
        .unwrap_or_else(|| panic!("the mint answers an IdentityId: {minted}"))
        .to_string();
    let token = minted["Token"]
        .as_str()
        .unwrap_or_else(|| panic!("the mint answers a Token: {minted}"))
        .to_string();
    (identity_id, token)
}

/// A compact JWS carrying `claims` that no mint of this service produced.
fn forged_token(issuer: &str, pool_id: &str, identity_id: &str) -> String {
    identity_token(&json!({
        "iss": issuer,
        "aud": pool_id,
        "sub": identity_id,
        // An instant far ahead of any run of this suite, so a row is decided on the issuance
        // it has none of rather than on a window.
        "exp": 4_102_444_800u64,
    }))
}

/// The exchange itself: `GetCredentialsForIdentity` for `identity_id`, presenting `logins` and
/// the `CustomRoleArn` the row names (none when it names none).
async fn exchange(
    emulator: &Emulator,
    identity_id: &str,
    logins: &Value,
    custom_role_arn: Option<&str>,
) -> (u16, Value) {
    let mut request = Map::new();
    request.insert("IdentityId".to_string(), json!(identity_id));
    request.insert("Logins".to_string(), logins.clone());
    if let Some(custom_role_arn) = custom_role_arn {
        request.insert("CustomRoleArn".to_string(), json!(custom_role_arn));
    }
    emulator.call(OPERATION, &Value::Object(request)).await
}

/// The `message` an error response carries.
fn error_message(case: &str, body: &Value) -> String {
    body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: an error response carries a message: {body}"))
        .to_string()
}

/// Assert that the exchange was refused with `code` and the fixed sentence `message`, and that
/// it vended nothing.
fn assert_refused(case: &str, status: u16, body: &Value, code: &str, message: &str) {
    assert_eq!(
        status, REFUSED_STATUS,
        "case {case:?}: expected HTTP {REFUSED_STATUS} for a refused exchange, got {status} \
         with {body}"
    );
    let answered = error_code(body);
    assert!(
        answered.ends_with(code),
        "case {case:?}: expected {code}, got {answered:?} in {body}"
    );
    let actual = error_message(case, body);
    assert_eq!(
        actual, message,
        "case {case:?}: expected the fixed sentence {message:?}, got {actual:?}"
    );
    assert!(
        body.get("Credentials").is_none(),
        "case {case:?}: a refused exchange vends nothing, got {body}"
    );
}

/// RULE: a value presented under this service's own provider name is honoured only as a token
/// this pool minted for the identity named, and the sentence its refusal carries follows from
/// what the value claims to be — whatever else the `Logins` map carries.
#[tokio::test]
async fn a_value_this_pool_never_minted_is_refused_as_the_token_it_claims_to_be() {
    let emulator = Emulator::new();
    let pool_id = vending_pool(&emulator, "login-token-form", true).await;
    let other_pool_id = vending_pool(&emulator, "login-token-form-other", true).await;

    let (identity_id, own_token) = register(&emulator, &pool_id, "backend-user-1").await;
    let (_, sibling_token) = register(&emulator, &pool_id, "backend-user-2").await;
    let (_, foreign_token) = register(&emulator, &other_pool_id, "backend-user-1").await;

    // (case, the value presented under this service's provider name, the sentence owed —
    // `None` for the row the exchange is SERVED for)
    let rows: Vec<(&str, String, Option<&str>)> = vec![
        (
            "the developer user identifier the identity is registered under",
            "backend-user-1".to_string(),
            Some(NOT_A_COGNITO_TOKEN),
        ),
        (
            "an opaque string that is no token at all",
            "an-opaque-backend-token".to_string(),
            Some(NOT_A_COGNITO_TOKEN),
        ),
        (
            "a token of another issuer's claim set",
            forged_token("https://accounts.google.com", &pool_id, &identity_id),
            Some(NOT_A_COGNITO_TOKEN),
        ),
        (
            "a token minted in another identity pool",
            foreign_token.clone(),
            Some(NOT_A_COGNITO_TOKEN),
        ),
        (
            "a JWS of this service's own form that no mint produced",
            forged_token(COGNITO_IDENTITY_ISSUER, &pool_id, &identity_id),
            Some(COULD_NOT_VERIFY_TOKEN),
        ),
        (
            "a token minted for another identity of the same pool",
            sibling_token.clone(),
            Some(UNMATCHED_LOGINS),
        ),
        ("the identity's own minted token", own_token.clone(), None),
    ];

    // ...each presented alone and beside a login of the public provider, so the rule holds
    // over the SHAPE of the map as well as over the value.
    for (case, presented, owed) in &rows {
        for (shape, logins) in [
            ("alone", json!({COGNITO_IDENTITY_PROVIDER: presented})),
            (
                "beside a public-provider login",
                json!({
                    COGNITO_IDENTITY_PROVIDER: presented,
                    PUBLIC_PROVIDER: "an-oauth-access-token",
                }),
            ),
        ] {
            let case = format!("{case}, {shape}");
            let (status, body) = exchange(&emulator, &identity_id, &logins, None).await;
            match owed {
                Some(message) => {
                    assert_refused(&case, status, &body, "NotAuthorizedException", message)
                }
                None => {
                    assert_eq!(
                        status, 200,
                        "case {case:?}: the identity's own token is exchanged, got {status} \
                         with {body}"
                    );
                    assert!(
                        body["Credentials"]["AccessKeyId"].is_string(),
                        "case {case:?}: a served exchange carries Credentials.AccessKeyId, got \
                         {body}"
                    );
                }
            }
        }
    }
}

/// RULE: the presented login is decided BEFORE the role the vend would assume, so a value this
/// pool never minted is answered for the login — not with the `CustomRoleArn` parameter refusal
/// and not with the pool-configuration one — while a request whose login stands up still meets
/// both of those rules.
#[tokio::test]
async fn the_presented_login_is_decided_before_the_role_the_vend_would_assume() {
    let emulator = Emulator::new();
    let configured = vending_pool(&emulator, "login-token-ordering", true).await;
    // A pool in the same shape but with no role configuration at all: there the exchange has
    // no role to resolve, which is the OTHER refusal a vend answers after the login.
    let unconfigured = vending_pool(&emulator, "login-token-ordering-bare", false).await;

    let (identity_id, own_token) = register(&emulator, &configured, "ordering-user").await;
    let (bare_identity, bare_token) = register(&emulator, &unconfigured, "ordering-user").await;

    // A role ARN the pool's configuration does not permit the exchange to name: neither
    // provider of these pools is a SAML provider or one the pool holds a `RoleMappings` entry
    // for, so naming a custom role at all is the parameter refusal.
    let custom_role_arn = seeded_role_arn("login-token-ordering", "custom");

    // (case, the identity, the logins presented, the CustomRoleArn, the code owed, the
    // sentence owed)
    type Row<'a> = (&'a str, &'a str, Value, Option<&'a str>, &'a str, &'a str);
    let rows: Vec<Row<'_>> = vec![
        (
            "a developer user identifier beside a CustomRoleArn the pool does not permit",
            identity_id.as_str(),
            json!({COGNITO_IDENTITY_PROVIDER: "ordering-user"}),
            Some(custom_role_arn.as_str()),
            "NotAuthorizedException",
            NOT_A_COGNITO_TOKEN,
        ),
        (
            "a JWS of this service's own form beside that same CustomRoleArn",
            identity_id.as_str(),
            json!({
                COGNITO_IDENTITY_PROVIDER:
                    forged_token(COGNITO_IDENTITY_ISSUER, &configured, &identity_id),
            }),
            Some(custom_role_arn.as_str()),
            "NotAuthorizedException",
            COULD_NOT_VERIFY_TOKEN,
        ),
        (
            "the identity's own token beside a CustomRoleArn the pool does not permit",
            identity_id.as_str(),
            json!({COGNITO_IDENTITY_PROVIDER: &own_token}),
            Some(custom_role_arn.as_str()),
            "InvalidParameterException",
            CUSTOM_ROLE_ARN_UNSUPPORTED,
        ),
        (
            "an opaque value against a pool that configures no role",
            bare_identity.as_str(),
            json!({COGNITO_IDENTITY_PROVIDER: "an-opaque-backend-token"}),
            None,
            "NotAuthorizedException",
            NOT_A_COGNITO_TOKEN,
        ),
    ];

    for (case, identity, logins, custom_role_arn, code, owed) in &rows {
        let (status, body) = exchange(&emulator, identity, logins, *custom_role_arn).await;
        assert_refused(case, status, &body, code, owed);
    }

    // The control for the last row: the same pool, the same identity, a login that DOES stand
    // up — that request is the one the pool's own configuration refuses, which is how the row
    // above is known to have been answered for its login rather than for the pool.
    let (status, body) = exchange(
        &emulator,
        &bare_identity,
        &json!({COGNITO_IDENTITY_PROVIDER: &bare_token}),
        None,
    )
    .await;
    assert_eq!(
        status, REFUSED_STATUS,
        "a pool that configures no role has none to assume, got {status} with {body}"
    );
    let answered = error_code(&body);
    assert!(
        answered.ends_with("InvalidIdentityPoolConfigurationException"),
        "a login that stands up reaches the pool's own configuration, got {answered:?} in \
         {body}"
    );
}
