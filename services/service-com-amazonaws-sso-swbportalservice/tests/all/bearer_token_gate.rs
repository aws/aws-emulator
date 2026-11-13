// SPDX-License-Identifier: CC0-1.0

//! The portal's bearer credential, end to end over HTTP — including the request that
//! presents NO credential at all.
//!
//! RULE: the portal authenticates every request by the sign-in session its bearer token
//! names, and does so BEFORE it reads anything else from the request; a request that
//! presents no usable session — the modelled header absent, empty, or naming a session
//! that is unknown, logged out or expired — is refused as unauthenticated (HTTP 401
//! `UnauthorizedException`, with the gate's one message) on every bound route, whatever
//! else the request carries or omits. Only once a usable session IS presented is the
//! rest of the request read, so that is when a missing or out-of-range parameter becomes
//! the modelled 400.
//!
//! The suite sweeps one table of requests — the four routes × what else the request
//! carries (every parameter present, a required parameter omitted, each published
//! `max_result` bound, values that are unicode or injection-shaped) — twice: once per
//! class of unusable credential, and once for a live session, where each row's own
//! modelled answer is asserted instead. The recorded case (one route, one credential) is
//! one cell of that table.
//!
//! A request carrying a VALUE outside a modelled constraint is not a row here either,
//! but for a reason of its own: that value is refused by the framework's own boundary
//! validation, which runs over a request the router already matched to an operation, so
//! no handler — and no handler's own gate — runs for it, and the rule above has to be
//! answered before the framework reads the request at all.
//! `authentication_precedes_reading.rs` sweeps that half, the same classes of unusable
//! credential against a `max_result` outside the modelled range; `portal_session.rs`
//! pins those `max_result` rows (0, 101, -1 — one past each published bound) as the
//! modelled 400 an AUTHENTICATED caller earns.

use super::portal_session::{body_json, body_text, error_code, error_message, roles_of, Portal};
use bytes::Bytes;
use http::{Response, StatusCode};
use std::time::Duration;

/// The message the Portal API answers an unusable bearer token with, pinned here as a
/// literal so this suite fails if either side of the gate changes its wording.
const EXPECTED_MESSAGE: &str = "Session token not found or invalid";

/// An account and a role the `Portal` fixture really holds and assigns, so a served row
/// is served out of state and a refused row is refused by the rule under test rather
/// than by the absence of the resource.
const KNOWN_ACCOUNT: &str = "111111111111";
const KNOWN_ROLE: &str = "AdministratorAccess";

/// How a request presents its credential.
#[derive(Debug, Clone)]
enum Credential {
    /// The modelled `x-amz-sso_bearer_token` header is not on the request at all — the
    /// request a client makes before it has signed in, or after dropping the header.
    Absent,
    /// The header is there, carrying this value.
    Presented(String),
}

/// One request to a bound route: the method and path come from `OPERATIONS.md`
/// (`GET /assignment/accounts`, `GET /assignment/roles`, `GET /federation/credentials`,
/// `POST /logout`).
enum Call {
    Get(String),
    Logout,
}

