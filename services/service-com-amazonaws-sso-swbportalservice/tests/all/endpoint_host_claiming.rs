// SPDX-License-Identifier: CC0-1.0

//! Which ENDPOINT HOSTS this service claims a request on, and which it leaves alone.
//!
//! RULE: a portal request names no service out of band — every operation is
//! `@optionalAuth` with an empty `@auth`, so an SDK sends it with no SigV4 credential
//! scope, and `restJson1` carries no `X-Amz-Target` — so the endpoint host is the only
//! routing signal left, and a host names a service only when it is a service's PUBLISHED
//! endpoint. This service's own endpoint is claimed whole; another service's endpoint is
//! that service's, even on a path this model binds; and every other host names NO service
//! — the emulator's own address, an IP literal, no host at all, and equally an ordinary
//! operator hostname (a compose service name, a container hostname, a corporate DNS name,
//! a reverse proxy), whose first label the framework's host-style tier does resolve a
//! routing key from while naming no service with it. On such a host the request is
//! attributed by its own modelled signature, so each of the four `@http` bindings answers
//! its modelled result — including the state change `Logout` exists to make — and a path
//! no binding names still reaches no service.
//!
//! The host each row carries is the only thing that differs between them: the same
//! request on the same seeded state is dispatched at every one, so a row can only be
//! satisfied by the claim rule, never by the request's own shape.

use crate::portal_session::{body_json, body_text, error_code, Portal};
use bytes::Bytes;
use http::{Method, Request, Response, StatusCode};

/// The header the model binds the access token to (`OPERATIONS.md`).
const ACCESS_TOKEN_HEADER: &str = "x-amz-sso_bearer_token";

/// The account every row's session is assigned to, and its name and email, as
/// [`Portal`]'s own fixture seeds them (`portal_session.rs`'s `FIXTURE`).
const ACCOUNT_ID: &str = "111111111111";
const ACCOUNT_NAME: &str = "Production";
const ACCOUNT_EMAIL: &str = "production@example.com";

/// The roles that fixture assigns in that account, ascending — the order a role listing
/// reports them in.
const ACCOUNT_ROLES: &[&str] = &["AdministratorAccess", "ReadOnly"];

/// A path no `@http` binding of this model names, so no route attributes it.
const UNMODELLED_PATH: &str = "/assignment/nope";

/// What a host says about which service answers at it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Names {
    /// No service at all: the emulator's own address, an IP literal, no host header, or
    /// an ordinary hostname an operator points a client at. The request is attributed by
    /// its own modelled signature, and nothing else is claimed.
    NoService,
    /// This service's own published endpoint — the model's `awsService` endpoint prefix
    /// (`portal.sso`), alone or followed by a dot. The endpoint is claimed WHOLE, so even
    /// a path this model does not bind gets this service's own routing answer.
    ThisEndpoint,
    /// Another service's published endpoint: a host in one of the zones AWS serves its
    /// endpoints in whose first label is not this endpoint's. Nothing is claimed.
    AnotherEndpoint,
}

