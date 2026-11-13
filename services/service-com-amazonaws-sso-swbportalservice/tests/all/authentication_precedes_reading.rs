// SPDX-License-Identifier: CC0-1.0

//! Where authentication falls relative to reading the rest of the request, end to end
//! over HTTP.
//!
//! RULE: the portal's only caller identity is the sign-in session the presented bearer
//! token names, so that session is resolved BEFORE any other value of the request is
//! read; a request presenting no usable session is refused as unauthenticated (HTTP 401
//! `UnauthorizedException`, with the gate's one message) whatever else it carried —
//! including a value the service cannot read at all — and only a request that presents
//! one goes on to earn the modelled answer that value deserves.
//!
//! `bearer_token_gate.rs` sweeps that rule over requests whose values are all readable.
//! This suite sweeps the half it leaves out: a request carrying a value outside a
//! modelled constraint, which the framework's own boundary validation refuses over a
//! request already matched to an operation, so no handler (and no handler's gate) runs
//! for it. The two classes of unreadable parameter one portal request can carry — a
//! `max_result` outside the modelled range, and a `next_token` this service never issued
//! — are swept side by side, because the rule gives them the same precedence.
//!
//! The bound comes from the model: `MaxResultType` is an integer with
//! `range { min: 1, max: 100 }`, bound to the `max_result` query parameter of
//! `ListAccounts` and `ListAccountRoles` (the only constrained member this service's
//! model declares; `GetRoleCredentials` and `Logout` carry none, so their only
//! pre-handler refusal is an absent required member, which `route_path_agreement.rs`
//! and `portal_session.rs` pin).

use super::portal_session::{body_json, body_text, error_code, error_message, roles_of, Portal};
use bytes::Bytes;
use http::{Response, StatusCode};
use service_com_amazonaws_sso_swbportalservice::state::{self, INVALID_SESSION_MESSAGE};
use std::time::{Duration, SystemTime};

/// An account and a role the `Portal` fixture really holds, so a served row is served out
/// of state and a refused row is refused by the rule under test.
const KNOWN_ACCOUNT: &str = "111111111111";
const KNOWN_ROLE: &str = "AdministratorAccess";

/// The published bounds of the modelled `MaxResultType` range, from the model
/// (`range { min: 1, max: 100 }`).
const PAGE_BOUND: (i64, i64) = (1, 100);

/// What an AUTHENTICATED caller earns for a row, once the gate has let the request
/// through and the rest of it is read.
#[derive(Debug, Clone, Copy)]
enum Expect {
    /// Inside every modelled rule, so the request is served.
    Served,
    /// This service's modelled refusal of a request it will not read: 400
    /// `InvalidRequestException`.
    Invalid,
    /// Refused 400 by the codec, before any rule of this service applies, so the code is
    /// the framework's rather than a modelled one.
    Unreadable,
}

/// The listings that bind the constrained member, with the query each needs to name a
/// route at all.
fn bound_routes() -> Vec<(&'static str, String)> {
    vec![
        ("ListAccounts", "/assignment/accounts".to_string()),
        (
            "ListAccountRoles",
            format!("/assignment/roles?account_id={KNOWN_ACCOUNT}"),
        ),
    ]
}

/// The query this suite appends to a route, as `(case, query, what a live session earns)`.
///
/// Rows span the published bounds and one past each of them, a negative, a value past the
/// modelled integer width, a value that is no integer at all, the OTHER unreadable
/// parameter of the same request shape (a page token this service never issued), both
/// together, and the request that carries neither.
fn queries() -> Vec<(String, String, Expect)> {
    let (min, max) = PAGE_BOUND;
    let mut rows: Vec<(String, String, Expect)> = vec![
        ("no page bound at all".to_string(), String::new(), Expect::Served),
        (
            "a next_token this service never issued".to_string(),
            "next_token=not-a-cursor".to_string(),
            Expect::Invalid,
        ),
        (
            "a next_token this service never issued, with a bound inside the range"
                .to_string(),
            format!("next_token=zz-not-hex&max_result={min}"),
            Expect::Invalid,
        ),
        (
            "a next_token this service never issued, with a bound outside the range"
                .to_string(),
            format!("next_token=not-a-cursor&max_result={}", max + 1),
            Expect::Invalid,
        ),
        (
            "a page bound that is no integer".to_string(),
            "max_result=not-a-number".to_string(),
            Expect::Unreadable,
        ),
        (
            "a page bound past the modelled integer width".to_string(),
            format!("max_result={}", i64::from(i32::MAX) + 1),
            Expect::Unreadable,
        ),
    ];
    // Each published bound (inside the range) and one past each of them, plus a negative
    // bound and zero: the values the framework refuses before a handler runs.
    for value in [min, max] {
        rows.push((
            format!("the page bound {value}, a published bound"),
            format!("max_result={value}"),
            Expect::Served,
        ));
    }
    for value in [min - 1, max + 1, -1] {
        rows.push((
            format!("the page bound {value}, one the modelled range excludes"),
            format!("max_result={value}"),
            Expect::Invalid,
        ));
    }
    rows
}

