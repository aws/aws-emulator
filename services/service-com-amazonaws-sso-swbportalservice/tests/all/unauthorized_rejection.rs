// SPDX-License-Identifier: CC0-1.0

//! The portal's authentication gate, end to end over HTTP.
//!
//! RULE: a presented bearer token that names no usable sign-in session is refused with
//! HTTP 401 `UnauthorizedException` carrying ONE constant message — the same on every
//! bound operation, for every class of unusable token, and whatever else the request
//! carries. The gate derives nothing from the request, so the refusal body carries the
//! code and that message and no payload member.
//!
//! Each test sweeps that rule over a table: the operations × the token classes × the
//! rest of the request. The recorded case (one bogus token on one operation) is one row.

use super::portal_session::{body_json, body_text, error_code, error_message, roles_of, Portal};
use bytes::Bytes;
use http::{Response, StatusCode};
use service_com_amazonaws_sso_swbportalservice::state::INVALID_SESSION_MESSAGE;
use std::time::Duration;

/// The message the IAM Identity Center Portal API answers an unusable bearer token
/// with, recorded on all four bound operations and every class of token the behavioral
/// suite presents. Pinned here as a literal, independently of the constant the service
/// holds, so a change to either side fails this suite.
const EXPECTED_MESSAGE: &str = "Session token not found or invalid";

/// An account id and a role name [`Portal::new`]'s fixture really holds, so a row that
/// is refused is refused by the gate and not by the absence of the resource.
const KNOWN_ACCOUNT: &str = "111111111111";
const KNOWN_ROLE: &str = "AdministratorAccess";

/// One request to a bound operation: the route and method come from `OPERATIONS.md`
/// (`GET /assignment/accounts`, `GET /assignment/roles`, `GET /federation/credentials`,
/// `POST /logout`).
enum Call {
    Get(String),
    Logout,
}

async fn send(portal: &Portal, call: &Call, token: &str) -> Response<Bytes> {
    match call {
        Call::Get(uri) => portal.get(uri, Some(token)).await,
        Call::Logout => portal.logout(Some(token)).await,
    }
}

/// One row per bound operation, plus rows that vary everything else the request can
/// carry: the modeled `max_result` bounds (1 and 100 — the `MaxResultType` range in
/// `OPERATIONS.md`), a `next_token` this service never issued, an account id no
/// assignment names, and query values that are unicode or injection-like
/// (percent-encoded here, since a URI carries no raw space or non-ASCII byte).
fn calls() -> Vec<(&'static str, Call)> {
    vec![
        (
            "ListAccounts",
            Call::Get("/assignment/accounts".to_string()),
        ),
        (
            "ListAccounts max_result=1 (the modeled minimum)",
            Call::Get("/assignment/accounts?max_result=1".to_string()),
        ),
        (
            "ListAccounts max_result=100 (the modeled maximum)",
            Call::Get("/assignment/accounts?max_result=100".to_string()),
        ),
        (
            "ListAccounts with a next_token this service never issued",
            Call::Get("/assignment/accounts?next_token=not-a-cursor".to_string()),
        ),
        (
            "ListAccountRoles",
            Call::Get(format!("/assignment/roles?account_id={KNOWN_ACCOUNT}")),
        ),
        (
            "ListAccountRoles with a malformed next_token and a page bound",
            Call::Get(format!(
                "/assignment/roles?account_id={KNOWN_ACCOUNT}&max_result=100&next_token=zz-not-hex"
            )),
        ),
        (
            "ListAccountRoles for an account no assignment names",
            Call::Get("/assignment/roles?account_id=999999999999".to_string()),
        ),
        (
            // "' OR '1'='1", percent-encoded.
            "ListAccountRoles with an injection-like account_id",
            Call::Get(
                "/assignment/roles?account_id=%27%20OR%20%271%27%3D%271".to_string(),
            ),
        ),
        (
            "GetRoleCredentials",
            Call::Get(format!(
                "/federation/credentials?role_name={KNOWN_ROLE}&account_id={KNOWN_ACCOUNT}"
            )),
        ),
        (
            // "роль" (U+0440 U+043E U+043B U+044C), percent-encoded UTF-8.
            "GetRoleCredentials with a unicode role_name",
            Call::Get(format!(
                "/federation/credentials?role_name=%D1%80%D0%BE%D0%BB%D1%8C&account_id={KNOWN_ACCOUNT}"
            )),
        ),
        (
            // "<script>alert(1)</script>", percent-encoded.
            "GetRoleCredentials with an injection-like role_name",
            Call::Get(format!(
                "/federation/credentials?role_name=%3Cscript%3Ealert%281%29%3C%2Fscript%3E&account_id={KNOWN_ACCOUNT}"
            )),
        ),
        (
            "GetRoleCredentials for an account no assignment names",
            Call::Get(format!(
                "/federation/credentials?role_name={KNOWN_ROLE}&account_id=999999999999"
            )),
        ),
        ("Logout", Call::Logout),
    ]
}

