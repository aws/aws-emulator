// SPDX-License-Identifier: CC0-1.0

//! WHICH refusal a token this service minted carries once its validity window has CLOSED,
//! including after exchanges have been served with it.
//!
//! RULE: a token this service mints is a time-bounded bearer credential of one identity, and
//! presenting it READS the issuance rather than consuming it — the mint hands its caller a
//! credential to keep for the window it asked for, "You can specify a custom expiration time
//! for the token so that you can cache it"
//! (`smithy_query(query="doc",shape="GetOpenIdTokenForDeveloperIdentityInput",member="TokenDuration")`),
//! and a cached token is a token presented more than once. So every presentation inside that
//! window (plus the clock-skew tolerance the reading adds) is served, and the presentation
//! that follows the CLOSE of the window is refused the modelled `NotAuthorizedException`
//! with AWS's expiry sentence `Invalid login token. Token is expired.` — the sentence that
//! tells a caller its token aged out and a fresh mint repairs it — whatever the caller had
//! already exchanged that token for. AWS's other sentence,
//! `Invalid login token. Not a Cognito token.`, stays reserved for a value this pool never
//! minted at all, so the two refusals keep telling a stale token apart from a login value of
//! the wrong kind.
//!
//! The rows span what the rule generalises over: the window the mint asked for (both modelled
//! bounds of `TokenDuration`, the window of a request naming none, and one in between) and
//! how many exchanges were served with the token before its window closed (none, one,
//! several).
//!
//! The window is closed by AGEING the issuance record this service holds, never by waiting
//! for wall-clock time to pass: a window of hours cannot be waited out, and an outcome that
//! depends on how fast the host runs is no assertion at all. The harness is
//! `login_token_window`'s, the suite that drives that same record.

use std::time::Duration;

use crate::login_token_window::{assert_refused, assert_served_credentials, Emulator};

/// AWS's fixed sentences, spelled as literals so the wire-visible text is pinned by the test
/// rather than by the code under test: each is fixed text, with no part taken from the request
/// or from stored state.
const EXPIRED_MESSAGE: &str = "Invalid login token. Token is expired.";
const NOT_A_COGNITO_TOKEN_MESSAGE: &str = "Invalid login token. Not a Cognito token.";

/// The modelled bounds of `TokenDuration` (`long range 1..86400`,
/// `smithy_query(query="operation",operation="GetOpenIdTokenForDeveloperIdentity")`): the
/// shortest and the longest window a mint can ask for.
const MIN_TOKEN_SECONDS: i64 = 1;
const MAX_TOKEN_SECONDS: i64 = 86_400;

/// The window of a mint that names no `TokenDuration`: "If you don't provide an expiration
/// time, the token is valid for 15 minutes." (that member's own documentation).
const DEFAULT_TOKEN_SECONDS: i64 = 900;

/// How far past the instant its issuance recorded a presented token is still read as valid:
/// the longest grace period the `TokenDuration` documentation asks a caller to allow for,
/// "usually no more than 5 minutes, to account for clock skew"
/// (`crate::state::PRESENTED_TOKEN_CLOCK_SKEW_GRACE`). Spelled as a literal for the same
/// reason, so a row can age an issuance past the whole of the window it is read against.
const CLOCK_SKEW_GRACE: Duration = Duration::from_secs(300);

/// The developer provider the fixture pool's backend logins are made under, and the user it
/// authenticates.
const DEVELOPER_PROVIDER: &str = "login.example.backend";
const DEVELOPER_USER: &str = "backend-user";

/// How many exchanges a row serves with its token before the window closes: none, one, and
/// more than one, so "several" is not "the second".
const SERVED_BEFORE: [usize; 3] = [0, 1, 3];

/// The window one row's mint asks for: how the case reads, what it sends as `TokenDuration`,
/// and the nominal window in seconds it is owed.
fn windows() -> Vec<(&'static str, Option<i64>, i64)> {
    vec![
        (
            "the modelled minimum window of one second",
            Some(MIN_TOKEN_SECONDS),
            MIN_TOKEN_SECONDS,
        ),
        ("a window of two seconds", Some(2), 2),
        (
            "no TokenDuration at all: the documented 15-minute default",
            None,
            DEFAULT_TOKEN_SECONDS,
        ),
        (
            "the modelled maximum window of 86400 seconds",
            Some(MAX_TOKEN_SECONDS),
            MAX_TOKEN_SECONDS,
        ),
    ]
}

/// How far an issuance of a nominal window of `nominal` seconds has to be aged for the whole
/// window it is read against — that window plus the clock-skew tolerance — to be closed.
fn past_the_window(nominal: i64) -> Duration {
    Duration::from_secs(u64::try_from(nominal).unwrap_or_default())
        + CLOCK_SKEW_GRACE
        + Duration::from_secs(1)
}

