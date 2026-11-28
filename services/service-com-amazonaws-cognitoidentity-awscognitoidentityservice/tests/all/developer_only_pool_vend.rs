// SPDX-License-Identifier: CC0-1.0

//! Whether the OpenID token this service's OWN developer-identity mint handed back is
//! redeemable at the credential exchange of a pool whose only configured provider is its
//! developer one.
//!
//! RULE: `GetCredentialsForIdentity` honours a `Logins` entry keyed by the Cognito Identity
//! principal whenever it carries an OpenID token THIS pool minted for the identity named
//! whose validity window is still open — "If the token is for cognito-identity.amazonaws.com,
//! it will be passed through to Security Token Service with the appropriate role for the
//! token" (`smithy_query(query="doc",shape="GetCredentialsForIdentityInput")`) — and answers
//! it with the credentials of a session of the role the pool resolves for that request. What
//! ELSE the pool is configured with decides nothing about that entry: whether it admits
//! unauthenticated identities, whether it declares a public login provider beside its
//! developer one, which of its two roles it configures, and what its role's trust policy
//! conditions the assume on, leave the pool-issued token the login of its identity that it
//! is. Neither does the window the mint was ASKED for: every `TokenDuration` in the modelled
//! `@range 1..86400`
//! (`smithy_query(query="operation",operation="GetOpenIdTokenForDeveloperIdentity")`), and the
//! member's absence, hand back a token the exchange redeems.
//!
//! The complement is the reserved refusal: a value presented under that same provider name
//! that NO issuance of this pool resolves — the developer user identifier the identity is
//! registered under, an opaque credential of the caller's own back end, a token another pool
//! minted — is the modelled `NotAuthorizedException` carrying AWS's fixed sentence
//! `Invalid login token. Not a Cognito token.`, so the sentence stays reserved for a value
//! that really is no Cognito token of this pool.
//!
//! What this suite asserts that `developer_login_credential_exchange` does not: that one
//! tables WHICH sentence each kind of unresolvable value is refused with, against a pool that
//! admits guests and is fronted by a public provider. The axis here is the POOL and the MINT —
//! a pool whose only configured provider is its developer one, admitting no guest, with only
//! the authenticated role and a trust policy conditioned on the authentication methods, over
//! the whole modelled window range — which is the shape a developer-authenticated client
//! actually creates, and the shape whose vend was refused. Its refusal rows are the control
//! that the served rows above were served for their token rather than for that pool shape.
//!
//! The rows span what the rule generalises over: the pool's shape (admitting unauthenticated
//! identities or not, declaring a public provider or not, configuring both roles or only the
//! authenticated one, a trust policy conditioned on the audience alone or on the
//! authentication methods too), the `TokenDuration` the mint is asked for (absent, each
//! published bound, and one in between) and the form of the developer user identifier the
//! token was minted for.
//!
//! Each served vend is read back through the service that OWNS the session it mints:
//! `sts:GetCallerIdentity` under the vended credential answers
//! `arn:<partition>:sts::<account>:assumed-role/<role>/CognitoIdentityCredentials`, so a
//! credential nothing resolves cannot pass. The harness is `vended_role_session`'s: one
//! shared engine with this service and STS mounted on it behind the same `MultiServiceRouter`
//! the production router builds, with the role a vend assumes seeded into the IAM store of
//! that same engine.

use serde_json::{json, Value};

use crate::identity_pool_flow::error_code;
use crate::vended_role_session::{element, expected_session_arn, trust_policy, vended, Emulator};

/// The operations the rule is asserted over.
const MINT: &str = "GetOpenIdTokenForDeveloperIdentity";
const EXCHANGE: &str = "GetCredentialsForIdentity";

/// The `Logins` provider name a token minted by THIS service is presented under. Spelled as a
/// literal so the wire-visible name is pinned by the test rather than by the code under test.
const COGNITO_IDENTITY_PROVIDER: &str = "cognito-identity.amazonaws.com";

/// The pool's own `DeveloperProviderName` — the "domain" its developer logins are made under.
const DEVELOPER_PROVIDER: &str = "login.example.backend";

