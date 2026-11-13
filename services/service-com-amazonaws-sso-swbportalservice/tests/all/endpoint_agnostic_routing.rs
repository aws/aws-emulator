// SPDX-License-Identifier: CC0-1.0

//! Which requests this service CLAIMS, over the endpoints a client can be pointed at.
//!
//! RULE: every portal operation is `@optionalAuth` with an empty `@auth` and the
//! protocol is `restJson1`, so a portal request carries no SigV4 credential scope and
//! no `X-Amz-Target`; when the host it is addressed to names no service either — which
//! every host that is not a service's published endpoint does, an ordinary operator
//! hostname included, whatever routing key the framework's host-style tier takes off its
//! first label — the request's own modelled signature — the (method, path) pair of one of
//! the four `@http` bindings — is what attributes it to this service. So each of the four
//! operations answers its modelled result (its own 200 from state, or 401
//! `UnauthorizedException` for a token that names no usable session) whatever endpoint
//! host the client was pointed at, including none at all.
//!
//! The converse half of the same rule is pinned here too: a request that DOES name
//! another service (a credential scope, an `X-Amz-Target`, or another service's
//! endpoint host), or that names a (method, path) pair this model does not bind, is NOT
//! claimed — it stays the router's own `UnknownService`, so nothing is swallowed by
//! claiming on the path.

use bytes::Bytes;
use http::{Method, Request, Response, StatusCode};
use service_com_amazonaws_sso_swbportalservice::state::{
    self, ComAmazonawsSsoSwbportalserviceState,
};
use service_com_amazonaws_sso_swbportalservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;
use std::time::Duration;

/// How long the seeded sign-in sessions last. The sign-in session length is a directory
/// setting in IAM Identity Center, so the fixture picks one rather than the service.
const SESSION_TTL: Duration = Duration::from_secs(8 * 60 * 60);

/// The one account the fixture assigns, and the one role assigned in it.
const ACCOUNT_ID: &str = "111111111111";
const ACCOUNT_NAME: &str = "Production";
const ACCOUNT_EMAIL: &str = "production@example.com";
const ROLE_NAME: &str = "AdministratorAccess";

/// The endpoints a client can be pointed at, as `(case, Host)`. Rows span what the
/// host-style routing tier can make of a host: the ones it resolves NO key from (a
/// loopback name with and without a port, a loopback IP, a private IP, no `Host` header
/// at all — an origin-form request with no authority), the ordinary hostnames it DOES
/// resolve a key from while that key names no service (a single-label compose service
/// name, a container hostname, a corporate DNS name, a reverse proxy), and — the one row
/// that does name this service — the real portal endpoint. Only the last names a service,
/// which is exactly the case the rule covers.
const ENDPOINTS: &[(&str, Option<&str>)] = &[
    ("the emulator's own loopback endpoint", Some("localhost:9297")),
    ("a loopback name with no port", Some("localhost")),
    ("a loopback IP with a port", Some("127.0.0.1:9297")),
    ("a private IP endpoint", Some("10.0.0.5:8080")),
    ("no Host header at all", None),
    ("a single-label compose service name", Some("aws:9297")),
    ("a container hostname", Some("host.docker.internal:9297")),
    ("a corporate DNS name", Some("emulator.corp.local:9297")),
    ("a reverse proxy in front of the emulator", Some("gateway.internal")),
    (
        "the modelled portal endpoint",
        Some("portal.sso.us-east-1.amazonaws.com"),
    ),
];

/// What a 200 from one route must report, so a row cannot be satisfied by an empty
/// answer: each variant is checked against the state the fixture seeded.
#[derive(Debug, Clone, Copy)]
enum Success {
    /// `ListAccounts` reports the seeded account, with the name and email in state.
    AccountList,
    /// `ListAccountRoles` reports the seeded role, echoing the account it is in.
    RoleList,
    /// `GetRoleCredentials` reports credentials the emulator minted.
    Credentials,
    /// `Logout` models no output members, so its 200 reports nothing — the state change
    /// is what it does, and `portal_session.rs` pins that.
    NoOutput,
}

