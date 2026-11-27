// SPDX-License-Identifier: CC0-1.0

//! WHOSE login a token this service minted is.
//!
//! RULE: a token this service mints is minted FOR one identity — "If the token is for
//! cognito-identity.amazonaws.com, it will be passed through to Security Token Service with
//! the appropriate role for the token" (`GetCredentialsForIdentity`, this service's model),
//! and the issuance records which identity that is — so presenting it under this service's
//! own provider name is a login of THAT identity and of no other: a request naming any other
//! identity is refused with the modelled `NotAuthorizedException` (HTTP 400) and vends
//! nothing, and an identity belonging to the pool that minted it, that is also what keeps a
//! token minted in one identity pool out of every other pool's identities. The refusal is
//! answered before any role is resolved and before any session is minted, and it costs the
//! token nothing: presented for the identity it WAS minted for, the same token is still
//! exchanged — whether or not an exchange has been served with it before
//! (`login_token_single_use`'s rule).
//!
//! WHICH refusal it is, is decided by the pool the token is read against — the pool that owns
//! the identity the request names. A token of THAT pool, minted for another of its
//! identities, is one of the pool's tokens and simply not a login of the identity named, so
//! the refusal is the login mismatch; a token ANOTHER pool minted is no Cognito token of this
//! pool at all, and the refusal says that instead.
//!
//! The rows differ along what the rule generalises over: which identity the presented token
//! names against which the request names (the same one, another of the same pool, one of
//! another pool, in both directions), how the token was minted
//! (`GetOpenIdToken` for an unauthenticated identity, `GetOpenIdToken` for a federated one,
//! `GetOpenIdTokenForDeveloperIdentity` for a backend-authenticated one), and whether the
//! `Logins` map presents the token alone or beside a login of another provider.
//!
//! The harness is `identity_pool_flow`'s: one shared engine behind the same
//! `MultiServiceRouter` the production router builds, with the role each vend assumes seeded
//! into the IAM store of that same engine.

use serde_json::{json, Value};

use crate::developer_identity_tokens::{pool_with_developer_provider, token_request};
use crate::identity_pool_flow::{
    cognito_trust_policy, error_code, seeded_role_arn, Emulator,
};

/// The operation the rule is asserted at.
const OPERATION: &str = "GetCredentialsForIdentity";

/// The `Logins` provider name a token minted by THIS service is presented under. Spelled as
/// a literal here so the wire-visible name is pinned by the test, not by the code under
/// test.
const COGNITO_IDENTITY_PROVIDER: &str = "cognito-identity.amazonaws.com";

/// A public login provider, for the row that presents a login of another provider beside
/// the minted token.
const PUBLIC_PROVIDER: &str = "graph.facebook.com";

/// The message the refusal of a request none of whose logins is one of the identity it names
/// carries (`crate::state::UNMATCHED_LOGINS_MESSAGE`) — pinned here as a literal for the
/// same reason. Fixed text: it carries no part taken from the request or from stored state.
const UNMATCHED_LOGINS_MESSAGE: &str = "Logins don't match. Please include at least one \
                                        valid login for this identity or identity pool.";

/// The message the refusal of a token that is no token of the pool the request addresses
/// carries — one ANOTHER identity pool minted (`crate::state::NOT_A_COGNITO_TOKEN_MESSAGE`).
/// Pinned here as a literal for the same reason, and fixed text just as well: it names
/// neither pool, neither identity nor the token.
const NOT_A_COGNITO_TOKEN_MESSAGE: &str = "Invalid login token. Not a Cognito token.";