/// A public login provider the rows that declare one are fronted by: one of the names
/// `GetId$Logins` publishes, whose login is an opaque OAuth access token.
const PUBLIC_PROVIDER: &str = "graph.facebook.com";

/// The status every refusal of this service travels with, whatever `@httpError` the shape it
/// answers is annotated with (`crate::service` owns that rule).
const REFUSED_STATUS: u16 = 400;

/// AWS's fixed sentence for a value presented under this service's own provider name that is
/// no Cognito token of the pool at all. Spelled as a literal so the wire-visible text is
/// pinned by the test: it is fixed text, with no part taken from the request or from state.
const NOT_A_COGNITO_TOKEN: &str = "Invalid login token. Not a Cognito token.";

/// The authentication method an exchange that presented a login is made under, which the
/// `cognito-identity.amazonaws.com:amr` condition of the trust policy the Amazon Cognito
/// Developer Guide publishes for an authenticated role tests for.
const AUTHENTICATED_METHOD: &str = "authenticated";

/// One pool shape a row is run against.
struct PoolShape {
    case: &'static str,
    /// The pool's `AllowUnauthenticatedIdentities` — the scenario this suite covers is a pool
    /// that admits no guest at all.
    allow_unauthenticated: bool,
    /// Whether the pool declares a PUBLIC login provider beside its developer one.
    declares_public_provider: bool,
    /// Whether `SetIdentityPoolRoles` configures the `unauthenticated` role as well.
    configures_unauthenticated_role: bool,
    /// Whether the authenticated role's trust policy conditions the assume on the
    /// authentication methods the exchange publishes as well as on the audience.
    conditions_on_methods: bool,
}

/// The pool shapes the rule is asserted over, each with the authenticated role it configures.
fn pool_shapes() -> Vec<PoolShape> {
    vec![
        PoolShape {
            case: "a developer-only pool admitting no unauthenticated identity, authenticated \
                   role alone",
            allow_unauthenticated: false,
            declares_public_provider: false,
            configures_unauthenticated_role: false,
            conditions_on_methods: false,
        },
        PoolShape {
            case: "the same pool whose role admits the assume only for an authenticated method",
            allow_unauthenticated: false,
            declares_public_provider: false,
            configures_unauthenticated_role: false,
            conditions_on_methods: true,
        },
        PoolShape {
            case: "a developer-only pool with both roles configured",
            allow_unauthenticated: false,
            declares_public_provider: false,
            configures_unauthenticated_role: true,
            conditions_on_methods: false,
        },
        PoolShape {
            case: "a pool that admits unauthenticated identities too",
            allow_unauthenticated: true,
            declares_public_provider: false,
            configures_unauthenticated_role: true,
            conditions_on_methods: false,
        },
        PoolShape {
            case: "a pool fronted by a public provider beside its developer one",
            allow_unauthenticated: false,
            declares_public_provider: true,
            configures_unauthenticated_role: false,
            conditions_on_methods: true,
        },
    ]
}

/// A trust policy admitting the Cognito Identity principal for the pool `audience`, as the
/// Amazon Cognito Developer Guide publishes one for an AUTHENTICATED role: the audience
/// condition that keeps one pool out of another pool's role, and the
/// `cognito-identity.amazonaws.com:amr` condition that admits only an exchange made with a
/// login.
fn authenticated_trust_policy(audience: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Federated": COGNITO_IDENTITY_PROVIDER},
            "Action": "sts:AssumeRoleWithWebIdentity",
            "Condition": {
                "StringEquals": {"cognito-identity.amazonaws.com:aud": audience},
                "ForAnyValue:StringLike": {
                    "cognito-identity.amazonaws.com:amr": AUTHENTICATED_METHOD,
                },
            },
        }],
    })
}