/// Every way a request can present no usable session, as `(class, header value)`. `None`
/// is the request carrying no credential header at all.
///
/// Seeded by [`portal_with_unusable_sessions`]: a session that never existed, one that
/// expired, one that was logged out, and two header values that name nothing.
fn unusable_credentials() -> Vec<(&'static str, Option<&'static str>)> {
    vec![
        ("no bearer-token header at all", None),
        ("an empty header value", Some("")),
        ("a token that was never issued", Some("tok-never-issued")),
        ("an expired session", Some("tok-expired")),
        ("a logged-out session", Some("tok-logged-out")),
    ]
}

/// A portal with the fixture seeded, one expired session and one that has been logged
/// out.
async fn portal_with_unusable_sessions() -> Portal {
    let portal = Portal::new();
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

/// The route and the query of a row, as one URI.
fn uri_of(route: &str, query: &str) -> String {
    if query.is_empty() {
        return route.to_string();
    }
    let separator = if route.contains('?') { '&' } else { '?' };
    format!("{route}{separator}{query}")
}

/// The whole caller-visible envelope of a response: what a client branches on, plus the
/// body verbatim.
fn envelope(response: &Response<Bytes>) -> (StatusCode, String, String, String) {
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    (
        response.status(),
        header("content-type"),
        header("x-amzn-errortype"),
        body_text(response),
    )
}

/// Assert one response is the gate's refusal and nothing more.
fn assert_unauthenticated(case: &str, response: &Response<Bytes>) {
    assert_eq!(
        (
            response.status(),
            error_code(response),
            error_message(response)
        ),
        (
            StatusCode::UNAUTHORIZED,
            "UnauthorizedException".to_string(),
            INVALID_SESSION_MESSAGE.to_string()
        ),
        "case {case:?}: expected 401 UnauthorizedException {INVALID_SESSION_MESSAGE:?}, got \
         {} {:?}",
        response.status(),
        body_text(response)
    );
    assert_eq!(
        response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok()),
        Some("UnauthorizedException"),
        "case {case:?}: expected the error-type header, got {:?}",
        response.headers().get("x-amzn-errortype")
    );
    // The refusal body is the envelope and nothing else: nothing the request carried
    // reaches it, so nothing of the unreadable value can.
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
}

/// RULE: a request presenting no usable session is 401 `UnauthorizedException` with the
/// gate's one message even when a value it carries is one the service cannot read at all
/// — so a value outside a modelled bound and a page token this service never issued get
/// the same precedence, on every route that binds the constrained member and for every
/// class of unusable session.
#[tokio::test]
async fn an_unusable_session_is_unauthenticated_whatever_value_the_request_carries() {
    let portal = portal_with_unusable_sessions().await;

    for (route_case, route) in bound_routes() {
        // The answer the plain, readable request gets for the same credential: the rule
        // says every row answers exactly this, because none of them is read.
        for (class, credential) in unusable_credentials() {
            let plain = portal.get(&route, credential).await;
            assert_unauthenticated(&format!("{route_case} with {class}"), &plain);
            for (query_case, query, _) in queries() {
                let case = format!("{route_case} carrying {query_case}, with {class}");
                let uri = uri_of(&route, &query);
                let response = portal.get(&uri, credential).await;
                assert_unauthenticated(&case, &response);
                assert_eq!(
                    envelope(&response),
                    envelope(&plain),
                    "case {case:?}: {uri:?} must answer exactly what the readable request \
                     answers, got {:?} against {:?}",
                    envelope(&response),
                    envelope(&plain)
                );
            }
        }
    }
}

