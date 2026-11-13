// SPDX-License-Identifier: CC0-1.0

//! How this service recognises one of its own four bound routes, over the URI FORMS the
//! same request can arrive in.
//!
//! RULE: a request's path is the path component of its URI — no scheme, no authority, no
//! query string — so the one (method, path) signature that attributes an anonymous portal
//! request to this service is the SAME signature its refusal wording looks a route's
//! `@required` http-bound members up by. Whichever form the URI arrived in — origin form
//! with no authority at all, or absolute form carrying a scheme and a host — a request
//! for one of the four `@http` bindings is claimed AND reworded by both halves, and a
//! path no binding names is recognised as a route by neither.
//!
//! Each test sweeps the four bindings across the three forms a client can address them
//! in, so a row cannot be satisfied by one spelling of the URI: the two halves of the
//! service's routing have to agree on all of them.

use crate::portal_session::{body_json, body_text, error_code};
use bytes::Bytes;
use http::{Method, Request, Response, StatusCode};
use service_com_amazonaws_sso_swbportalservice::state::{
    self, ComAmazonawsSsoSwbportalserviceState,
};
use service_com_amazonaws_sso_swbportalservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;
use std::time::Duration;

/// The header the model binds the access token to (`OPERATIONS.md`).
const ACCESS_TOKEN_HEADER: &str = "x-amz-sso_bearer_token";

/// How long the seeded sign-in sessions last. The sign-in session length is a directory
/// setting in IAM Identity Center, so the fixture picks one rather than the service.
const SESSION_TTL: Duration = Duration::from_secs(8 * 60 * 60);

/// The one account the fixture assigns and the one role assigned in it, so a served row
/// is served out of state.
const ACCOUNT_ID: &str = "111111111111";
const ACCOUNT_NAME: &str = "Production";
const ACCOUNT_EMAIL: &str = "production@example.com";
const ROLE_NAME: &str = "AdministratorAccess";

/// The modelled portal endpoint (the model's `awsService` endpoint prefix plus a region
/// and the partition's DNS suffix), and an endpoint that names no service at all.
const PORTAL_HOST: &str = "portal.sso.us-east-1.amazonaws.com";
const ANONYMOUS_HOST: &str = "localhost:9297";

/// How a client spells the same request's URI on the wire.
#[derive(Debug, Clone, Copy)]
enum Form {
    /// Origin form: the URI is the path and query alone, with no authority and no `Host`
    /// header, so nothing in the request names a service and it is attributed by its own
    /// (method, path) signature.
    Origin,
    /// Absolute form against an endpoint that names no service: attributed by the same
    /// signature, but the URI now carries a scheme and an authority ahead of the path.
    AbsoluteAnonymous,
    /// Absolute form against the modelled portal endpoint, which the host does name.
    AbsolutePortal,
}

/// Every form a row is swept over.
const FORMS: &[Form] = &[Form::Origin, Form::AbsoluteAnonymous, Form::AbsolutePortal];

impl Form {
    /// The endpoint host this form addresses, or `None` for the origin form, which names
    /// none.
    fn host(self) -> Option<&'static str> {
        match self {
            Form::Origin => None,
            Form::AbsoluteAnonymous => Some(ANONYMOUS_HOST),
            Form::AbsolutePortal => Some(PORTAL_HOST),
        }
    }

    /// A request in this form: `path_and_query` is the origin-form spelling the test
    /// writes, and the absolute forms put the host in front of it.
    fn request(self, method: &Method, path_and_query: &str, token: Option<&str>) -> Request<Bytes> {
        let mut builder = Request::builder().method(method.clone());
        builder = match self.host() {
            Some(host) => builder
                .uri(format!("https://{host}{path_and_query}"))
                .header("host", host),
            None => builder.uri(path_and_query),
        };
        if let Some(token) = token {
            builder = builder.header(ACCESS_TOKEN_HEADER, token);
        }
        builder
            .body(Bytes::new())
            .expect("the test built a well-formed request")
    }

    /// What a request whose path names no binding must be answered with in this form: the
    /// router's own `UnknownService` where nothing claims it, and — where the host claims
    /// it for this service — the framework's own routing answer, since a path that is no
    /// route of this service's gets none of its wording.
    fn unrecognised(self) -> (StatusCode, &'static str) {
        match self {
            Form::Origin | Form::AbsoluteAnonymous => (StatusCode::NOT_FOUND, "UnknownService"),
            Form::AbsolutePortal => (StatusCode::NOT_IMPLEMENTED, "NotImplemented"),
        }
    }
}

/// One `@http` binding, with everything a row needs to drive both halves of the routing
/// through it. The methods, paths and query parameters are the bindings in
/// `OPERATIONS.md`.
struct Route {
    /// The operation the binding names, for the case label.
    operation: &'static str,
    method: Method,
    /// The binding's path.
    path: &'static str,
    /// Every `@required` query parameter of the route, present.
    complete_query: &'static str,
    /// The output member a 200 must report and a string that answer must carry — a value
    /// out of seeded state, or (for the credential the emulator mints, which echoes
    /// nothing seeded) the member the minted key id rides in. `None` is `Logout`, which
    /// models no output members.
    reports: Option<(&'static str, &'static str)>,
    /// One row per `@required` QUERY parameter: the parameter dropped, and the query that
    /// is left. The access-token header is not a row here — it is the caller's
    /// credential, and its absence is the 401 every row below asserts anyway.
    without: &'static [(&'static str, &'static str)],
}

impl Route {
    /// This route's origin-form URI with `query` as its query string.
    fn uri(&self, query: &str) -> String {
        if query.is_empty() {
            self.path.to_string()
        } else {
            format!("{}?{}", self.path, query)
        }
    }
}