/// The four routes the model binds, with every `@required` http-bound parameter present:
/// `(case, method, path and query, what a 200 reports)`. The (method, path) halves are
/// the `@http` bindings in `OPERATIONS.md`.
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

/// Access tokens that name no usable sign-in session. Rows differ along the shapes a
/// client can present — never issued, empty, whitespace, token-shaped, JWT-shaped and
/// oversized — because the rule is about the session the token names, not about the
/// token's form. A non-ASCII value is NOT a row: `accessToken` is bound to an HTTP
/// header, which cannot carry one, so no client can present it.
fn unusable_tokens() -> Vec<String> {
    vec![
        "a-token-that-was-never-issued".to_string(),
        String::new(),
        " ".to_string(),
        "aoak_v1-not-a-token".to_string(),
        "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJub2JvZHkifQ.c2lnbmF0dXJl".to_string(),
        "t".repeat(4096),
    ]
}

/// One emulator holding this service, plus the handle its state is seeded through.
struct Portal {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Portal {
    /// An emulator with the one fixture account seeded and no sessions.
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        let portal = Self { engine, router };
        portal
            .state()
            .lock()
            .put_account(ACCOUNT_ID, ACCOUNT_NAME, ACCOUNT_EMAIL);
        portal
    }

    /// The same store the handlers reach: the seam an administrator's assignments and a
    /// sign-in arrive through, since the portal models no create API.
    fn state(&self) -> engine::ServiceState<ComAmazonawsSsoSwbportalserviceState> {
        state::service_state(&*self.engine).expect("the portal store is reachable")
    }

    /// Seed an active session for `access_token`, assigned the fixture role in the
    /// fixture account.
    fn sign_in(&self, access_token: &str) {
        self.state().lock().put_session(
            access_token,
            "someone@example.com",
            [(ACCOUNT_ID, [ROLE_NAME])],
            SESSION_TTL,
        );
    }

    async fn dispatch(&self, request: Request<Bytes>) -> Response<Bytes> {
        self.router.dispatch(request).await
    }
}

/// A portal request: `host` is the endpoint the client was pointed at (`None` sends an
/// origin-form request with no `Host` header and no authority), `token` rides in the
/// modelled `x-amz-sso_bearer_token` header, and `extra` adds the headers a row needs
/// to name another service.
fn portal_request(
    method: &Method,
    path: &str,
    host: Option<&str>,
    token: Option<&str>,
    extra: &[(&str, &str)],
) -> Request<Bytes> {
    let mut builder = Request::builder().method(method.clone());
    builder = match host {
        Some(host) => builder
            .uri(format!("https://{host}{path}"))
            .header("host", host),
        None => builder.uri(path),
    };
    if let Some(token) = token {
        builder = builder.header("x-amz-sso_bearer_token", token);
    }
    for (name, value) in extra {
        builder = builder.header(*name, *value);
    }
    builder
        .body(Bytes::new())
        .expect("the test built a well-formed request")
}

fn body_text(response: &Response<Bytes>) -> String {
    String::from_utf8_lossy(response.body()).to_string()
}

fn body_json(response: &Response<Bytes>) -> serde_json::Value {
    serde_json::from_slice(response.body()).unwrap_or(serde_json::Value::Null)
}