/// RULE: every presentation inside the window is served with a session of its own, and the
/// presentation after the window has closed is refused as the EXPIRED token it is — however
/// many exchanges were served with that token first, and whatever window its mint asked for.
#[tokio::test]
async fn a_token_already_exchanged_is_refused_as_expired_once_its_window_closes() {
    let emulator = Emulator::new();
    let pool_id = emulator
        .vending_pool("token-reuse-expiry", DEVELOPER_PROVIDER)
        .await;

    for (window, token_duration, nominal) in windows() {
        for served in SERVED_BEFORE {
            let case = format!("{window}, {served} exchange(s) served first");
            // One issuance per row, of that row's window: what is read here is the window of
            // the token presented and nothing another row did.
            let (identity_id, token) = emulator
                .mint(&pool_id, DEVELOPER_PROVIDER, DEVELOPER_USER, token_duration)
                .await;

            let mut vended: Vec<String> = Vec::new();
            for presentation in 1..=served {
                let case = format!("{case}, presentation {presentation} of {served}");
                let (status, body) = emulator.exchange(&identity_id, &token).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: a token inside its window is exchanged however often it \
                     is presented, got {status} with {body}"
                );
                assert_served_credentials(&case, &identity_id, &body);
                let access_key_id = body["Credentials"]["AccessKeyId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                assert!(
                    !vended.contains(&access_key_id),
                    "case {case:?}: every exchange mints its own session, got \
                     {access_key_id} again"
                );
                vended.push(access_key_id);
            }

            // Nothing about the token changes but the window it is read against: the record
            // is left as it would be had it been minted long enough ago for the whole of
            // that window to have elapsed.
            emulator.age_issuance(&case, &token, past_the_window(nominal));
            let (status, body) = emulator.exchange(&identity_id, &token).await;
            assert_refused(&case, status, &body, EXPIRED_MESSAGE);
        }
    }
}

/// RULE, the distinction the two sentences carry: the expiry sentence is the AGED token's,
/// and the not-a-Cognito-token sentence stays the unminted value's — read at one identity, in
/// one pool, at the same instant, so nothing but the value presented differs between them.
#[tokio::test]
async fn the_expiry_sentence_is_the_aged_tokens_and_the_other_stays_the_unminted_values() {
    let emulator = Emulator::new();
    let pool_id = emulator
        .vending_pool("token-refusal-sentences", DEVELOPER_PROVIDER)
        .await;
    let (identity_id, aged) = emulator
        .mint(
            &pool_id,
            DEVELOPER_PROVIDER,
            DEVELOPER_USER,
            Some(MAX_TOKEN_SECONDS),
        )
        .await;

    // The token is exchanged first, so the refusal below is its WINDOW's doing and not the
    // exchange's.
    let (status, body) = emulator.exchange(&identity_id, &aged).await;
    assert_eq!(
        status, 200,
        "the minted token is exchanged inside its window, got {status} with {body}"
    );
    assert_served_credentials("the first exchange", &identity_id, &body);

    // The values that are no token of this pool at all: an opaque credential of some other
    // provider, and the developer user IDENTIFIER the mint takes where a token has to be.
    for (case, value) in [
        ("an opaque credential of some other provider", "not-a-token"),
        ("a developer user identifier", DEVELOPER_USER),
    ] {
        let (status, body) = emulator.exchange(&identity_id, value).await;
        assert_refused(case, status, &body, NOT_A_COGNITO_TOKEN_MESSAGE);
    }

    // The pool's OWN token, once the clock has closed the window it is read against.
    emulator.age_issuance(
        "the aged token",
        &aged,
        past_the_window(MAX_TOKEN_SECONDS),
    );
    let (status, body) = emulator.exchange(&identity_id, &aged).await;
    assert_refused("the aged token", status, &body, EXPIRED_MESSAGE);

    // The control: a FRESH issuance of the same identity is exchanged as ever, so each
    // refusal above was the value presented and nothing about the identity or the pool.
    let (_, fresh) = emulator
        .mint(
            &pool_id,
            DEVELOPER_PROVIDER,
            DEVELOPER_USER,
            Some(MAX_TOKEN_SECONDS),
        )
        .await;
    let (status, body) = emulator.exchange(&identity_id, &fresh).await;
    assert_eq!(
        status, 200,
        "a fresh issuance of the same identity is exchanged, got {status} with {body}"
    );
    assert_served_credentials("the fresh issuance", &identity_id, &body);
}