/// What an AUTHENTICATED caller gets back for a row, once the gate has let the request
/// through and the rest of it is read.
#[derive(Debug, Clone, Copy)]
enum Expect {
    /// 200 — inside every modelled rule, for the state the fixture seeded.
    Served,
    /// A modelled refusal with this status and error code.
    Refused(StatusCode, &'static str),
}

/// One cell of the request table: what the request is, and what a live session earns for
/// it. The unauthenticated sweep ignores `authenticated` — that is the point of the
/// rule: the gate answers before any of this is read.
struct Row {
    case: &'static str,
    call: Call,
    authenticated: Expect,
}

async fn send(portal: &Portal, call: &Call, credential: &Credential) -> Response<Bytes> {
    let token = match credential {
        Credential::Absent => None,
        Credential::Presented(token) => Some(token.as_str()),
    };
    match call {
        Call::Get(uri) => portal.get(uri, token).await,
        Call::Logout => portal.logout(token).await,
    }
}

/// The requests, spanning the four routes and everything else a portal request can
/// carry: all parameters present, a `@required` parameter omitted, each published bound
/// of the modelled `MaxResultType` range (1..100), a page token this service never
/// issued, and query values that are unicode or injection-shaped (percent-encoded, since
/// a URI carries no raw space or non-ASCII byte).
fn requests() -> Vec<Row> {
    vec![
        Row {
            case: "ListAccounts",
            call: Call::Get("/assignment/accounts".to_string()),
            authenticated: Expect::Served,
        },
        Row {
            case: "ListAccounts with max_result at the modelled minimum",
            call: Call::Get("/assignment/accounts?max_result=1".to_string()),
            authenticated: Expect::Served,
        },
        Row {
            case: "ListAccounts with max_result at the modelled maximum",
            call: Call::Get("/assignment/accounts?max_result=100".to_string()),
            authenticated: Expect::Served,
        },
        Row {
            case: "ListAccounts with a next_token this service never issued",
            call: Call::Get("/assignment/accounts?next_token=not-a-cursor".to_string()),
            authenticated: Expect::Refused(StatusCode::BAD_REQUEST, "InvalidRequestException"),
        },
        Row {
            case: "ListAccountRoles for an assigned account",
            call: Call::Get(format!("/assignment/roles?account_id={KNOWN_ACCOUNT}")),
            authenticated: Expect::Served,
        },
        Row {
            case: "ListAccountRoles without its required account_id",
            call: Call::Get("/assignment/roles".to_string()),
            authenticated: Expect::Refused(StatusCode::BAD_REQUEST, "InvalidRequestException"),
        },
        Row {
            case: "ListAccountRoles for an account no assignment names",
            call: Call::Get("/assignment/roles?account_id=999999999999".to_string()),
            authenticated: Expect::Refused(StatusCode::NOT_FOUND, "ResourceNotFoundException"),
        },
        Row {
            // "' OR '1'='1", percent-encoded.
            case: "ListAccountRoles with an injection-shaped account_id",
            call: Call::Get("/assignment/roles?account_id=%27%20OR%20%271%27%3D%271".to_string()),
            authenticated: Expect::Refused(StatusCode::NOT_FOUND, "ResourceNotFoundException"),
        },
        Row {
            case: "GetRoleCredentials for an assigned role",
            call: Call::Get(format!(
                "/federation/credentials?account_id={KNOWN_ACCOUNT}&role_name={KNOWN_ROLE}"
            )),
            authenticated: Expect::Served,
        },
        Row {
            case: "GetRoleCredentials without its required account_id",
            call: Call::Get(format!("/federation/credentials?role_name={KNOWN_ROLE}")),
            authenticated: Expect::Refused(StatusCode::BAD_REQUEST, "InvalidRequestException"),
        },
        Row {
            case: "GetRoleCredentials without its required role_name",
            call: Call::Get(format!("/federation/credentials?account_id={KNOWN_ACCOUNT}")),
            authenticated: Expect::Refused(StatusCode::BAD_REQUEST, "InvalidRequestException"),
        },
        Row {
            case: "GetRoleCredentials with neither required parameter",
            call: Call::Get("/federation/credentials".to_string()),
            authenticated: Expect::Refused(StatusCode::BAD_REQUEST, "InvalidRequestException"),
        },
        Row {
            // "роль" (U+0440 U+043E U+043B U+044C), percent-encoded UTF-8.
            case: "GetRoleCredentials with a unicode role_name",
            call: Call::Get(format!(
                "/federation/credentials?account_id={KNOWN_ACCOUNT}\
                 &role_name=%D1%80%D0%BE%D0%BB%D1%8C"
            )),
            authenticated: Expect::Refused(StatusCode::NOT_FOUND, "ResourceNotFoundException"),
        },
        Row {
            case: "Logout",
            call: Call::Logout,
            authenticated: Expect::Served,
        },
    ]
}

/// Every way a request can present no usable session, as `(class, credential)`.
///
/// `tok-expired` and `tok-logged-out` are seeded by the caller, so the table covers a
/// session that never existed, two that stopped being usable, and a request carrying no
/// credential at all. Every presented value is visible ASCII because an HTTP header
/// value is.
fn unusable_credentials() -> Vec<(&'static str, Credential)> {
    vec![
        ("no bearer-token header at all", Credential::Absent),
        ("an empty header value", Credential::Presented(String::new())),
        (
            "a whitespace-only header value",
            Credential::Presented("   ".to_string()),
        ),
        (
            "a token that was never issued",
            Credential::Presented("sim2real-bogus-abc".to_string()),
        ),
        (
            "a JWT-shaped token",
            Credential::Presented(
                "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJhZG1pbiJ9.c2lnbmF0dXJl".to_string(),
            ),
        ),
        (
            "an oversized token",
            Credential::Presented("A".repeat(4096)),
        ),
        (
            "an expired session",
            Credential::Presented("tok-expired".to_string()),
        ),
        (
            "a logged-out session",
            Credential::Presented("tok-logged-out".to_string()),
        ),
    ]
}

/// A portal with the fixture seeded, one live session that is never spent, one expired
/// session and one that has been logged out.
async fn portal_with_unusable_sessions() -> Portal {
    let portal = Portal::new();
    portal.sign_in("tok-live", &[KNOWN_ACCOUNT]);
    portal.sign_in("tok-logged-out", &[KNOWN_ACCOUNT]);
    // `put_session` dates the expiry from now, so a zero ttl is already past.
    portal.state().lock().put_session(
        "tok-expired",
        "expired@example.com",
        [(KNOWN_ACCOUNT, roles_of(KNOWN_ACCOUNT))],
        Duration::ZERO,
    );
    let ended = portal.logout(Some("tok-logged-out")).await;
    assert_eq!(
        ended.status(),
        StatusCode::OK,
        "case \"the logout that ends the session\": expected 200, got {} {:?}",
        ended.status(),
        body_text(&ended)
    );
    portal
}

/// Assert one response is the gate's refusal and nothing more.
fn assert_unauthenticated(case: &str, response: &Response<Bytes>, presented: &Credential) {
    assert_eq!(
        (
            response.status(),
            error_code(response),
            error_message(response)
        ),
        (
            StatusCode::UNAUTHORIZED,
            "UnauthorizedException".to_string(),
            EXPECTED_MESSAGE.to_string()
        ),
        "case {case:?}: expected 401 UnauthorizedException {EXPECTED_MESSAGE:?}, got {} {:?}",
        response.status(),
        body_text(response)
    );
    // restJson1 clients resolve the error shape from this header, so the pre-handler
    // refusal must carry it exactly as a handler's own refusal does.
    assert_eq!(
        response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok()),
        Some("UnauthorizedException"),
        "case {case:?}: expected the error-type header, got {:?}",
        response.headers().get("x-amzn-errortype")
    );
    // The refusal body is the envelope and nothing else — no listing, no credential, no
    // page cursor, and nothing of the presented token.
    let body = body_json(response);
    let members: Vec<String> = body
        .as_object()
        .map(|object| object.keys().cloned().collect())
        .unwrap_or_default();
    assert!(
        members.len() == 2
            && members.iter().any(|key| key == "__type")
            && members.iter().any(|key| key == "message"),
        "case {case:?}: the refusal body must carry only __type and message, got {members:?}"
    );
    if let Credential::Presented(token) = presented {
        // A token long enough not to occur in ordinary prose would show up here if the
        // message quoted it. (A short or whitespace token is a substring of almost any
        // sentence, so it is not checked.)
        assert!(
            token.trim().len() < 8 || !body_text(response).contains(token.as_str()),
            "case {case:?}: the refusal must not quote the presented token, got {:?}",
            body_text(response)
        );
    }
}