/// Create the pool `shape` describes, seed the roles it configures into the IAM store of the
/// same engine and write its role configuration. Answers its `IdentityPoolId` beside the NAME
/// of the authenticated role a vend of it is a session of.
async fn configured_pool(emulator: &Emulator, name: &str, shape: &PoolShape) -> (String, String) {
    let mut request = json!({
        "IdentityPoolName": name,
        "AllowUnauthenticatedIdentities": shape.allow_unauthenticated,
        "DeveloperProviderName": DEVELOPER_PROVIDER,
    });
    if shape.declares_public_provider {
        request["SupportedLoginProviders"] = json!({PUBLIC_PROVIDER: "0123456789"});
    }
    let pool_id = emulator.ok("CreateIdentityPool", &request).await["IdentityPoolId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateIdentityPool answers an IdentityPoolId for {name}"))
        .to_string();

    let authenticated_role = format!("{name}-auth");
    let policy = if shape.conditions_on_methods {
        authenticated_trust_policy(&pool_id)
    } else {
        trust_policy(&pool_id)
    };
    let authenticated = emulator.seed_role(&authenticated_role, Some(policy));
    let mut roles = json!({"authenticated": authenticated});
    if shape.configures_unauthenticated_role {
        let unauthenticated =
            emulator.seed_role(&format!("{name}-unauth"), Some(trust_policy(&pool_id)));
        roles["unauthenticated"] = json!(unauthenticated);
    }
    emulator
        .ok(
            "SetIdentityPoolRoles",
            &json!({"IdentityPoolId": &pool_id, "Roles": roles}),
        )
        .await;
    (pool_id, authenticated_role)
}

/// Mint a developer identity of `pool_id` for `identifier`, asking for `token_duration` (the
/// member is left out when the row carries none). Answers the `IdentityId` and the `Token` the
/// mint handed back.
async fn minted(
    emulator: &Emulator,
    case: &str,
    pool_id: &str,
    identifier: &str,
    token_duration: Option<i64>,
) -> (String, String) {
    let mut request = json!({
        "IdentityPoolId": pool_id,
        "Logins": {DEVELOPER_PROVIDER: identifier},
    });
    if let Some(token_duration) = token_duration {
        request["TokenDuration"] = json!(token_duration);
    }
    let answered = emulator.ok(MINT, &request).await;
    let identity_id = answered["IdentityId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: {MINT} answers an IdentityId: {answered}"))
        .to_string();
    let token = answered["Token"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: {MINT} answers a Token: {answered}"))
        .to_string();
    assert!(
        !token.is_empty(),
        "case {case:?}: {MINT} answers a non-empty Token, got {answered}"
    );
    (identity_id, token)
}

/// The exchange: `GetCredentialsForIdentity` for `identity_id` presenting `presented` under
/// this service's own provider name.
async fn exchange(emulator: &Emulator, identity_id: &str, presented: &str) -> (u16, Value) {
    emulator
        .call(
            EXCHANGE,
            &json!({
                "IdentityId": identity_id,
                "Logins": {COGNITO_IDENTITY_PROVIDER: presented},
            }),
        )
        .await
}

/// RULE: a token this pool's developer-identity mint handed back is redeemed for the
/// credentials of a session of the role the pool resolves, whatever else the pool is
/// configured with and whatever window the mint was asked for.
#[tokio::test]
async fn a_pool_issued_developer_identity_token_is_redeemed_for_a_session_of_the_pools_role() {
    let emulator = Emulator::new();

    // (the window the mint is asked for, the developer user identifier it is minted for):
    // the member's absence, each published bound of the modelled `@range 1..86400`, and one
    // in between, against identifiers of different forms.
    let windows: Vec<(Option<i64>, &str)> = vec![
        (None, "backend-user-1"),
        (Some(1), "u"),
        (Some(900), "user@example.com"),
        (Some(86_400), "ou=people/uid=42"),
    ];

    for (index, shape) in pool_shapes().iter().enumerate() {
        let (pool_id, role_name) =
            configured_pool(&emulator, &format!("developer-only-vend-{index}"), shape).await;

        for (token_duration, identifier) in &windows {
            let case = format!(
                "{}, TokenDuration {token_duration:?} for {identifier:?}",
                shape.case
            );
            let (identity_id, token) =
                minted(&emulator, &case, &pool_id, identifier, *token_duration).await;

            let (status, body) = exchange(&emulator, &identity_id, &token).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a token this pool minted is exchanged, got {status} with {body}"
            );
            assert_eq!(
                body["IdentityId"].as_str(),
                Some(identity_id.as_str()),
                "case {case:?}: the vend names the identity it was asked for, got {body}"
            );
            let (access_key_id, _) = vended(&body);
            assert!(
                body["Credentials"]["Expiration"]
                    .as_f64()
                    .is_some_and(|expiration| expiration > 0.0),
                "case {case:?}: a vend carries the instant its credentials expire, got {body}"
            );

            // The session the vend minted, read back from the service that OWNS it.
            let (caller_status, caller) = emulator.caller_identity(access_key_id).await;
            assert_eq!(
                caller_status, 200,
                "case {case:?}: GetCallerIdentity under the vended credential expected 200, got \
                 {caller_status} with {caller}"
            );
            let arn = element(&caller, "Arn");
            assert_eq!(
                arn,
                expected_session_arn(&role_name),
                "case {case:?}: expected a session of {role_name}, got {arn}"
            );
        }
    }
}