fn routes() -> Vec<Route> {
    vec![
        Route {
            operation: "ListAccounts",
            method: Method::GET,
            path: "/assignment/accounts",
            complete_query: "",
            reports: Some(("accountList", ACCOUNT_ID)),
            without: &[],
        },
        Route {
            operation: "ListAccountRoles",
            method: Method::GET,
            path: "/assignment/roles",
            complete_query: "account_id=111111111111",
            reports: Some(("roleList", ROLE_NAME)),
            without: &[("account_id", "")],
        },
        Route {
            operation: "GetRoleCredentials",
            method: Method::GET,
            path: "/federation/credentials",
            complete_query: "account_id=111111111111&role_name=AdministratorAccess",
            reports: Some(("roleCredentials", "accessKeyId")),
            without: &[
                ("account_id", "role_name=AdministratorAccess"),
                ("role_name", "account_id=111111111111"),
            ],
        },
        Route {
            operation: "Logout",
            method: Method::POST,
            path: "/logout",
            complete_query: "",
            reports: None,
            without: &[],
        },
    ]
}

/// One emulator holding this service, plus the handle its state is seeded through.
///
/// This suite builds the requests itself (the URI form is what it varies), so it holds
/// the router rather than going through another suite's fixed-endpoint helper.
struct Portal {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Portal {
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

/// Assert a response is a failure with this status and this wire error code.
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

/// RULE (the serving half, over every URI form): each binding's (method, path) signature
/// is recognised whichever form the URI takes, so a complete request with a live session
/// is served out of state.
///
/// RULE (the wording half, over the same forms): a request that omits a `@required`
/// http-bound member matches no operation, so the service has to recognise its route
/// itself to word the refusal — the omitted credential is the modelled 401 and an omitted
/// parameter from an authenticated caller is the modelled 400, never the framework's
/// routing answer.
#[tokio::test]
async fn each_bound_route_is_recognised_in_every_uri_form() {
    let portal = Portal::new();

    for form in FORMS {
        for route in routes() {
            // A fresh session per row: `Logout` invalidates the one it is given, so a
            // shared token would make one row's answer depend on another's.
            let case = format!("{form:?} / {}", route.operation);
            let token = format!("tok-{case}");
            portal.sign_in(&token);
            let served = portal
                .dispatch(form.request(
                    &route.method,
                    &route.uri(route.complete_query),
                    Some(&token),
                ))
                .await;
            assert_eq!(
                served.status(),
                StatusCode::OK,
                "case {case} complete: expected 200, got {} {:?}",
                served.status(),
                body_text(&served)
            );
            match route.reports {
                Some((member, carried)) => {
                    let payload = body_json(&served);
                    let reported = &payload[member];
                    let body = body_text(&served);
                    assert!(
                        !reported.is_null() && body.contains(carried),
                        "case {case} complete: expected {member} carrying {carried:?}, got \
                         {body:?}"
                    );
                }
                None => {
                    let body = body_text(&served);
                    assert!(
                        body.is_empty() || body_json(&served) == serde_json::json!({}),
                        "case {case} complete: this operation models no output members, got \
                         {body:?}"
                    );
                }
            }

            // The credential absent. The router selects no route without it, so the
            // service's own route recognition is what answers — the portal's 401.
            let unauthenticated = portal
                .dispatch(form.request(&route.method, &route.uri(route.complete_query), None))
                .await;
            assert_error(
                &format!("{case} without a credential"),
                &unauthenticated,
                (StatusCode::UNAUTHORIZED, "UnauthorizedException"),
            );

            // One required query parameter absent, from an authenticated caller: the same
            // recognition, worded as the modelled invalid request.
            for (parameter, remaining) in route.without {
                let case = format!("{case} without {parameter}");
                let token = format!("tok-{case}");
                portal.sign_in(&token);
                let response = portal
                    .dispatch(form.request(&route.method, &route.uri(remaining), Some(&token)))
                    .await;
                assert_error(
                    &case,
                    &response,
                    (StatusCode::BAD_REQUEST, "InvalidRequestException"),
                );
            }
        }
    }
}

/// RULE (the converse half): the path a request names is the path component of its URI
/// alone and is matched exactly, so a path no binding names is recognised as a route by
/// neither half — in every URI form, and whether the bound path appears under another
/// method, under a deeper segment, in another case, or only inside the query string.
#[tokio::test]
async fn a_path_no_binding_names_is_recognised_as_none_in_every_uri_form() {
    let portal = Portal::new();
    portal.sign_in("tok-live");

    // `(case, method, origin-form path and query)`. Every row carries a live session, so
    // a 401 or a 400 here would mean a path was taken for a route it is not.
    let cases: &[(&str, Method, &str)] = &[
        ("a path no binding names", Method::GET, "/assignment/nope"),
        (
            "a bound path under a deeper segment",
            Method::GET,
            "/assignment/accounts/extra",
        ),
        (
            "a bound path in another case",
            Method::GET,
            "/Assignment/Accounts",
        ),
        (
            "a bound path only inside the query string",
            Method::GET,
            "/assignment/nope?next=/assignment/accounts",
        ),
        (
            "a bound path under a method it does not bind",
            Method::POST,
            "/assignment/accounts",
        ),
        (
            "another bound path under a method it does not bind",
            Method::GET,
            "/logout",
        ),
    ];

    for form in FORMS {
        for (case, method, path_and_query) in cases {
            let response = portal
                .dispatch(form.request(method, path_and_query, Some("tok-live")))
                .await;
            assert_error(
                &format!("{form:?} / {case}"),
                &response,
                form.unrecognised(),
            );
        }
    }
}