/// A pool that can both mint tokens and vend credentials: it names a developer provider, and
/// its two roles are seeded into the IAM store of the same engine and configured on it —
/// the shape a pool has to be in for an exchange to be served at all. Answers its
/// `IdentityPoolId`.
async fn vending_pool(emulator: &Emulator, name: &str, developer_provider: &str) -> String {
    let pool_id = pool_with_developer_provider(emulator, name, Some(developer_provider)).await;
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

/// How the token of one row was minted.
enum Mint<'a> {
    /// `GetOpenIdToken` for an identity that presented no login: an unauthenticated one.
    Anonymous,
    /// `GetOpenIdToken` for an identity reached through a login of this public provider.
    Federated(&'a str),
    /// `GetOpenIdTokenForDeveloperIdentity` for this developer user identifier.
    Developer(&'a str),
}

/// Mint one token in `pool_id` and answer the identity it names with the token itself.
async fn mint(
    emulator: &Emulator,
    case: &str,
    pool_id: &str,
    developer_provider: &str,
    how: &Mint<'_>,
) -> (String, String) {
    let answered = match how {
        Mint::Anonymous => {
            let identity_id = emulator.identity(pool_id, None).await;
            emulator
                .ok("GetOpenIdToken", &json!({"IdentityId": identity_id}))
                .await
        }
        Mint::Federated(login) => {
            let logins = json!({PUBLIC_PROVIDER: login});
            let identity_id = emulator.identity(pool_id, Some(logins.clone())).await;
            emulator
                .ok(
                    "GetOpenIdToken",
                    &json!({"IdentityId": identity_id, "Logins": logins}),
                )
                .await
        }
        Mint::Developer(user) => {
            emulator
                .ok(
                    "GetOpenIdTokenForDeveloperIdentity",
                    &token_request(pool_id, &[(developer_provider, user)], None, None),
                )
                .await
        }
    };
    let identity_id = answered["IdentityId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a mint answers an IdentityId: {answered}"))
        .to_string();
    let token = answered["Token"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a mint answers a Token: {answered}"))
        .to_string();
    (identity_id, token)
}

/// The exchange itself: `GetCredentialsForIdentity` for `identity_id`, presenting `logins`.
async fn exchange(emulator: &Emulator, identity_id: &str, logins: &Value) -> (u16, Value) {
    emulator
        .call(
            OPERATION,
            &json!({"IdentityId": identity_id, "Logins": logins}),
        )
        .await
}

/// Assert that the exchange was SERVED: a complete credential set for the identity asked
/// for.
fn assert_served(case: &str, identity_id: &str, status: u16, body: &Value) {
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
}

/// Assert that the exchange was REFUSED for a login that is none of the identity's, with the
/// sentence `message` that refusal is worded with and no credential in the answer.
fn assert_refused(case: &str, status: u16, body: &Value, message: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected the HTTP 400 of a refused exchange, got {status} with {body}"
    );
    let code = error_code(body);
    assert!(
        code.ends_with("NotAuthorizedException"),
        "case {case:?}: expected NotAuthorizedException, got {code:?} in {body}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(message),
        "case {case:?}: expected the fixed sentence {message:?}, got {body}"
    );
    assert!(
        body.get("Credentials").is_none(),
        "case {case:?}: a refused exchange carries no credential, got {body}"
    );
}

/// RULE: the token is a login of the identity its issuance names, so the exchange is served
/// for that identity and refused for every other — in the same pool and in another one.
#[tokio::test]
async fn a_minted_token_is_a_login_of_the_identity_it_was_minted_for_and_of_no_other() {
    let emulator = Emulator::new();
    let developer_provider = "login.example.com";
    let here = vending_pool(&emulator, "token-binding-here", developer_provider).await;
    let there = vending_pool(&emulator, "token-binding-there", developer_provider).await;

    // One identity per way a token is minted, in each of the two pools, so every row below
    // differs only in WHOSE token is presented for WHICH identity.
    let minted: Vec<(&str, &str, Mint<'_>)> = vec![
        ("an anonymous identity of this pool", &here, Mint::Anonymous),
        (
            "a federated identity of this pool",
            &here,
            Mint::Federated("federated-user-here"),
        ),
        (
            "a developer-authenticated identity of this pool",
            &here,
            Mint::Developer("backend-user-here"),
        ),
        (
            "an anonymous identity of the other pool",
            &there,
            Mint::Anonymous,
        ),
        (
            "a developer-authenticated identity of the other pool",
            &there,
            Mint::Developer("backend-user-there"),
        ),
    ];
    let mut tokens: Vec<(&str, &str, String, String)> = Vec::new();
    for (case, pool_id, how) in &minted {
        let (identity_id, token) =
            mint(&emulator, case, pool_id, developer_provider, how).await;
        tokens.push((case, pool_id, identity_id, token));
    }

    // ANOTHER identity's token is refused for it — every pair of the table, which spans
    // two identities of one pool as well as two of different pools. Which sentence the
    // refusal carries follows the pool the token is read against: a token of the addressed
    // pool is a token of it that does not match the identity, and a token of any other pool
    // is no token of the addressed pool at all.
    for (identity_case, identity_pool, identity_id, _) in &tokens {
        for (token_case, token_pool, token_identity, token) in &tokens {
            if token_identity == identity_id {
                continue;
            }
            let case = format!("{identity_case:?} presenting the token of {token_case:?}");
            let expected = if token_pool == identity_pool {
                UNMATCHED_LOGINS_MESSAGE
            } else {
                NOT_A_COGNITO_TOKEN_MESSAGE
            };
            let logins = json!({COGNITO_IDENTITY_PROVIDER: token});
            let (status, body) = exchange(&emulator, identity_id, &logins).await;
            assert_refused(&case, status, &body, expected);
        }
    }

    // The refusals cost the tokens nothing: each is still the login of its own identity, so
    // its OWN token is exchanged for every identity, whichever way it was minted and
    // whichever pool it belongs to.
    for (case, _, identity_id, token) in &tokens {
        let logins = json!({COGNITO_IDENTITY_PROVIDER: token});
        let (status, body) = exchange(&emulator, identity_id, &logins).await;
        assert_served(
            &format!("{case:?}, after the refusals"),
            identity_id,
            status,
            &body,
        );
    }
}

/// RULE, over the shape of the `Logins` map: the token is decided wherever it is presented,
/// so a map carrying another identity's token BESIDE a login of another provider is refused
/// just the same — and the same map carrying the identity's own token is served.
#[tokio::test]
async fn another_identitys_token_is_refused_whatever_else_the_logins_map_carries() {
    let emulator = Emulator::new();
    let developer_provider = "login.example.com";
    let pool_id = vending_pool(&emulator, "token-binding-map", developer_provider).await;
    let other_pool_id = vending_pool(&emulator, "token-binding-map-other", developer_provider)
        .await;

    let (identity_id, own_token) = mint(
        &emulator,
        "the identity under test",
        &pool_id,
        developer_provider,
        &Mint::Federated("map-user"),
    )
    .await;
    // A second issuance of the SAME identity, so each served row presents its own token and
    // no row is read against what another did.
    let (again_identity, own_token_again) = mint(
        &emulator,
        "the identity under test, minted again",
        &pool_id,
        developer_provider,
        &Mint::Federated("map-user"),
    )
    .await;
    assert_eq!(
        again_identity, identity_id,
        "one federated login is one identity, got {again_identity} against {identity_id}"
    );
    let (_, sibling_token) = mint(
        &emulator,
        "another identity of the same pool",
        &pool_id,
        developer_provider,
        &Mint::Developer("map-backend-user"),
    )
    .await;
    let (_, foreign_token) = mint(
        &emulator,
        "an identity of another pool",
        &other_pool_id,
        developer_provider,
        &Mint::Anonymous,
    )
    .await;

    // The rows differ along which token the map carries under this service's provider name
    // and what else it carries beside it; the third field is the refusal owed, and `None` a
    // served exchange.
    let rows: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "its own token alone",
            json!({COGNITO_IDENTITY_PROVIDER: &own_token}),
            None,
        ),
        (
            "its own token beside a login of the public provider",
            json!({
                COGNITO_IDENTITY_PROVIDER: &own_token_again,
                PUBLIC_PROVIDER: "map-user",
            }),
            None,
        ),
        (
            "a sibling identity's token alone",
            json!({COGNITO_IDENTITY_PROVIDER: &sibling_token}),
            Some(UNMATCHED_LOGINS_MESSAGE),
        ),
        (
            "a sibling identity's token beside a login of the public provider",
            json!({
                COGNITO_IDENTITY_PROVIDER: &sibling_token,
                PUBLIC_PROVIDER: "map-user",
            }),
            Some(UNMATCHED_LOGINS_MESSAGE),
        ),
        (
            "another pool's token alone",
            json!({COGNITO_IDENTITY_PROVIDER: &foreign_token}),
            Some(NOT_A_COGNITO_TOKEN_MESSAGE),
        ),
        (
            "another pool's token beside a login of the public provider",
            json!({
                COGNITO_IDENTITY_PROVIDER: &foreign_token,
                PUBLIC_PROVIDER: "map-user",
            }),
            Some(NOT_A_COGNITO_TOKEN_MESSAGE),
        ),
    ];

    for (case, logins, refusal) in &rows {
        let (status, body) = exchange(&emulator, &identity_id, logins).await;
        match refusal {
            None => assert_served(case, &identity_id, status, &body),
            Some(message) => assert_refused(case, status, &body, message),
        }
    }
}