/// The endpoints a client can be pointed at, as `(case, Host, what the host names)`.
///
/// The rows span what the framework's host-style routing tier can make of a host: the
/// hosts it resolves NO routing key from (a loopback name, an IP literal, a bracketed
/// IPv6 literal, no `Host` header at all), the hosts it DOES resolve one from while that
/// key names no service (a single-label compose service name, a container hostname, a
/// corporate DNS name, a proxy with a port, the same name in upper case, and a host that
/// merely starts with this endpoint's prefix label), this service's own endpoint in three
/// published spellings, and five other services' endpoints across the zones AWS publishes
/// its endpoints in (`amazonaws.com`, `amazonaws.com.cn`, `api.aws`, `on.aws`), one of
/// them in upper case.
const ENDPOINTS: &[(&str, Option<&str>, Names)] = &[
    ("the emulator's own loopback endpoint", Some("localhost:9297"), Names::NoService),
    ("a loopback IP with a port", Some("127.0.0.1:9297"), Names::NoService),
    ("a bracketed IPv6 loopback literal", Some("[::1]:9297"), Names::NoService),
    ("no Host header at all", None, Names::NoService),
    ("a single-label compose service name", Some("aws:9297"), Names::NoService),
    ("a container hostname", Some("host.docker.internal:9297"), Names::NoService),
    ("a corporate DNS name", Some("emulator.corp.local:9297"), Names::NoService),
    ("a proxy DNS name with a port", Some("sso-proxy.internal.example:8443"), Names::NoService),
    ("a corporate DNS name in upper case", Some("EMULATOR.CORP.LOCAL:9297"), Names::NoService),
    (
        "a host that merely starts with this endpoint's prefix",
        Some("portal.ssoother.example.com"),
        Names::NoService,
    ),
    (
        "the modelled portal endpoint",
        Some("portal.sso.us-east-1.amazonaws.com"),
        Names::ThisEndpoint,
    ),
    ("the endpoint prefix with nothing after it", Some("portal.sso"), Names::ThisEndpoint),
    (
        "the portal endpoint in another partition",
        Some("portal.sso.cn-north-1.amazonaws.com.cn"),
        Names::ThisEndpoint,
    ),
    ("another service's endpoint", Some("iam.amazonaws.com"), Names::AnotherEndpoint),
    (
        "another service's regional endpoint",
        Some("s3.us-west-2.amazonaws.com"),
        Names::AnotherEndpoint,
    ),
    (
        "another service's endpoint in another partition",
        Some("dynamodb.cn-north-1.amazonaws.com.cn"),
        Names::AnotherEndpoint,
    ),
    (
        "another service's dual-stack endpoint",
        Some("lambda.us-east-1.api.aws"),
        Names::AnotherEndpoint,
    ),
    (
        "another service's short-form endpoint",
        Some("console.us-east-1.on.aws"),
        Names::AnotherEndpoint,
    ),
    (
        "another service's endpoint in upper case",
        Some("S3.US-WEST-2.AMAZONAWS.COM"),
        Names::AnotherEndpoint,
    ),
];

/// What a 200 from one route must report, so a row cannot be satisfied by an empty
/// answer.
#[derive(Debug, Clone, Copy)]
enum Success {
    /// `ListAccounts` reports the account the presented session is assigned to.
    AccountList,
    /// `ListAccountRoles` reports the roles assigned in that account.
    RoleList,
    /// `GetRoleCredentials` reports credentials the emulator minted.
    Credentials,
    /// `Logout` models no output members: the state change is what it does, which this
    /// suite asserts on the store directly.
    NoOutput,
}

/// The four routes the model binds, with every `@required` http-bound parameter present:
/// `(operation, method, path and query, what a 200 reports)`. The (method, path) halves
/// are the `@http` bindings `smithy_query(query="digest")` reports.
const ROUTES: &[(&str, Method, &str, Success)] = &[
    (
        "ListAccounts",
        Method::GET,
        "/assignment/accounts",
        Success::AccountList,
    ),
    (
        "ListAccountRoles",
        Method::GET,
        "/assignment/roles?account_id=111111111111",
        Success::RoleList,
    ),
    (
        "GetRoleCredentials",
        Method::GET,
        "/federation/credentials?account_id=111111111111&role_name=AdministratorAccess",
        Success::Credentials,
    ),
    ("Logout", Method::POST, "/logout", Success::NoOutput),
];

/// One emulator holding this service, plus the handle its state is seeded through, is
/// [`crate::portal_session::Portal`] — the crate's own harness, reused rather than copied.
/// Only the request BUILDER below is this suite's own, because the endpoint host a request
/// is addressed to is exactly what this suite varies and that harness addresses the
/// modelled portal endpoint.
///
/// Whether the store still serves the sign-in session `access_token` names, read through
/// the same store the handlers reach.
fn session_is_active(portal: &Portal, access_token: &str) -> bool {
    portal
        .state()
        .lock()
        .sessions
        .get(access_token)
        .is_some_and(|session| session.active)
}

/// A portal request: `host` is the endpoint the client was pointed at (`None` sends an
/// origin-form request with no `Host` header and no authority), and `token` rides in the
/// modelled access-token header when the row presents one.
fn portal_request(
    method: &Method,
    path: &str,
    host: Option<&str>,
    token: Option<&str>,
) -> Request<Bytes> {
    let mut builder = Request::builder().method(method.clone());
    builder = match host {
        Some(host) => builder
            .uri(format!("https://{host}{path}"))
            .header("host", host),
        None => builder.uri(path),
    };
    if let Some(token) = token {
        builder = builder.header(ACCESS_TOKEN_HEADER, token);
    }
    builder
        .body(Bytes::new())
        .expect("the test built a well-formed request")
}