/// One row per class of token a client can present that names no session: opaque,
/// empty, whitespace, token-shaped (JWT, base64url), oversized, and punctuation-heavy.
///
/// Every value is visible ASCII because an HTTP header value is: a non-ASCII token
/// never reaches the service, so the unicode dimension is exercised on a query value
/// in [`calls`] instead.
fn unusable_tokens() -> Vec<(&'static str, String)> {
    vec![
        ("an opaque unknown token", "sim2real-bogus-abc".to_string()),
        ("the empty string", String::new()),
        ("one space", " ".to_string()),
        ("whitespace only", "    ".to_string()),
        (
            "a JWT-shaped token",
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJhZG1pbiJ9.c2lnbmF0dXJl".to_string(),
        ),
        (
            "a base64url-shaped token",
            "QUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQQ".to_string(),
        ),
        ("an oversized token", "A".repeat(4096)),
        ("a punctuation-heavy token", "../../'; DROP\"|{}".to_string()),
    ]
}

/// RULE: every (operation, token class, rest-of-request) combination that names no
/// usable session answers 401 `UnauthorizedException` with the one constant message,
/// and nothing else.
#[tokio::test]
async fn every_unusable_token_is_refused_with_one_constant_message() {
    // The service holds the constant in exactly one place; this suite pins its value.
    assert_eq!(
        INVALID_SESSION_MESSAGE, EXPECTED_MESSAGE,
        "the service's single rejection constant must be the message the Portal API \
         answers, got {INVALID_SESSION_MESSAGE:?}"
    );

    let portal = Portal::new();
    // A live session exists throughout, so a refusal below is about the token presented
    // and not about a portal with nothing in it.
    portal.sign_in("tok-live", &[KNOWN_ACCOUNT]);

    for (operation, call) in calls() {
        for (class, token) in unusable_tokens() {
            let case = format!("{operation} with {class}");
            let response = send(&portal, &call, &token).await;
            assert_eq!(
                (response.status(), error_code(&response).as_str()),
                (StatusCode::UNAUTHORIZED, "UnauthorizedException"),
                "case {case:?}: expected 401 UnauthorizedException, got {} {:?}",
                response.status(),
                body_text(&response)
            );
            assert_eq!(
                error_message(&response),
                EXPECTED_MESSAGE,
                "case {case:?}: expected the constant {EXPECTED_MESSAGE:?}, got {:?}",
                error_message(&response)
            );
            // Nothing of the request may reach the message: a token long enough not to
            // occur in ordinary prose would show up here if it did. (A one-character or
            // whitespace token is skipped: it is a substring of almost any sentence.)
            assert!(
                token.trim().len() < 8 || !error_message(&response).contains(token.as_str()),
                "case {case:?}: the message must not quote the presented token, got {:?}",
                error_message(&response)
            );
            // The refusal body is the envelope and nothing else — no listing, no
            // credential, no page cursor.
            let body = body_json(&response);
            let members: Vec<&String> = body
                .as_object()
                .map(|object| object.keys().collect())
                .unwrap_or_default();
            assert!(
                members.len() == 2
                    && members.iter().any(|key| key.as_str() == "__type")
                    && members.iter().any(|key| key.as_str() == "message"),
                "case {case:?}: the refusal body must carry only __type and message, got \
                 {members:?}"
            );
        }
    }

    // The gate refused the live session's token nowhere: a read never spends it.
    let served = portal.get("/assignment/accounts", Some("tok-live")).await;
    assert_eq!(
        served.status(),
        StatusCode::OK,
        "case \"a live session after every refusal\": expected 200, got {} {:?}",
        served.status(),
        body_text(&served)
    );
}

/// RULE: a session that STOPPED being usable — logged out, or expired — is refused with
/// the same constant as an unknown token, on every operation, however often it is
/// presented.
#[tokio::test]
async fn a_session_that_stopped_being_usable_is_refused_with_the_same_constant() {
    let portal = Portal::new();
    portal.sign_in("tok-logged-out", &[KNOWN_ACCOUNT]);
    // `put_session` dates the expiry from now, so a zero ttl is already past.
    portal.state().lock().put_session(
        "tok-expired",
        "expired@example.com",
        [(KNOWN_ACCOUNT, roles_of(KNOWN_ACCOUNT))],
        Duration::ZERO,
    );
    let logged_out = portal.logout(Some("tok-logged-out")).await;
    assert_eq!(
        logged_out.status(),
        StatusCode::OK,
        "case \"the logout that ends the session\": expected 200, got {} {:?}",
        logged_out.status(),
        body_text(&logged_out)
    );

    let tokens = [
        ("a logged-out session", "tok-logged-out"),
        ("an expired session", "tok-expired"),
        ("a token no session was ever minted for", "tok-unknown"),
    ];
    for (class, token) in tokens {
        for (operation, call) in calls() {
            // Repeating the call must not change the answer: the refusal is not a
            // transition, so the second and third attempts match the first.
            for attempt in 1..=3_u32 {
                let case = format!("{operation} with {class}, attempt {attempt}");
                let response = send(&portal, &call, token).await;
                assert_eq!(
                    (
                        response.status(),
                        error_code(&response),
                        error_message(&response)
                    ),
                    (
                        StatusCode::UNAUTHORIZED,
                        "UnauthorizedException".to_string(),
                        EXPECTED_MESSAGE.to_string()
                    ),
                    "case {case:?}: expected 401 UnauthorizedException {EXPECTED_MESSAGE:?}, \
                     got {} {:?}",
                    response.status(),
                    body_text(&response)
                );
            }
        }
    }
}
