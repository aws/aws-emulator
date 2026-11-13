// SPDX-License-Identifier: CC0-1.0

//! The validity WINDOW of an OpenID Connect token this service minted, at the exchange
//! that spends it.
//!
//! RULE: a token this service mints is a time-bounded credential — its window is the one
//! the issuance recorded (`TokenDuration` seconds, or the documented 15 minutes for a
//! request naming none) — so presenting it to `GetCredentialsForIdentity` under this
//! service's own provider name buys a credential only INSIDE that window: once the clock
//! has closed it the exchange is refused with the modelled `NotAuthorizedException`
//! (HTTP 400) and no credential, and a token this service cannot resolve at all buys
//! nothing either. The refusal is answered before any role is resolved and before any
//! session is minted or handed back, so a session the identity already holds does not
//! outlive the token it was vended for.
//!
//! Every case asserts that rule over a TABLE of windows — the modelled minimum, windows
//! the wait below also closes, the window of a request that names none, and the modelled
//! maximum — rather than one recorded call. The harness is `identity_pool_flow`'s: one
//! shared engine behind the same `MultiServiceRouter` the production router builds, with
//! the role a vend assumes seeded into the IAM store of that same engine.

// `std::thread::sleep`, not `tokio::time::sleep`: the workspace pins tokio without the
// `time` feature, and the wait has to pass on the same wall clock the handler reads
// (`SystemTime::now`) in any case.
use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};

use crate::developer_identity_tokens::{pool_with_developer_provider, token_request};
use crate::identity_pool_flow::{cognito_trust_policy, error_code, seeded_role_arn, Emulator};

/// The operation the window is enforced at.
const OPERATION: &str = "GetCredentialsForIdentity";

/// The operation that mints the token, and so records its window.
const MINT: &str = "GetOpenIdTokenForDeveloperIdentity";

/// The `Logins` provider name a token minted by THIS service is presented under: "If the
/// token is for cognito-identity.amazonaws.com, it will be passed through to Security Token
/// Service with the appropriate role for the token"
/// (`smithy_query(query="doc",shape="GetCredentialsForIdentityInput")`). Spelled as a
/// literal here so the wire-visible name is pinned by the test, not by the code under test.
const COGNITO_IDENTITY_PROVIDER: &str = "cognito-identity.amazonaws.com";

/// The message the refusal of a token whose window has closed carries
/// (`crate::state::EXPIRED_LOGIN_TOKEN_MESSAGE`) — pinned here as a literal for the same
/// reason. Fixed text: it carries no part taken from the request or from stored state.
const EXPIRED_MESSAGE: &str = "Invalid login token. Token is expired.";

/// The message the refusal of a value of this service's OWN token form that it cannot
/// resolve carries (`crate::state::UNVERIFIABLE_LOGIN_TOKEN_MESSAGE`).
const UNVERIFIABLE_MESSAGE: &str = "Invalid login token. Couldn't verify signed token.";

/// The message the refusal of a token this service cannot resolve at all carries, when the
/// value does not even claim this service as its issuer
/// (`crate::state::NOT_A_COGNITO_TOKEN_MESSAGE`).
const NOT_A_COGNITO_TOKEN_MESSAGE: &str = "Invalid login token. Not a Cognito token.";

/// The modelled bounds of `TokenDuration` (`long range 1..86400`,
/// `smithy_query(query="operation",operation="GetOpenIdTokenForDeveloperIdentity")`): the
/// shortest and the longest window a mint can ask for.
const MIN_TOKEN_SECONDS: i64 = 1;
const MAX_TOKEN_SECONDS: i64 = 86_400;

/// The longest window this suite waits out, in seconds, and the wait itself: past that
/// window, with a margin for a loaded test host. Only windows SHORTER than the wait are
/// expected to have closed when it is over.
const SHORT_WINDOW_SECONDS: i64 = 2;
const PAST_THE_SHORT_WINDOWS: Duration = Duration::from_millis(2_500);

/// A pool that names a developer provider AND can vend credentials: its `authenticated`
/// role is seeded into the IAM store of the same engine and configured on the pool, which
/// is the shape a pool has to be in for an exchange to be served at all. Answers its
/// `IdentityPoolId`.
async fn vending_pool(emulator: &Emulator, name: &str, developer_provider: &str) -> String {
    let pool_id = pool_with_developer_provider(emulator, name, Some(developer_provider)).await;
    let authenticated =
        emulator.seed_role(&seeded_role_arn(name, "auth"), cognito_trust_policy(&pool_id));
    emulator
        .ok(
            "SetIdentityPoolRoles",
            &json!({
                "IdentityPoolId": &pool_id,
                "Roles": {"authenticated": authenticated},
            }),
        )
        .await;
    pool_id
}