/// Assert a 200 reports what `expected` says the route reports out of the seeded state.
fn assert_success(case: &str, response: &Response<Bytes>, expected: Success) {
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "case {case}: expected 200, got {} {:?}",
        response.status(),
        body_text(response)
    );
    let body = body_text(response);
    // `Logout` models no output members, so its 200 carries no JSON document to read:
    // the state change is what it does, which this suite asserts on the store.
    if matches!(expected, Success::NoOutput) {
        let payload = body.trim();
        assert!(
            payload.is_empty() || payload == "{}",
            "case {case}: this operation models no output members, got {body:?}"
        );
        return;
    }
    let payload = body_json(response);
    match expected {
        Success::AccountList => {
            let reported: Vec<(String, String, String)> = payload["accountList"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|account| {
                    (
                        account["accountId"].as_str().unwrap_or_default().to_string(),
                        account["accountName"].as_str().unwrap_or_default().to_string(),
                        account["emailAddress"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect();
            assert_eq!(
                reported,
                vec![(
                    ACCOUNT_ID.to_string(),
                    ACCOUNT_NAME.to_string(),
                    ACCOUNT_EMAIL.to_string()
                )],
                "case {case}: expected the account the session is assigned to, got \
                 {reported:?}"
            );
        }
        Success::RoleList => {
            let reported: Vec<String> = payload["roleList"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|role| role["roleName"].as_str().unwrap_or_default().to_string())
                .collect();
            let expected: Vec<String> =
                ACCOUNT_ROLES.iter().map(|role| (*role).to_string()).collect();
            assert_eq!(
                reported, expected,
                "case {case}: expected the roles assigned in that account, got {reported:?}"
            );
        }
        Success::Credentials => {
            let credentials = &payload["roleCredentials"];
            assert!(
                credentials["accessKeyId"]
                    .as_str()
                    .is_some_and(|id| !id.is_empty())
                    && credentials["secretAccessKey"]
                        .as_str()
                        .is_some_and(|secret| !secret.is_empty())
                    && credentials["sessionToken"]
                        .as_str()
                        .is_some_and(|token| !token.is_empty()),
                "case {case}: expected minted role credentials, got {body:?}"
            );
        }
        // Answered above, before the body was read as JSON.
        Success::NoOutput => unreachable!(),
    }
}

/// Assert a response is a failure carrying this status and this wire error code.
fn assert_error(case: &str, response: &Response<Bytes>, expected: (StatusCode, &str)) {
    assert_eq!(
        (response.status(), error_code(response).as_str()),
        expected,
        "case {case}: expected {} {}, got {} {:?}",
        expected.0,
        expected.1,
        response.status(),
        body_text(response)
    );
}

/// RULE (the serving half): on every host that names no service — and on this service's
/// own endpoint — each of the four modelled routes is attributed to this service by its
/// own (method, path) signature, so a live session is served out of state.
#[tokio::test]
async fn every_modelled_route_is_served_on_every_host_that_names_no_service() {
    let portal = Portal::new();

    for (endpoint, host, names) in ENDPOINTS {
        for (operation, method, path, expected) in ROUTES {
            let case = format!("{endpoint} / {operation}");
            // A fresh session per row: `Logout` ends the one it is given, so a shared
            // token would make one row's answer depend on another's.
            let token = format!("tok-{case}");
            portal.sign_in(&token, &[ACCOUNT_ID]);
            let response = portal
                .dispatch(portal_request(method, path, *host, Some(&token)))
                .await;
            match names {
                Names::NoService | Names::ThisEndpoint => {
                    assert_success(&case, &response, *expected)
                }
                // Another service's endpoint: this service claims nothing there, and no
                // other emulated service binds this route, so the router answers its own
                // `UnknownService`.
                Names::AnotherEndpoint => assert_error(
                    &case,
                    &response,
                    (StatusCode::NOT_FOUND, "UnknownService"),
                ),
            }
        }
    }
}

/// RULE (the state change): `Logout` invalidates the server-side sign-in session the
/// presented token names, so the session stops being served — on every host a request for
/// it is claimed on, and on no host where it is not claimed.
#[tokio::test]
async fn logout_ends_the_session_exactly_where_the_request_is_claimed() {
    let portal = Portal::new();

    for (endpoint, host, names) in ENDPOINTS {
        let token = format!("tok-logout-{endpoint}");
        portal.sign_in(&token, &[ACCOUNT_ID]);
        assert!(
            session_is_active(&portal, &token),
            "case {endpoint}: the fixture must seed an active session, got none"
        );

        let logout = portal
            .dispatch(portal_request(
                &Method::POST,
                "/logout",
                *host,
                Some(&token),
            ))
            .await;
        let claimed = *names != Names::AnotherEndpoint;
        if claimed {
            assert_success(&format!("{endpoint} / Logout"), &logout, Success::NoOutput);
        } else {
            assert_error(
                &format!("{endpoint} / Logout"),
                &logout,
                (StatusCode::NOT_FOUND, "UnknownService"),
            );
        }

        // The store itself, and then a later read through the API: a session that was
        // logged out is served to nobody, and one whose request reached no service is
        // untouched.
        let still_active = session_is_active(&portal, &token);
        assert_eq!(
            still_active, !claimed,
            "case {endpoint}: expected the session to be active={}, got active={still_active}",
            !claimed
        );
        let later = portal
            .dispatch(portal_request(
                &Method::GET,
                "/assignment/accounts",
                Some("localhost:9297"),
                Some(&token),
            ))
            .await;
        if claimed {
            assert_error(
                &format!("{endpoint} / a read after Logout"),
                &later,
                (StatusCode::UNAUTHORIZED, "UnauthorizedException"),
            );
        } else {
            assert_success(
                &format!("{endpoint} / a read after an unclaimed Logout"),
                &later,
                Success::AccountList,
            );
        }
    }
}

/// RULE (the credential gate, which only a claimed request reaches): on every host the
/// request is claimed on, a request presenting NO session is the modelled 401 — including
/// the one that presents no access-token header at all, which the REST router selects no
/// route for.
#[tokio::test]
async fn a_request_presenting_no_session_is_the_modelled_401_wherever_it_is_claimed() {
    let portal = Portal::new();

    // The credential a row presents: none at all (no header), and a header naming no
    // session this service holds.
    let credentials: &[(&str, Option<&str>)] = &[
        ("no access-token header at all", None),
        (
            "a token that was never issued",
            Some("a-token-that-was-never-issued"),
        ),
    ];

    for (endpoint, host, names) in ENDPOINTS {
        for (operation, method, path, _) in ROUTES {
            for (credential, token) in credentials {
                let case = format!("{endpoint} / {operation} with {credential}");
                let response = portal
                    .dispatch(portal_request(method, path, *host, *token))
                    .await;
                let expected = match names {
                    Names::NoService | Names::ThisEndpoint => {
                        (StatusCode::UNAUTHORIZED, "UnauthorizedException")
                    }
                    Names::AnotherEndpoint => (StatusCode::NOT_FOUND, "UnknownService"),
                };
                assert_error(&case, &response, expected);
            }
        }
    }
}

/// RULE (the converse half): what a host names decides how far the claim reaches. A path
/// no binding names is claimed only on this service's OWN endpoint — which is claimed
/// whole, so it answers this service's routing failure there — while a host that names no
/// service claims nothing but the routes this model binds, and another service's endpoint
/// claims nothing at all.
#[tokio::test]
async fn an_unmodelled_path_is_claimed_only_on_this_services_own_endpoint() {
    let portal = Portal::new();
    portal.sign_in("tok-live", &[ACCOUNT_ID]);

    for (endpoint, host, names) in ENDPOINTS {
        let response = portal
            .dispatch(portal_request(
                &Method::GET,
                UNMODELLED_PATH,
                *host,
                Some("tok-live"),
            ))
            .await;
        let expected = match names {
            // The endpoint is this service's, so the request reached it and this
            // service's own routing answer stands.
            Names::ThisEndpoint => (StatusCode::NOT_IMPLEMENTED, "NotImplemented"),
            // Nothing attributes the request: no name, and no modelled route.
            Names::NoService | Names::AnotherEndpoint => {
                (StatusCode::NOT_FOUND, "UnknownService")
            }
        };
        assert_error(
            &format!("{endpoint} / an unmodelled path"),
            &response,
            expected,
        );
    }
}

/// RULE: a request that names a service OUT OF BAND — a SigV4 credential scope, in the
/// `Authorization` header or in a presigned URL's `X-Amz-Credential`, or an
/// `X-Amz-Target` — belongs to the service it names, whatever host it was addressed to
/// and whatever path it addresses. So this service claims such a request only when the
/// name is its own signing name, and an ordinary hostname does not change that.
#[tokio::test]
async fn a_request_naming_a_service_out_of_band_goes_to_the_service_it_names() {
    let portal = Portal::new();

    /// A credential scope for `service`, as SigV4 spells it.
    fn scope(service: &str) -> String {
        format!(
            "AWS4-HMAC-SHA256 \
             Credential=EMULATORAKEXAMPLE/20260101/us-east-1/{service}/aws4_request, \
             SignedHeaders=host, Signature=0"
        )
    }

    /// One request that names a service out of band.
    struct Case {
        case: &'static str,
        /// The endpoint the client was pointed at.
        host: &'static str,
        /// The query string the row appends to the modelled path, for the presigned form
        /// that carries its credential scope there.
        query: String,
        /// The headers that name a service.
        headers: Vec<(&'static str, String)>,
        /// Whether this service claims it.
        claimed: bool,
    }

    /// An ordinary operator hostname — the host that names no service, so only the
    /// out-of-band signal decides each row.
    const ORDINARY_HOST: &str = "emulator.corp.local:9297";
    /// This service's signing name, as the model's `sigv4Name` gives it.
    const SIGNING_NAME: &str = "awsssoportal";

    // The rows differ along WHICH signal names a service, WHICH service it names, and —
    // for the last row — whether this service's own endpoint host can override a name
    // that is somebody else's.
    let cases = [
        Case {
            case: "a credential scope naming this service, on an ordinary hostname",
            host: ORDINARY_HOST,
            query: String::new(),
            headers: vec![("authorization", scope(SIGNING_NAME))],
            claimed: true,
        },
        Case {
            case: "a credential scope naming another service, on an ordinary hostname",
            host: ORDINARY_HOST,
            query: String::new(),
            headers: vec![("authorization", scope("iam"))],
            claimed: false,
        },
        Case {
            case: "a presigned credential scope naming another service",
            host: ORDINARY_HOST,
            query: "?X-Amz-Credential=EMULATORAKEXAMPLE%2F20260101%2Fus-east-1%2Fiam\
                    %2Faws4_request"
                .to_string(),
            headers: Vec::new(),
            claimed: false,
        },
        Case {
            case: "an X-Amz-Target naming another service, on an ordinary hostname",
            host: ORDINARY_HOST,
            query: String::new(),
            headers: vec![("x-amz-target", "DynamoDB_20120810.GetItem".to_string())],
            claimed: false,
        },
        Case {
            case: "an X-Amz-Target naming another service, on this service's endpoint",
            host: "portal.sso.us-east-1.amazonaws.com",
            query: String::new(),
            headers: vec![("x-amz-target", "DynamoDB_20120810.GetItem".to_string())],
            claimed: false,
        },
    ];

    for (index, row) in cases.iter().enumerate() {
        let token = format!("tok-out-of-band-{index}");
        portal.sign_in(&token, &[ACCOUNT_ID]);
        let path = format!("/assignment/accounts{}", row.query);
        let mut request = portal_request(&Method::GET, &path, Some(row.host), Some(&token));
        for (name, value) in &row.headers {
            request.headers_mut().insert(
                http::HeaderName::from_static(name),
                http::HeaderValue::from_str(value).expect("the test built a header value"),
            );
        }
        let response = portal.dispatch(request).await;
        if row.claimed {
            assert_success(row.case, &response, Success::AccountList);
        } else {
            assert_error(
                row.case,
                &response,
                (StatusCode::NOT_FOUND, "UnknownService"),
            );
        }
    }
}