/// The wire error code (`__type`) of a failure response, which is also how the router
/// words its own refusals.
fn error_code(response: &Response<Bytes>) -> String {
    body_json(response)["__type"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Assert a 200 reports what `expected` says the route reports out of state.
fn assert_success(case: &str, response: &Response<Bytes>, expected: Success) {
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "case {case}: expected 200, got {} {:?}",
        response.status(),
        body_text(response)
    );
    let payload = body_json(response);
    match expected {
        Success::AccountList => {
            let accounts = payload["accountList"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let reported: Vec<(String, String, String)> = accounts
                .iter()
                .map(|account| {
                    (
                        account["accountId"].as_str().unwrap_or_default().to_string(),
                        account["accountName"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
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
                "case {case}: expected the seeded account, got {reported:?}"
            );
        }
        Success::RoleList => {
            let roles = payload["roleList"].as_array().cloned().unwrap_or_default();
            let reported: Vec<(String, String)> = roles
                .iter()
                .map(|role| {
                    (
                        role["roleName"].as_str().unwrap_or_default().to_string(),
                        role["accountId"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect();
            assert_eq!(
                reported,
                vec![(ROLE_NAME.to_string(), ACCOUNT_ID.to_string())],
                "case {case}: expected the seeded role, got {reported:?}"
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
                        .is_some_and(|token| !token.is_empty())
                    && credentials["expiration"].as_i64().unwrap_or_default() > 0,
                "case {case}: expected minted role credentials, got {:?}",
                body_text(response)
            );
        }
        Success::NoOutput => {
            let body = body_text(response);
            assert!(
                body.is_empty() || payload == serde_json::json!({}),
                "case {case}: Logout models no output members, got {body:?}"
            );
        }
    }
}

/// One row of the "not claimed here" table: a request named by its method and path,
/// the endpoint it is addressed to, and the headers that make it somebody else's.
type ForeignRequest = (
    &'static str,
    Method,
    &'static str,
    Option<&'static str>,
    &'static [(&'static str, &'static str)],
);

/// Assert a response is the modelled 401 for a token that names no usable session.
fn assert_unauthorized(case: &str, response: &Response<Bytes>) {
    assert_eq!(
        (response.status(), error_code(response).as_str()),
        (StatusCode::UNAUTHORIZED, "UnauthorizedException"),
        "case {case}: expected 401 UnauthorizedException, got {} {:?}",
        response.status(),
        body_text(response)
    );
}

/// RULE (the serving half): each of the four routes is attributed to this service by its
/// own (method, path) signature on ANY endpoint — so a live session is served and an
/// unusable token is the modelled 401, whatever host the client was pointed at.
#[tokio::test]
async fn every_route_is_served_on_an_endpoint_that_names_no_service() {
    let portal = Portal::new();
    let tokens = unusable_tokens();

    for (endpoint, host) in ENDPOINTS {
        for (operation, method, path, expected) in ROUTES {
            // A fresh session per row: `Logout` invalidates the one it is given, so a
            // shared token would make the next row's answer depend on this one.
            let live = format!("tok-live-{endpoint}-{operation}");
            portal.sign_in(&live);
            let served = portal
                .dispatch(portal_request(method, path, *host, Some(&live), &[]))
                .await;
            assert_success(&format!("{endpoint} / {operation}"), &served, *expected);

            for token in &tokens {
                let refused = portal
                    .dispatch(portal_request(method, path, *host, Some(token), &[]))
                    .await;
                assert_unauthorized(
                    &format!("{endpoint} / {operation} with token {token:?}"),
                    &refused,
                );
            }
        }
    }
}

/// RULE: the token gate answers before any other parameter is read, so an odd — unicode,
/// injection-shaped, or boundary — parameter value on a request whose token names no
/// session is still the modelled 401, never a 404, a 500 or a leaked answer.
#[tokio::test]
async fn an_unusable_token_is_refused_whatever_the_other_parameters_say() {
    let portal = Portal::new();
    // Rows: a role name and account id carrying percent-encoded non-ASCII, path
    // traversal and SQL/metacharacter shapes; an account id the fixture does not hold;
    // and each published bound of the modelled `MaxResultType` range (1..100) on the
    // listings that take it.
    let cases: &[(&str, Method, &str)] = &[
        (
            "a percent-encoded unicode role name",
            Method::GET,
            "/federation/credentials?account_id=111111111111&role_name=r%C3%B4le%E4%B8%AD",
        ),
        (
            "a path-traversal role name",
            Method::GET,
            "/federation/credentials?account_id=111111111111&role_name=..%2F..%2Fetc%2Fpasswd",
        ),
        (
            "a quote-bearing account id",
            Method::GET,
            "/federation/credentials?account_id=%27%20OR%201%3D1&role_name=AdministratorAccess",
        ),
        (
            "an account id the service does not hold",
            Method::GET,
            "/assignment/roles?account_id=999999999999",
        ),
        (
            "max_result at the bottom of the modelled range",
            Method::GET,
            "/assignment/accounts?max_result=1",
        ),
        (
            "max_result at the top of the modelled range",
            Method::GET,
            "/assignment/accounts?max_result=100",
        ),
        (
            "a next_token this service never issued",
            Method::GET,
            "/assignment/accounts?next_token=not-a-cursor",
        ),
    ];

    for (case, method, path) in cases {
        let response = portal
            .dispatch(portal_request(
                method,
                path,
                Some("localhost:9297"),
                Some("a-token-that-was-never-issued"),
                &[],
            ))
            .await;
        assert_unauthorized(case, &response);
    }
}

/// RULE: a refused request reports the error only — a 401 from `GetRoleCredentials`
/// carries no `roleCredentials` payload and none of its secrets.
#[tokio::test]
async fn an_unauthorized_error_carries_no_credentials_payload() {
    let portal = Portal::new();

    for token in unusable_tokens() {
        let response = portal
            .dispatch(portal_request(
                &Method::GET,
                "/federation/credentials?account_id=111111111111&role_name=AdministratorAccess",
                Some("127.0.0.1:9297"),
                Some(&token),
                &[],
            ))
            .await;
        assert_unauthorized(&format!("token {token:?}"), &response);
        let body = body_text(&response);
        for leaked in [
            "roleCredentials",
            "accessKeyId",
            "secretAccessKey",
            "sessionToken",
        ] {
            assert!(
                !body.contains(leaked),
                "case token {token:?}: a refusal must not carry {leaked:?}, got {body:?}"
            );
        }
    }
}

/// RULE: the gate is per-request, so concurrent requests presenting an unusable token
/// are ALL refused — none of them slips through while another is being served.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_requests_with_an_unusable_token_are_all_refused() {
    let portal = Portal::new();
    portal.sign_in("tok-live");

    let in_flight = 10;
    let mut calls = Vec::with_capacity(in_flight);
    for index in 0..in_flight {
        let router = portal.router.clone();
        // Half the calls present a live session alongside: an answer must depend on the
        // token the request itself carries, not on what a concurrent request carried.
        let token = if index % 2 == 0 {
            "a-token-that-was-never-issued".to_string()
        } else {
            "tok-live".to_string()
        };
        calls.push(tokio::spawn(async move {
            let response = router
                .dispatch(portal_request(
                    &Method::GET,
                    "/federation/credentials?account_id=111111111111&role_name=AdministratorAccess",
                    Some("localhost:9297"),
                    Some(&token),
                    &[],
                ))
                .await;
            (index, token, response)
        }));
    }

    for call in calls {
        let (index, token, response) = call.await.expect("the dispatch task completed");
        let case = format!("concurrent call {index} with token {token:?}");
        if token == "tok-live" {
            assert_success(&case, &response, Success::Credentials);
        } else {
            assert_unauthorized(&case, &response);
        }
    }
}

/// RULE (the converse half): a request that names ANOTHER service, or a (method, path)
/// pair this model does not bind, is not claimed here — the router answers its own
/// `404 UnknownService`.
#[tokio::test]
async fn a_request_for_another_service_or_an_unmodelled_route_is_not_claimed() {
    let portal = Portal::new();
    portal.sign_in("tok-live");

    // `(case, method, path, host, extra headers)`.
    let cases: &[ForeignRequest] = &[
        (
            "a SigV4 credential scope naming another service",
            Method::GET,
            "/assignment/accounts",
            Some("localhost:9297"),
            &[(
                "authorization",
                "AWS4-HMAC-SHA256 \
                 Credential=EMULATORAKEXAMPLE/20260101/us-east-1/iam/aws4_request, \
                 SignedHeaders=host, Signature=0",
            )],
        ),
        (
            "an X-Amz-Target naming another service",
            Method::POST,
            "/logout",
            Some("localhost:9297"),
            &[("x-amz-target", "DynamoDB_20120810.GetItem")],
        ),
        (
            "another service's endpoint host",
            Method::GET,
            "/assignment/accounts",
            Some("iam.amazonaws.com"),
            &[],
        ),
        (
            "another service's endpoint host in another partition",
            Method::GET,
            "/assignment/accounts",
            Some("dynamodb.cn-north-1.amazonaws.com.cn"),
            &[],
        ),
        (
            "another service's dual-stack endpoint host",
            Method::POST,
            "/logout",
            Some("lambda.us-east-1.api.aws"),
            &[],
        ),
        (
            // A host that merely STARTS with this endpoint's prefix is not this
            // endpoint, so it claims no path of its own: it is an ordinary hostname,
            // served on the routes this model binds (`endpoint_host_claiming.rs`) and
            // nowhere else — where the real portal endpoint answers its own routing
            // failure for an unmodelled path, this one reaches no service at all.
            "a host that merely starts with this endpoint's prefix, on a path this model \
             does not bind",
            Method::GET,
            "/assignment/nope",
            Some("portal.ssoother.example.com"),
            &[],
        ),
        (
            "a path this model does not bind",
            Method::GET,
            "/assignment/nope",
            Some("localhost:9297"),
            &[],
        ),
        (
            "a modelled path under a method it does not bind",
            Method::GET,
            "/logout",
            Some("localhost:9297"),
            &[],
        ),
        (
            "a modelled path under another method it does not bind",
            Method::POST,
            "/assignment/accounts",
            Some("localhost:9297"),
            &[],
        ),
    ];

    for (case, method, path, host, extra) in cases {
        let response = portal
            .dispatch(portal_request(method, path, *host, Some("tok-live"), extra))
            .await;
        assert_eq!(
            (response.status(), error_code(&response).as_str()),
            (StatusCode::NOT_FOUND, "UnknownService"),
            "case {case:?}: expected the router's 404 UnknownService, got {} {:?}",
            response.status(),
            body_text(&response)
        );
    }
}

/// RULE: claiming by the route's (method, path) — not by the bearer-token header — keeps
/// the modelled answer for an AUTHENTICATED request that omits a `@required` http-bound
/// PARAMETER on an endpoint that names no service: one row per (route, required
/// parameter) the model declares. The access-token header is the caller's credential
/// rather than a parameter, so its absence is the 401 in `bearer_token_gate.rs`.
#[tokio::test]
async fn an_absent_required_member_is_an_invalid_request_on_any_endpoint() {
    let portal = Portal::new();
    portal.sign_in("tok-live");

    // `(case, method, path, token)`: each row drops exactly one required parameter.
    let cases: &[(&str, Method, &str, Option<&str>)] = &[
        (
            "ListAccountRoles without accountId",
            Method::GET,
            "/assignment/roles",
            Some("tok-live"),
        ),
        (
            "GetRoleCredentials without accountId",
            Method::GET,
            "/federation/credentials?role_name=AdministratorAccess",
            Some("tok-live"),
        ),
        (
            "GetRoleCredentials without roleName",
            Method::GET,
            "/federation/credentials?account_id=111111111111",
            Some("tok-live"),
        ),
    ];

    for (case, method, path, token) in cases {
        let response = portal
            .dispatch(portal_request(
                method,
                path,
                Some("localhost:9297"),
                *token,
                &[],
            ))
            .await;
        assert_eq!(
            (response.status(), error_code(&response).as_str()),
            (StatusCode::BAD_REQUEST, "InvalidRequestException"),
            "case {case:?}: expected 400 InvalidRequestException, got {} {:?}",
            response.status(),
            body_text(&response)
        );
    }
}