/// RULE, complement: under that same provider name, a value NO issuance of this pool resolves
/// buys nothing and carries AWS's fixed sentence for a value that is no Cognito token of this
/// pool — so the sentence the pool's own token is NOT answered with stays reserved for one.
#[tokio::test]
async fn a_value_this_pool_never_minted_keeps_the_reserved_refusal() {
    let emulator = Emulator::new();
    let shape = PoolShape {
        case: "a developer-only pool admitting no unauthenticated identity",
        allow_unauthenticated: false,
        declares_public_provider: false,
        configures_unauthenticated_role: false,
        conditions_on_methods: true,
    };
    let (pool_id, role_name) = configured_pool(&emulator, "developer-only-refusal", &shape).await;
    // A second pool of the same shape, whose own mint is where the foreign token comes from.
    let (other_pool_id, _) =
        configured_pool(&emulator, "developer-only-refusal-other", &shape).await;

    let identifier = "backend-user-1";
    let (identity_id, own_token) =
        minted(&emulator, "the pool's own mint", &pool_id, identifier, Some(900)).await;
    let (_, foreign_token) = minted(
        &emulator,
        "another pool's mint",
        &other_pool_id,
        identifier,
        Some(900),
    )
    .await;

    // (case, the value presented under this service's provider name)
    let rows: Vec<(&str, String)> = vec![
        (
            "the developer user identifier the identity is registered under",
            identifier.to_string(),
        ),
        (
            "an opaque credential of the caller's own back end",
            "an-opaque-backend-token".to_string(),
        ),
        (
            "a token another identity pool's mint handed back",
            foreign_token.clone(),
        ),
    ];

    for (case, presented) in &rows {
        let (status, body) = exchange(&emulator, &identity_id, presented).await;
        assert_eq!(
            status, REFUSED_STATUS,
            "case {case:?}: expected HTTP {REFUSED_STATUS} for a refused exchange, got {status} \
             with {body}"
        );
        let code = error_code(&body);
        assert!(
            code.ends_with("NotAuthorizedException"),
            "case {case:?}: expected NotAuthorizedException, got {code:?} in {body}"
        );
        let message = body["message"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: an error carries a message: {body}"));
        assert_eq!(
            message, NOT_A_COGNITO_TOKEN,
            "case {case:?}: expected the fixed sentence {NOT_A_COGNITO_TOKEN:?}, got {message:?}"
        );
        assert!(
            body.get("Credentials").is_none(),
            "case {case:?}: a refused exchange vends nothing, got {body}"
        );
    }

    // The control: the pool's OWN token, presented to that same identity of that same pool, is
    // served — so every row above was refused for the value it presented and not for the pool,
    // the identity or the role.
    let (status, body) = exchange(&emulator, &identity_id, &own_token).await;
    assert_eq!(
        status, 200,
        "the pool's own token is exchanged, got {status} with {body}"
    );
    let (access_key_id, _) = vended(&body);
    let (caller_status, caller) = emulator.caller_identity(access_key_id).await;
    assert_eq!(
        caller_status, 200,
        "GetCallerIdentity under the vended credential expected 200, got {caller_status} with \
         {caller}"
    );
    assert_eq!(
        element(&caller, "Arn"),
        expected_session_arn(&role_name),
        "the control vend is a session of {role_name}, got {caller}"
    );
}