/// Mint a token for `user` in `pool_id` with the window `token_duration` asks for (none
/// when it is `None`), and answer the identity it resolved to with the token itself.
async fn mint(
    emulator: &Emulator,
    pool_id: &str,
    developer_provider: &str,
    user: &str,
    token_duration: Option<i64>,
) -> (String, String) {
    let answered = emulator
        .ok(
            MINT,
            &token_request(
                pool_id,
                &[(developer_provider, user)],
                None,
                token_duration,
            ),
        )
        .await;
    let identity_id = answered["IdentityId"]
        .as_str()
        .unwrap_or_else(|| panic!("{MINT} answers an IdentityId: {answered}"))
        .to_string();
    let token = answered["Token"]
        .as_str()
        .unwrap_or_else(|| panic!("{MINT} answers a Token: {answered}"))
        .to_string();
    (identity_id, token)
}

/// The exchange itself: `GetCredentialsForIdentity` presenting `token` under this service's
/// own provider name, as a client spends a minted token.
async fn exchange(emulator: &Emulator, identity_id: &str, token: &str) -> (u16, Value) {
    emulator
        .call(
            OPERATION,
            &json!({
                "IdentityId": identity_id,
                "Logins": {COGNITO_IDENTITY_PROVIDER: token},
            }),
        )
        .await
}

/// The `message` an error response carries.
fn error_message(case: &str, body: &Value) -> String {
    body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: an error response carries a message: {body}"))
        .to_string()
}

/// Assert that `body` is a served exchange: a complete credential set for `identity_id`.
fn assert_served_credentials(case: &str, identity_id: &str, body: &Value) {
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

/// Assert that `body` is the refusal `message` words, with no credential in it.
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
    let actual = error_message(case, body);
    assert_eq!(
        actual, message,
        "case {case:?}: expected the refusal message {message:?}, got {actual:?}"
    );
    assert!(
        body.get("Credentials").is_none(),
        "case {case:?}: a refused exchange carries no credential, got {body}"
    );
}

/// RULE: the window a mint records is the window the exchange honours — inside it the
/// token is spent for a credential, past it the exchange is refused.
#[tokio::test]
async fn a_minted_token_is_exchanged_only_inside_the_window_its_issuance_recorded() {
    let emulator = Emulator::new();
    let developer_provider = "login.example.com";
    let pool_id = vending_pool(&emulator, "token-window", developer_provider).await;
    // One developer user identifier for every row, so re-minting also has to keep
    // answering the SAME identity.
    let user = "backend-user-1";

    // The rows differ along the window the mint asks for: each modelled bound, a window the
    // wait below also closes, the window of a request that names none, and one well past
    // the wait. The third field says whether the window is still open after the wait; the
    // fourth says whether the row asserts the SERVED half too — the one-second row does
    // not, because a mint and an exchange over the wire can take longer than that window on
    // a loaded host, which would fail the test for its timing rather than for the rule.
    let rows: Vec<(&str, Option<i64>, bool, bool)> = vec![
        (
            "the modelled minimum window of one second",
            Some(MIN_TOKEN_SECONDS),
            false,
            false,
        ),
        (
            "a window of two seconds",
            Some(SHORT_WINDOW_SECONDS),
            false,
            true,
        ),
        (
            "no TokenDuration at all: the documented 15-minute default",
            None,
            true,
            true,
        ),
        ("a window of one hour", Some(3_600), true, true),
        (
            "the modelled maximum window of 86400 seconds",
            Some(MAX_TOKEN_SECONDS),
            true,
            true,
        ),
    ];

    let mut spent: Vec<(&str, String, bool)> = Vec::new();
    let mut identity: Option<String> = None;
    for (case, token_duration, open_after_the_wait, assert_served) in rows {
        let (identity_id, token) =
            mint(&emulator, &pool_id, developer_provider, user, token_duration).await;
        match &identity {
            Some(first) => assert_eq!(
                &identity_id, first,
                "case {case:?}: re-minting for one developer user identifier answers the same \
                 identity, got {identity_id} after {first}"
            ),
            None => identity = Some(identity_id.clone()),
        }

        // Inside its window, whatever window was asked for, the token is spent.
        if assert_served {
            let (status, body) = exchange(&emulator, &identity_id, &token).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a token inside its window is exchanged, got {status} with {body}"
            );
            assert_served_credentials(case, &identity_id, &body);
        }
        spent.push((case, token, open_after_the_wait));
    }

    // Nothing is minted and nothing is configured from here on, so the only thing that
    // differs between the two halves of every row is the clock.
    sleep(PAST_THE_SHORT_WINDOWS);
    let identity_id = identity.expect("the table has rows");
    for (case, token, open_after_the_wait) in spent {
        let (status, body) = exchange(&emulator, &identity_id, &token).await;
        if open_after_the_wait {
            assert_eq!(
                status, 200,
                "case {case:?}: this window is still open, so the exchange is still served, \
                 got {status} with {body}"
            );
            assert_served_credentials(case, &identity_id, &body);
        } else {
            assert_refused(case, status, &body, EXPIRED_MESSAGE);
        }
    }
}