/// RULE: every request presenting no usable session is 401 `UnauthorizedException` with
/// the gate's one message, on every bound route, whatever else it carries or omits.
#[tokio::test]
async fn a_request_presenting_no_usable_session_is_unauthenticated_on_every_route() {
    let portal = portal_with_unusable_sessions().await;

    for (class, credential) in unusable_credentials() {
        for row in requests() {
            let case = format!("{} with {class}", row.case);
            let response = send(&portal, &row.call, &credential).await;
            assert_unauthenticated(&case, &response, &credential);
        }
    }

    // The gate spent nothing: the live session was never presented above, and it is
    // still served afterwards.
    let served = portal.get("/assignment/accounts", Some("tok-live")).await;
    assert_eq!(
        served.status(),
        StatusCode::OK,
        "case \"a live session after every refusal\": expected 200, got {} {:?}",
        served.status(),
        body_text(&served)
    );
}

/// RULE (the ordering half): the SAME requests, presented with a usable session, are
/// read — so each earns its own modelled answer, and the 400 for a missing or
/// out-of-range parameter is reachable only once the caller is authenticated.
#[tokio::test]
async fn an_authenticated_caller_earns_each_requests_own_modelled_answer() {
    let portal = Portal::new();

    for (index, row) in requests().into_iter().enumerate() {
        // A fresh session per row: `Logout` invalidates the one it is given, so a shared
        // token would make a row's answer depend on an earlier row's.
        let token = format!("tok-live-{index}");
        portal.sign_in(&token, &[KNOWN_ACCOUNT]);
        let response = send(&portal, &row.call, &Credential::Presented(token)).await;
        match row.authenticated {
            Expect::Served => assert_eq!(
                response.status(),
                StatusCode::OK,
                "case {:?}: an authenticated, modelled request is served, got {} {:?}",
                row.case,
                response.status(),
                body_text(&response)
            ),
            Expect::Refused(status, code) => assert_eq!(
                (response.status(), error_code(&response)),
                (status, code.to_string()),
                "case {:?}: expected {status} {code}, got {} {:?}",
                row.case,
                response.status(),
                body_text(&response)
            ),
        }
    }
}

/// RULE (one gate, one answer): how the session failed to be presented does not reach
/// the caller, so the answer to a request carrying NO credential header is the very same
/// response — status, envelope headers and body — as the answer to one whose token names
/// no session.
#[tokio::test]
async fn an_absent_credential_answers_exactly_what_an_unknown_token_answers() {
    let portal = portal_with_unusable_sessions().await;

    for row in requests() {
        let absent = send(&portal, &row.call, &Credential::Absent).await;
        let unknown = send(
            &portal,
            &row.call,
            &Credential::Presented("sim2real-bogus-abc".to_string()),
        )
        .await;
        let envelope = |response: &Response<Bytes>| {
            (
                response.status(),
                response
                    .headers()
                    .get("content-type")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default()
                    .to_string(),
                response
                    .headers()
                    .get("x-amzn-errortype")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default()
                    .to_string(),
                body_text(response),
            )
        };
        assert_eq!(
            envelope(&absent),
            envelope(&unknown),
            "case {:?}: an absent credential and an unknown token must answer alike, got \
             {:?} and {:?}",
            row.case,
            envelope(&absent),
            envelope(&unknown)
        );
    }
}
