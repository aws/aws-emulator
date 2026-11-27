// SPDX-License-Identifier: CC0-1.0

//! HOW MANY credential exchanges one OpenID token this service minted buys.
//!
//! RULE: as many as it is presented for, while the window its issuance recorded is open. The
//! exchange is the pass-through that token was minted for — "If the token is for
//! `cognito-identity.amazonaws.com`, it will be passed through to Security Token Service with
//! the appropriate role for the token"
//! (`smithy_query(query="doc",shape="GetCredentialsForIdentity")`) — and the pass-through
//! READS the issuance rather than consuming it: the mint hands its caller a credential to keep
//! for the window it asked for, "You can specify a custom expiration time for the token so
//! that you can cache it"
//! (`smithy_query(query="doc",shape="GetOpenIdTokenForDeveloperIdentityInput",member="TokenDuration")`),
//! and a cached token is a token presented more than once. So a served exchange leaves the
//! token exchangeable, every presentation of it is answered with a session of its own, and the
//! only thing that ends it is the close of its window (`login_token_window`'s rule, and
//! `login_token_reuse_then_expiry`'s for the refusal it then carries).
//!
//! The reading is the same wherever a presented login is read, so a token an exchange was
//! served with is still the login it is at the later call that reads one — the exchange itself
//! and the unlink both read one.
//!
//! And a request is still decided for its presented login BEFORE its role-selection
//! parameters, so a request whose login is fine and whose `CustomRoleArn` the pool does not
//! permit is answered for the parameter — the token, having nothing wrong with it, is not what
//! the refusal names.
//!
//! The rows span what the rule generalises over: how the token was minted (`GetOpenIdToken`
//! for an unauthenticated identity, `GetOpenIdToken` for a federated one,
//! `GetOpenIdTokenForDeveloperIdentity` for a backend-authenticated one), the shape of the
//! second presentation (the token alone, beside a public-provider login, beside a
//! `CustomRoleArn` the pool does not permit), which refusal an earlier exchange was answered
//! with for the rows that are refused, and how many issuances of one identity are outstanding
//! at once.
//!
//! The harness is `identity_pool_flow`'s: one shared engine behind the same
//! `MultiServiceRouter` the production router builds, with the role a vend assumes seeded into
//! the IAM store of that same engine.

use serde_json::{json, Map, Value};

use crate::developer_identity_tokens::token_request;
use crate::identity_pool_flow::{cognito_trust_policy, error_code, seeded_role_arn, Emulator};

/// The operation the rule is asserted at, and the other call that reads a presented login.
const EXCHANGE: &str = "GetCredentialsForIdentity";
const UNLINK: &str = "UnlinkIdentity";

/// The `Logins` provider name a token minted by THIS service is presented under. Spelled as a
/// literal so the wire-visible name is pinned by the test rather than by the code under test.
const COGNITO_IDENTITY_PROVIDER: &str = "cognito-identity.amazonaws.com";

/// The pool's own `DeveloperProviderName` — the "domain" its developer logins are made under.
const DEVELOPER_PROVIDER: &str = "login.example.backend";

/// A public login provider the fixture pool is fronted by: one of the names `GetId$Logins`
/// publishes, whose login is an opaque OAuth access token.
const PUBLIC_PROVIDER: &str = "graph.facebook.com";

/// The status every refusal of this service travels with, whatever `@httpError` the shape it
/// answers is annotated with (`crate::service` owns that rule).
const REFUSED_STATUS: u16 = 400;

/// AWS's fixed sentences, spelled as literals so the wire-visible text is pinned by the test:
/// each is fixed text, with no part taken from the request or from stored state.
const NOT_A_COGNITO_TOKEN: &str = "Invalid login token. Not a Cognito token.";
const UNMATCHED_LOGINS: &str = "Logins don't match. Please include at least one valid login \
                                for this identity or identity pool.";
const CUSTOM_ROLE_ARN_UNSUPPORTED: &str =
    "Only SAML providers and providers with RoleMappings support custom role ARN.";

/// How many issuances of one identity the last case holds outstanding at once: more than two,
/// so "one each" is not "the second one".
const OUTSTANDING_ISSUANCES: usize = 3;