/// RULE, the same window from the other side: the answer does not depend on whether the
/// issuance store still HOLDS the record — a token whose window has closed is refused as
/// the expired token it is even after a later mint has swept that record away.
#[tokio::test]
async fn a_token_whose_record_has_been_swept_is_still_refused_as_expired() {
    let emulator = Emulator::new();
    let developer_provider = "login.example.com";
    let pool_id = vending_pool(&emulator, "token-sweep", developer_provider).await;
    let user = "backend-user-2";

    // A two-second window, so the exchange made right after the mint is inside it even on
    // a loaded host, while the wait below still closes it.
    let (identity_id, short_lived) = mint(
        &emulator,
        &pool_id,
        developer_provider,
        user,
        Some(SHORT_WINDOW_SECONDS),
    )
    .await;
    let (status, body) = exchange(&emulator, &identity_id, &short_lived).await;
    assert_eq!(
        status, 200,
        "a token inside its window is exchanged, got {status} with {body}"
    );

    sleep(PAST_THE_SHORT_WINDOWS);
    // A later mint is what sweeps the issuances the clock has expired, so this is the call
    // that makes the first token's record disappear.
    let (_, fresh) = mint(
        &emulator,
        &pool_id,
        developer_provider,
        user,
        Some(MAX_TOKEN_SECONDS),
    )
    .await;

    let (status, body) = exchange(&emulator, &identity_id, &short_lived).await;
    assert_refused(
        "the swept token, past its window",
        status,
        &body,
        EXPIRED_MESSAGE,
    );
    // The window of the token minted afterwards is its own, so the sweep costs it nothing.
    let (status, body) = exchange(&emulator, &identity_id, &fresh).await;
    assert_eq!(
        status, 200,
        "the freshly minted token is inside its own window, got {status} with {body}"
    );
    assert_served_credentials("the freshly minted token", &identity_id, &body);
}

/// A compact JWS carrying `claims`, with the empty signature segment RFC 7515 §6 defines
/// for an unsecured one — the form this service's tokens take, so a row below can present a
/// value of that form that no mint ever produced.
fn unsecured_jws(claims: &Value) -> String {
    let segment = |bytes: &[u8]| {
        aws_smithy_types::base64::encode(bytes)
            .replace('+', "-")
            .replace('/', "_")
            .trim_end_matches('=')
            .to_string()
    };
    format!(
        "{}.{}.",
        segment(json!({"alg": "none", "typ": "JWT"}).to_string().as_bytes()),
        segment(claims.to_string().as_bytes())
    )
}

/// RULE, the resolution half: a value presented under this service's own provider name has
/// to be a token this service issued, so one it never issued vouches for nothing and buys
/// no credential — and WHICH refusal it carries follows from what the value itself claims to
/// be: a value of this service's own token form is the unverifiable token it claims to be,
/// while a value that claims this service as its issuer in no way is no Cognito token at
/// all.
#[tokio::test]
async fn a_token_this_service_never_issued_buys_no_credential() {
    let emulator = Emulator::new();
    let developer_provider = "login.example.com";
    let pool_id = vending_pool(&emulator, "token-unknown", developer_provider).await;
    // The identity exists and holds a credential from a token that WAS issued, so each row
    // below is refused on the token it presents and not on anything else.
    let (identity_id, issued) = mint(
        &emulator,
        &pool_id,
        developer_provider,
        "backend-user-3",
        Some(MAX_TOKEN_SECONDS),
    )
    .await;
    let (status, body) = exchange(&emulator, &identity_id, &issued).await;
    assert_eq!(
        status, 200,
        "the issued token is exchanged, got {status} with {body}"
    );

    // An instant far ahead of any run of this suite, so no row below is refused for its
    // window rather than for being unknown.
    let far_future = 4_102_444_800u64;
    // The rows differ along the FORM of the value presented: none of them is a token this
    // service minted, and the third field is the sentence that form is owed.
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "a value that is no token at all",
            "not-a-token".to_string(),
            NOT_A_COGNITO_TOKEN_MESSAGE,
        ),
        (
            "a JWS whose payload is not a claim set",
            unsecured_jws(&json!("a string, not an object")),
            NOT_A_COGNITO_TOKEN_MESSAGE,
        ),
        (
            "a JWS naming another issuer",
            unsecured_jws(&json!({
                "iss": "https://accounts.google.com",
                "aud": &pool_id,
                "sub": &identity_id,
                "exp": far_future,
            })),
            NOT_A_COGNITO_TOKEN_MESSAGE,
        ),
        (
            "a JWS of this service's own form that no mint produced",
            unsecured_jws(&json!({
                "iss": "https://cognito-identity.amazonaws.com",
                "aud": &pool_id,
                "sub": &identity_id,
                "exp": far_future,
            })),
            UNVERIFIABLE_MESSAGE,
        ),
    ];

    for (case, token, owed) in rows {
        let (status, body) = exchange(&emulator, &identity_id, &token).await;
        assert_refused(case, status, &body, owed);
    }

    // The token that WAS issued is untouched by any of it: the refusals decided on the
    // value presented, nothing else.
    let (status, body) = exchange(&emulator, &identity_id, &issued).await;
    assert_eq!(
        status, 200,
        "the issued token is still exchanged, got {status} with {body}"
    );
    assert_served_credentials("the issued token", &identity_id, &body);
}