/// RULE (the ordering half): the SAME requests presented with a usable session ARE read,
/// so each earns its own modelled answer — the 400 for a value outside the modelled range
/// is reachable only once the caller is authenticated.
#[tokio::test]
async fn an_authenticated_caller_earns_the_modelled_answer_for_the_same_value() {
    let portal = Portal::new();
    portal.sign_in("tok-live", &[KNOWN_ACCOUNT]);

    for (route_case, route) in bound_routes() {
        for (query_case, query, expect) in queries() {
            let case = format!("{route_case} carrying {query_case}");
            let uri = uri_of(&route, &query);
            let response = portal.get(&uri, Some("tok-live")).await;
            match expect {
                Expect::Served => assert_eq!(
                    response.status(),
                    StatusCode::OK,
                    "case {case:?}: {uri:?} is inside every modelled rule, expected 200, got \
                     {} {:?}",
                    response.status(),
                    body_text(&response)
                ),
                Expect::Invalid => assert_eq!(
                    (response.status(), error_code(&response).as_str()),
                    (StatusCode::BAD_REQUEST, "InvalidRequestException"),
                    "case {case:?}: {uri:?} is a request this service will not read, \
                     expected 400 InvalidRequestException, got {} {:?}",
                    response.status(),
                    body_text(&response)
                ),
                Expect::Unreadable => assert_eq!(
                    response.status(),
                    StatusCode::BAD_REQUEST,
                    "case {case:?}: {uri:?} is unreadable as an integer, expected 400, got \
                     {} {:?}",
                    response.status(),
                    body_text(&response)
                ),
            }
        }
    }

    // The sweep spent nothing: the session is still served afterwards.
    let served = portal.get("/assignment/accounts", Some("tok-live")).await;
    assert_eq!(
        served.status(),
        StatusCode::OK,
        "case \"a live session after every refusal\": expected 200, got {} {:?}",
        served.status(),
        body_text(&served)
    );
}

/// RULE (the clock's, under the gate): a minted role credential ends at the expiration its
/// response advertised whatever request arrives next — including one the gate refuses
/// before the request is read.
#[tokio::test]
async fn a_refused_request_still_ends_a_credential_the_clock_has_spent() {
    let (min, max) = PAGE_BOUND;
    // Rows: the query the refused request carries — one past each published bound, and a
    // value that is no integer at all.
    let queries = [
        format!("max_result={}", min - 1),
        format!("max_result={}", max + 1),
        "max_result=not-a-number".to_string(),
    ];

    for query in queries {
        let portal = Portal::new();
        portal.sign_in("tok-live", &[KNOWN_ACCOUNT]);
        let minted = portal
            .get(
                &format!(
                    "/federation/credentials?account_id={KNOWN_ACCOUNT}&role_name={KNOWN_ROLE}"
                ),
                Some("tok-live"),
            )
            .await;
        assert_eq!(
            minted.status(),
            StatusCode::OK,
            "case {query:?}: minting a role credential must succeed, got {} {:?}",
            minted.status(),
            body_text(&minted)
        );
        let access_key_id = body_json(&minted)["roleCredentials"]["accessKeyId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let key = (
            "tok-live".to_string(),
            KNOWN_ACCOUNT.to_string(),
            KNOWN_ROLE.to_string(),
        );

        // Put the clock past that expiration: the state time itself reaches once the
        // credential's own duration has run out.
        {
            let store = portal.state();
            let mut store = store.lock();
            let record = store
                .issued_credentials
                .get_mut(&key)
                .unwrap_or_else(|| panic!("case {query:?}: the minted credential must be stored"));
            record.expiration_millis = state::epoch_millis(SystemTime::now()) - 1;
        }

        // The next request is one the gate refuses before reading it.
        let refused = portal
            .get(
                &uri_of("/assignment/accounts", &query),
                Some("tok-never-issued"),
            )
            .await;
        assert_unauthenticated(&format!("a refused request carrying {query:?}"), &refused);

        let store = portal.state();
        let store = store.lock();
        assert!(
            !store.issued_credentials.contains_key(&key),
            "case {query:?}: the spent credential must be forgotten, still holding {:?}",
            store.issued_credentials.get(&key)
        );
        assert!(
            store
                .signed_in_subject(&access_key_id, SystemTime::now())
                .is_none(),
            "case {query:?}: the spent access key must speak for nobody, got {:?}",
            store.signed_in_subject(&access_key_id, SystemTime::now())
        );
    }
}