/// How many times one issuance is presented where the rule is "as often as it is presented":
/// more than twice, so "again" is not "once more".
const PRESENTATIONS: usize = 3;

/// A pool that mints tokens and vends credentials: it names the developer provider its
/// backend logins are made under and the public provider its federated logins are made under,
/// and its two roles are seeded into the IAM store of the same engine and configured on it —
/// the shape a pool has to be in for an exchange to be served at all. Answers its
/// `IdentityPoolId`.
async fn vending_pool(emulator: &Emulator, name: &str) -> String {
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
    pool_id
}

/// How the identity of one row was authenticated, and so which mint hands it a token.
#[derive(Clone, Copy)]
enum Mint {
    /// `GetOpenIdToken` for an identity that presented no login: an unauthenticated one.
    Guest,
    /// `GetOpenIdToken` for an identity reached through a login of the public provider.
    Federated(&'static str),
    /// `GetOpenIdTokenForDeveloperIdentity` for this developer user identifier.
    Developer(&'static str),
}

impl Mint {
    /// The `Logins` map a row's identity is reached through at the calls that take one, empty
    /// for the unauthenticated row.
    fn provider_logins(self) -> Value {
        match self {
            Mint::Guest | Mint::Developer(_) => json!({}),
            Mint::Federated(login) => json!({PUBLIC_PROVIDER: login}),
        }
    }
}

/// The identity of one row, minted once: a guest identity of the pool, the identity a
/// federated login resolves, or the one a developer user identifier is registered under.
async fn identity_of(emulator: &Emulator, pool_id: &str, how: Mint) -> String {
    match how {
        Mint::Guest => emulator.identity(pool_id, None).await,
        Mint::Federated(_) => {
            emulator
                .identity(pool_id, Some(how.provider_logins()))
                .await
        }
        Mint::Developer(user) => {
            let minted = emulator
                .ok(
                    "GetOpenIdTokenForDeveloperIdentity",
                    &token_request(pool_id, &[(DEVELOPER_PROVIDER, user)], None, None),
                )
                .await;
            minted["IdentityId"]
                .as_str()
                .unwrap_or_else(|| panic!("the mint answers an IdentityId: {minted}"))
                .to_string()
        }
    }
}

/// ONE issuance of `identity_id`, minted the way the row's identity was authenticated: every
/// call answers a token of its own for the same identity.
async fn mint(emulator: &Emulator, pool_id: &str, identity_id: &str, how: Mint) -> String {
    let minted = match how {
        Mint::Guest => {
            emulator
                .ok("GetOpenIdToken", &json!({"IdentityId": identity_id}))
                .await
        }
        Mint::Federated(_) => {
            emulator
                .ok(
                    "GetOpenIdToken",
                    &json!({"IdentityId": identity_id, "Logins": how.provider_logins()}),
                )
                .await
        }
        Mint::Developer(user) => {
            emulator
                .ok(
                    "GetOpenIdTokenForDeveloperIdentity",
                    &token_request(
                        pool_id,
                        &[(DEVELOPER_PROVIDER, user)],
                        Some(identity_id),
                        None,
                    ),
                )
                .await
        }
    };
    assert_eq!(
        minted["IdentityId"].as_str(),
        Some(identity_id),
        "a mint of one identity answers that identity, got {minted}"
    );
    minted["Token"]
        .as_str()
        .unwrap_or_else(|| panic!("a mint answers a Token: {minted}"))
        .to_string()
}

/// The exchange itself: `GetCredentialsForIdentity` for `identity_id`, presenting `logins` and
/// the `CustomRoleArn` the case names (none when it names none).
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
    emulator.call(EXCHANGE, &Value::Object(request)).await
}

/// Assert that the exchange was SERVED: a complete credential set for the identity asked for.
/// Answers the `AccessKeyId` of the session it minted, so a caller can tell one session from
/// the next.
fn assert_served(case: &str, identity_id: &str, status: u16, body: &Value) -> String {
    assert_eq!(
        status, 200,
        "case {case:?}: expected the HTTP 200 of a served exchange, got {status} with {body}"
    );
    assert_eq!(
        body["IdentityId"].as_str(),
        Some(identity_id),
        "case {case:?}: a served exchange answers the identity it was made for, got {body}"
    );
    for member in ["AccessKeyId", "SecretKey", "SessionToken", "Expiration"] {
        assert!(
            !body["Credentials"][member].is_null(),
            "case {case:?}: a served exchange carries Credentials.{member}, got {body}"
        );
    }
    body["Credentials"]["AccessKeyId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Assert that the call was refused with `code` and the fixed sentence `message`, and that it
/// vended nothing.
fn assert_refused(case: &str, status: u16, body: &Value, code: &str, message: &str) {
    assert_eq!(
        status, REFUSED_STATUS,
        "case {case:?}: expected HTTP {REFUSED_STATUS} for a refused call, got {status} with \
         {body}"
    );
    let answered = error_code(body);
    assert!(
        answered.ends_with(code),
        "case {case:?}: expected {code}, got {answered:?} in {body}"
    );
    let actual = body["message"].as_str().unwrap_or_default();
    assert_eq!(
        actual, message,
        "case {case:?}: expected the fixed sentence {message:?}, got {actual:?} in {body}"
    );
    assert!(
        body.get("Credentials").is_none(),
        "case {case:?}: a refused call vends nothing, got {body}"
    );
}

/// The rows: one per way an identity of one pool is authenticated, and so per mint that hands
/// it a token.
fn rows() -> Vec<(&'static str, Mint)> {
    vec![
        ("an unauthenticated identity's token", Mint::Guest),
        (
            "a federated identity's token",
            Mint::Federated("fb-access-token"),
        ),
        (
            "a developer-authenticated identity's token",
            Mint::Developer("backend-user"),
        ),
    ]
}

/// What a refused exchange of `a_refused_exchange_leaves_the_token_exchangeable` is refused
/// FOR: never the presented token's own validity, so each one has to leave the token
/// exchangeable.
#[derive(Clone, Copy)]
enum Refused {
    /// A `CustomRoleArn` the pool's configuration does not permit the exchange to name.
    CustomRoleArn,
    /// An identity the presented token is no login of.
    AnotherIdentity,
    /// A second `Logins` entry keyed by the pool's developer provider, which carries a
    /// developer user identifier rather than a token.
    DeveloperProviderLogin,
}

impl Refused {
    /// How the case reads in an assertion message.
    fn case(self) -> &'static str {
        match self {
            Refused::CustomRoleArn => "a CustomRoleArn the pool does not permit",
            Refused::AnotherIdentity => "an identity the token is no login of",
            Refused::DeveloperProviderLogin => "a developer-provider login beside the token",
        }
    }
}

/// RULE: a SERVED exchange leaves the issuance it was presented with exchangeable, so the same
/// token presented again — whichever mint handed it out, and in whatever shape — buys another
/// exchange, each with a session of its own.
#[tokio::test]
async fn a_served_exchange_leaves_the_token_it_was_presented_with_exchangeable() {
    let emulator = Emulator::new();
    let pool_id = vending_pool(&emulator, "token-reuse").await;

    for (row, how) in rows() {
        let identity_id = identity_of(&emulator, &pool_id, how).await;
        // The shapes the LATER presentations take: the token alone, and beside a login of the
        // public provider.
        let shapes: Vec<(&str, Value)> = vec![
            ("alone", json!({COGNITO_IDENTITY_PROVIDER: "<the token>"})),
            (
                "beside a public-provider login",
                json!({
                    COGNITO_IDENTITY_PROVIDER: "<the token>",
                    PUBLIC_PROVIDER: "fb-access-token",
                }),
            ),
        ];
        for (shape, map) in &shapes {
            let case = format!("{row}, presented again {shape}");
            let token = mint(&emulator, &pool_id, &identity_id, how).await;
            let mut vended: Vec<String> = Vec::new();
            let mut presented = map.clone();
            presented[COGNITO_IDENTITY_PROVIDER] = json!(&token);
            for presentation in 1..=PRESENTATIONS {
                let case = format!("{case}, presentation {presentation} of {PRESENTATIONS}");
                let (status, body) = exchange(&emulator, &identity_id, &presented, None).await;
                let access_key_id = assert_served(&case, &identity_id, status, &body);
                assert!(
                    !vended.contains(&access_key_id),
                    "case {case:?}: every exchange mints its own session, got \
                     {access_key_id} again"
                );
                vended.push(access_key_id);
            }
        }
    }
}

/// RULE, against the request's role-selection parameters: the presented login is decided
/// FIRST, so a request whose token is good and whose `CustomRoleArn` the pool does not permit
/// is answered for the parameter — and that refusal leaves the token exchangeable, as every
/// refusal does.
#[tokio::test]
async fn a_good_token_beside_an_impermissible_custom_role_arn_is_answered_for_the_parameter() {
    let emulator = Emulator::new();
    let pool_id = vending_pool(&emulator, "token-reuse-custom-role").await;
    // A role ARN this pool's configuration does not permit an exchange to name at all:
    // neither of its providers is a SAML provider or one the pool holds a `RoleMappings`
    // entry for.
    let custom_role_arn = seeded_role_arn("token-reuse-custom-role", "custom");

    for (row, how) in rows() {
        let identity_id = identity_of(&emulator, &pool_id, how).await;
        let token = mint(&emulator, &pool_id, &identity_id, how).await;
        let presented = json!({COGNITO_IDENTITY_PROVIDER: &token});

        // Served first, so the token is one an exchange has already been answered with.
        let (status, body) = exchange(&emulator, &identity_id, &presented, None).await;
        assert_served(&format!("{row}, the first exchange"), &identity_id, status, &body);

        let case = format!("{row}, beside a CustomRoleArn the pool does not permit");
        let (status, body) = exchange(
            &emulator,
            &identity_id,
            &presented,
            Some(custom_role_arn.as_str()),
        )
        .await;
        assert_refused(
            &case,
            status,
            &body,
            "InvalidParameterException",
            CUSTOM_ROLE_ARN_UNSUPPORTED,
        );
        assert_ne!(
            body["message"].as_str(),
            Some(NOT_A_COGNITO_TOKEN),
            "case {case:?}: a token an exchange was served with is no less a Cognito token, \
             got {body}"
        );

        // And the parameter's refusal cost the token nothing.
        let (status, body) = exchange(&emulator, &identity_id, &presented, None).await;
        assert_served(&format!("{case}, afterwards"), &identity_id, status, &body);
    }
}

/// RULE, the other side: an exchange that was REFUSED leaves the token it presented
/// exchangeable too — whichever refusal it was answered with.
#[tokio::test]
async fn a_refused_exchange_leaves_the_token_exchangeable() {
    let emulator = Emulator::new();
    let pool_id = vending_pool(&emulator, "token-refused-costs-nothing").await;
    let custom_role_arn = seeded_role_arn("token-refused-costs-nothing", "custom");
    // A second identity of the same pool, so a row can present one identity's token for
    // another.
    let other_identity = identity_of(&emulator, &pool_id, Mint::Developer("other-user")).await;

    for (row, how) in rows() {
        let identity_id = identity_of(&emulator, &pool_id, how).await;

        // The refusals a row drives: each is answered for something other than the presented
        // token's own validity — a parameter the request may not name, an identity the login
        // is none of, and a second `Logins` entry that carries no token at all.
        for refused in [
            Refused::CustomRoleArn,
            Refused::AnotherIdentity,
            Refused::DeveloperProviderLogin,
        ] {
            let token = mint(&emulator, &pool_id, &identity_id, how).await;
            let (named, logins, custom, code, message) = match refused {
                Refused::CustomRoleArn => (
                    &identity_id,
                    json!({COGNITO_IDENTITY_PROVIDER: &token}),
                    Some(custom_role_arn.as_str()),
                    "InvalidParameterException",
                    CUSTOM_ROLE_ARN_UNSUPPORTED,
                ),
                Refused::AnotherIdentity => (
                    &other_identity,
                    json!({COGNITO_IDENTITY_PROVIDER: &token}),
                    None,
                    "NotAuthorizedException",
                    UNMATCHED_LOGINS,
                ),
                Refused::DeveloperProviderLogin => (
                    &identity_id,
                    json!({
                        COGNITO_IDENTITY_PROVIDER: &token,
                        DEVELOPER_PROVIDER: "backend-user",
                    }),
                    None,
                    "NotAuthorizedException",
                    NOT_A_COGNITO_TOKEN,
                ),
            };
            let case = format!("{row}, refused for {}", refused.case());
            let (status, body) = exchange(&emulator, named, &logins, custom).await;
            assert_refused(&case, status, &body, code, message);

            // The refusal cost the token nothing: it still buys the exchange it was minted
            // for.
            let (status, body) = exchange(
                &emulator,
                &identity_id,
                &json!({COGNITO_IDENTITY_PROVIDER: &token}),
                None,
            )
            .await;
            assert_served(&format!("{case}, afterwards"), &identity_id, status, &body);
        }
    }
}

/// RULE, over how many issuances are outstanding: each one stands on its own, so an identity
/// holding several buys an exchange with each of them — in whichever order they are presented
/// — and with each of them again.
#[tokio::test]
async fn every_outstanding_issuance_buys_an_exchange_as_often_as_it_is_presented() {
    let emulator = Emulator::new();
    let pool_id = vending_pool(&emulator, "token-outstanding").await;

    for (row, how) in rows() {
        let identity_id = identity_of(&emulator, &pool_id, how).await;
        let mut outstanding = Vec::new();
        while outstanding.len() < OUTSTANDING_ISSUANCES {
            outstanding.push(mint(&emulator, &pool_id, &identity_id, how).await);
        }
        // Presented youngest first, so the order they were minted in is not what decides
        // them.
        outstanding.reverse();

        let mut vended: Vec<String> = Vec::new();
        for presentation in 1..=PRESENTATIONS {
            for (index, token) in outstanding.iter().enumerate() {
                let case = format!(
                    "{row}, issuance {} of {OUTSTANDING_ISSUANCES}, presentation \
                     {presentation} of {PRESENTATIONS}",
                    index + 1
                );
                let (status, body) = exchange(
                    &emulator,
                    &identity_id,
                    &json!({COGNITO_IDENTITY_PROVIDER: token}),
                    None,
                )
                .await;
                let access_key_id = assert_served(&case, &identity_id, status, &body);
                assert!(
                    !vended.contains(&access_key_id),
                    "case {case:?}: every exchange mints its own session, got \
                     {access_key_id} again"
                );
                vended.push(access_key_id);
            }
        }
    }
}

/// RULE, wherever a presented login is read: a token an exchange was served with is still the
/// login it is at every later reading of one, so the unlink that presents it is served as the
/// exchange was.
#[tokio::test]
async fn an_exchanged_issuance_is_still_read_as_the_login_it_is() {
    let emulator = Emulator::new();
    let pool_id = vending_pool(&emulator, "token-read-elsewhere").await;
    let how = Mint::Federated("fb-access-token");
    let identity_id = identity_of(&emulator, &pool_id, how).await;

    let token = mint(&emulator, &pool_id, &identity_id, how).await;
    let (status, body) = exchange(
        &emulator,
        &identity_id,
        &json!({COGNITO_IDENTITY_PROVIDER: &token}),
        None,
    )
    .await;
    assert_served("the exchange served with the issuance", &identity_id, status, &body);

    // The unlink presenting that same token: it reads a presented login too, so it reads the
    // login the token is and detaches the provider it was asked to.
    let unlink = json!({
        "IdentityId": &identity_id,
        "Logins": {
            COGNITO_IDENTITY_PROVIDER: &token,
            PUBLIC_PROVIDER: "fb-access-token",
        },
        "LoginsToRemove": [PUBLIC_PROVIDER],
    });
    let (status, body) = emulator.call(UNLINK, &unlink).await;
    assert_eq!(
        status, 200,
        "the unlink presenting the exchanged issuance is served, got {status} with {body}"
    );
}
