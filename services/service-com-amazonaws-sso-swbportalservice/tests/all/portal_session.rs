// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the IAM Identity Center portal: the four operations over HTTP,
//! against state seeded the way an administrator's assignments and a `CreateToken` call
//! would have seeded it.
//!
//! Each test asserts a RULE over a table of inputs rather than one recorded case:
//!
//! * a listing reports exactly the assignments held in state, ordered by key;
//! * a page is bounded by `max_result` and its cursor resumes immediately after the
//!   last item it returned (swept over every item count × page size in range);
//! * a role listing is scoped to one account, and an account the presented session is
//!   not assigned to is reported as absent;
//! * role credentials are minted by the emulator, stored, and expire in the future;
//! * `Logout` invalidates the session in state, so every later call is unauthorized;
//! * an unusable token is 401, and an authenticated but unreadable request is 400, on
//!   every operation.

use bytes::Bytes;
use http::{Method, Request, Response, StatusCode};
use service_com_amazonaws_sso_swbportalservice::state::{
    self, ComAmazonawsSsoSwbportalserviceState, ROLE_SESSION_DURATION,
};
use service_com_amazonaws_sso_swbportalservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// The portal endpoint an SDK addresses. Every portal operation is `@optionalAuth` with
/// an empty `@auth`, so the request carries no SigV4 scope and the service claims it by
/// this host.
const PORTAL_HOST: &str = "portal.sso.us-east-1.amazonaws.com";

/// How long the seeded sign-in sessions last. The sign-in session length is a directory
/// setting in IAM Identity Center, so the fixture picks one rather than the service.
const SESSION_TTL: Duration = Duration::from_secs(8 * 60 * 60);

/// The accounts the fixture holds and the roles it assigns the signed-in user in them:
/// `(account id, name, email, roles)`.
///
/// Rows differ along everything a listing generalises over — how many roles the user is
/// assigned in an account (none, one, several), where its id sorts, and the shape of a
/// role name — so no assertion below can be satisfied by a single canned answer.
const FIXTURE: &[(&str, &str, &str, &[&str])] = &[
    (
        "111111111111",
        "Production",
        "production@example.com",
        &["AdministratorAccess", "ReadOnly"],
    ),
    ("222222222222", "Staging", "staging@example.com", &["ReadOnly"]),
    ("333333333333", "Sandbox", "sandbox@example.com", &[]),
    (
        "444444444444",
        "Audit",
        "audit@example.com",
        &["Audit-Role_1", "ReadOnly", "ZZZ-Break-Glass"],
    ),
];

/// Every account id in [`FIXTURE`].
fn every_account() -> Vec<&'static str> {
    FIXTURE.iter().map(|(id, ..)| *id).collect()
}

/// The roles [`FIXTURE`] assigns in `account_id`, ascending — the order a role listing
/// must report them in.
pub(crate) fn roles_of(account_id: &str) -> Vec<String> {
    let mut roles: Vec<String> = FIXTURE
        .iter()
        .find(|(id, ..)| *id == account_id)
        .map(|(.., roles)| roles.iter().map(|role| (*role).to_string()).collect())
        .unwrap_or_default();
    roles.sort();
    roles
}

/// One emulator holding this service, plus the handle its state is seeded through.
pub(crate) struct Portal {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Portal {
    /// An emulator with [`FIXTURE`]'s accounts seeded and no sessions.
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        let portal = Self { engine, router };
        {
            let store = portal.state();
            let mut store = store.lock();
            for (account_id, name, email, _) in FIXTURE {
                store.put_account(*account_id, *name, *email);
            }
        }
        portal
    }

    /// The same store the handlers reach: the seam an administrator's assignments and a
    /// sign-in arrive through, since the portal models no create API.
    pub(crate) fn state(&self) -> engine::ServiceState<ComAmazonawsSsoSwbportalserviceState> {
        state::service_state(&*self.engine).expect("the portal store is reachable")
    }

    /// Seed an active session for `access_token`, assigned to `accounts` with the roles
    /// [`FIXTURE`] assigns its user in each of them.
    pub(crate) fn sign_in(&self, access_token: &str, accounts: &[&str]) {
        let assignments: Vec<(&str, Vec<String>)> = accounts
            .iter()
            .map(|account_id| (*account_id, roles_of(account_id)))
            .collect();
        self.sign_in_assigned(access_token, assignments);
    }

    /// Seed an active session for `access_token` holding exactly `assignments`:
    /// `(account id, the roles assigned to THIS session's user in that account)`.
    ///
    /// The roles are a property of the (user, account) assignment, so a suite that needs
    /// two sessions to differ in them — or accounts outside [`FIXTURE`] — seeds them here
    /// rather than on the account.
    pub(crate) fn sign_in_assigned<A, I, R, S>(&self, access_token: &str, assignments: A)
    where
        A: IntoIterator<Item = (I, R)>,
        I: Into<String>,
        R: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.state().lock().put_session(
            access_token,
            format!("{access_token}@example.com"),
            assignments,
            SESSION_TTL,
        );
    }

    async fn send(&self, method: Method, uri: &str, token: Option<&str>) -> Response<Bytes> {
        let mut builder = Request::builder()
            .method(method)
            .uri(format!("https://{PORTAL_HOST}{uri}"))
            .header("host", PORTAL_HOST);
        if let Some(token) = token {
            builder = builder.header("x-amz-sso_bearer_token", token);
        }
        let request = builder
            .body(Bytes::new())
            .expect("the test built a well-formed request");
        self.router.dispatch(request).await
    }

    pub(crate) async fn get(&self, uri: &str, token: Option<&str>) -> Response<Bytes> {
        self.send(Method::GET, uri, token).await
    }

    pub(crate) async fn logout(&self, token: Option<&str>) -> Response<Bytes> {
        self.send(Method::POST, "/logout", token).await
    }

    /// Dispatch a request the CALLER built, for a suite whose subject is the shape of the
    /// request itself — the endpoint host it is addressed to, say — rather than the
    /// operation it names. The helpers above address [`PORTAL_HOST`]; this is the same
    /// router and the same seeded state, handed a request nothing here spelled.
    pub(crate) async fn dispatch(&self, request: Request<Bytes>) -> Response<Bytes> {
        self.router.dispatch(request).await
    }
}

pub(crate) fn body_text(response: &Response<Bytes>) -> String {
    String::from_utf8_lossy(response.body()).to_string()
}

pub(crate) fn body_json(response: &Response<Bytes>) -> serde_json::Value {
    serde_json::from_slice(response.body()).unwrap_or_else(|error| {
        panic!(
            "the response body must be JSON ({error}), got {:?}",
            body_text(response)
        )
    })
}

/// The wire error code (`__type`) of a failure response.
pub(crate) fn error_code(response: &Response<Bytes>) -> String {
    body_json(response)["__type"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The `message` an error envelope carries, or the empty string when it carries none.
pub(crate) fn error_message(response: &Response<Bytes>) -> String {
    body_json(response)["message"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The `id_key` of every item under `items_key`, in the order reported.
fn item_ids(response: &Response<Bytes>, items_key: &str, id_key: &str) -> Vec<String> {
    let json = body_json(response);
    let items = json.get(items_key).and_then(serde_json::Value::as_array);
    items
        .map(|items| {
            items
                .iter()
                .map(|item| item[id_key].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Walk every page of a listing with `max_result=page_size`, asserting the page bound
/// and the terminal page as it goes, and answer the ids seen in order.
async fn walk_pages(
    portal: &Portal,
    path: &str,
    token: &str,
    items_key: &str,
    id_key: &str,
    page_size: usize,
    case: &str,
) -> Vec<String> {
    let separator = if path.contains('?') { '&' } else { '?' };
    let mut seen: Vec<String> = Vec::new();
    let mut next: Option<String> = None;
    // A bound, so a cursor that fails to advance fails the test instead of looping.
    for page in 0..64_u32 {
        let mut uri = format!("{path}{separator}max_result={page_size}");
        if let Some(token) = &next {
            uri.push_str(&format!("&next_token={token}"));
        }
        let response = portal.get(&uri, Some(token)).await;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "case {case:?}: page {page} of {uri:?} must succeed, got {} {:?}",
            response.status(),
            body_text(&response)
        );
        let items = item_ids(&response, items_key, id_key);
        assert!(
            items.len() <= page_size,
            "case {case:?}: page {page} returned {} items, more than max_result={page_size}: {items:?}",
            items.len()
        );
        seen.extend(items.iter().cloned());
        next = body_json(&response)["nextToken"]
            .as_str()
            .map(str::to_string);
        match next {
            // The last page carries no cursor: that is how a client knows to stop.
            None => return seen,
            Some(_) => assert!(
                !items.is_empty(),
                "case {case:?}: page {page} carried a nextToken but no items, so the \
                 cursor cannot advance (seen so far: {seen:?})"
            ),
        }
    }
    panic!("case {case:?}: the listing did not terminate within 64 pages: {seen:?}")
}

/// RULE: `ListAccounts` reports exactly the accounts state records for the presented
/// session, ascending by account id, with the name and email held in state.
#[tokio::test]
async fn list_accounts_reports_exactly_the_sessions_assignments() {
    // Rows differ in how many accounts a session is assigned to (none, one, several,
    // all) and include an assignment naming an account the service holds no record of,
    // which there is nothing to report for.
    let cases: &[(&str, &[&str])] = &[
        ("tok-none", &[]),
        ("tok-one", &["222222222222"]),
        ("tok-two", &["444444444444", "111111111111"]),
        ("tok-all", &["111111111111", "222222222222", "333333333333", "444444444444"]),
        ("tok-unknown-account", &["999999999999", "333333333333"]),
    ];

    let portal = Portal::new();
    for (token, assigned) in cases {
        portal.sign_in(token, assigned);
        let response = portal.get("/assignment/accounts", Some(token)).await;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "case {token:?}: expected 200, got {} {:?}",
            response.status(),
            body_text(&response)
        );

        let mut expected: Vec<&str> = assigned
            .iter()
            .copied()
            .filter(|id| every_account().contains(id))
            .collect();
        expected.sort_unstable();
        let reported = item_ids(&response, "accountList", "accountId");
        assert_eq!(
            reported, expected,
            "case {token:?}: expected accountList {expected:?}, got {reported:?}"
        );
        assert!(
            body_json(&response).get("nextToken").is_none(),
            "case {token:?}: a complete listing must omit nextToken, got {:?}",
            body_text(&response)
        );

        // Every reported account carries the name and email held in state, not a value
        // derived from the request.
        let json = body_json(&response);
        let reported_accounts = json["accountList"].as_array().cloned().unwrap_or_default();
        for account in reported_accounts {
            let id = account["accountId"].as_str().unwrap_or_default();
            let (_, name, email, _) = FIXTURE
                .iter()
                .find(|(fixture_id, ..)| *fixture_id == id)
                .unwrap_or_else(|| panic!("case {token:?}: unseeded account {id:?} reported"));
            assert_eq!(
                (
                    account["accountName"].as_str().unwrap_or_default(),
                    account["emailAddress"].as_str().unwrap_or_default()
                ),
                (*name, *email),
                "case {token:?}: account {id:?} must report the seeded name and email, got {account:?}"
            );
        }
    }
}

/// RULE: a page holds at most `maxResults` items and its `nextToken` resumes
/// immediately after the last item returned, so walking the pages yields the whole
/// listing exactly once and the last page omits the token.
///
/// Swept over every (item count, page size) pair in range rather than sampled: the
/// account count spans 0..=4 and the page size spans 1..=5 (one past the longest
/// listing), which is the whole domain the rule ranges over for this fixture.
#[tokio::test]
async fn paging_a_listing_yields_every_item_exactly_once() {
    let portal = Portal::new();
    let accounts = every_account();

    for count in 0..=accounts.len() {
        let assigned = &accounts[..count];
        let token = format!("tok-accounts-{count}");
        portal.sign_in(&token, assigned);
        let mut expected: Vec<String> = assigned.iter().map(|id| (*id).to_string()).collect();
        expected.sort();

        for page_size in 1..=accounts.len() + 1 {
            let case = format!("{count} accounts, max_result={page_size}");
            let seen = walk_pages(
                &portal,
                "/assignment/accounts",
                &token,
                "accountList",
                "accountId",
                page_size,
                &case,
            )
            .await;
            assert_eq!(
                seen, expected,
                "case {case:?}: paging must yield {expected:?} exactly once, got {seen:?}"
            );
        }
    }

    // The same rule over the other paginated listing, whose pages are roles.
    portal.sign_in("tok-roles", &accounts);
    for account_id in accounts {
        let expected = roles_of(account_id);
        for page_size in 1..=expected.len() + 1 {
            let case = format!("roles of {account_id}, max_result={page_size}");
            let seen = walk_pages(
                &portal,
                &format!("/assignment/roles?account_id={account_id}"),
                "tok-roles",
                "roleList",
                "roleName",
                page_size,
                &case,
            )
            .await;
            assert_eq!(
                seen, expected,
                "case {case:?}: paging must yield {expected:?} exactly once, got {seen:?}"
            );
        }
    }
}

/// RULE: `ListAccountRoles` reports the roles assigned in ONE account for the presented
/// session, echoing that account id; an account the session is not assigned to is
/// reported as absent (404), and a cursor from another account's listing is not a
/// cursor for this one (400).
#[tokio::test]
async fn list_account_roles_is_scoped_to_one_visible_account() {
    let portal = Portal::new();
    portal.sign_in("tok-two", &["111111111111", "333333333333"]);

    for account_id in ["111111111111", "333333333333"] {
        let response = portal
            .get(
                &format!("/assignment/roles?account_id={account_id}"),
                Some("tok-two"),
            )
            .await;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "case {account_id:?}: expected 200, got {} {:?}",
            response.status(),
            body_text(&response)
        );
        let reported = item_ids(&response, "roleList", "roleName");
        assert_eq!(
            reported,
            roles_of(account_id),
            "case {account_id:?}: expected roleList {:?}, got {reported:?}",
            roles_of(account_id)
        );
        let json = body_json(&response);
        for role in json["roleList"].as_array().cloned().unwrap_or_default() {
            assert_eq!(
                role["accountId"].as_str().unwrap_or_default(),
                account_id,
                "case {account_id:?}: every RoleInfo must carry its own account, got {role:?}"
            );
        }
    }

    // Rows: an account id nothing is seeded for, and one that IS seeded but is not
    // assigned to this session. Neither exists as far as this session is concerned.
    for account_id in ["999999999999", "222222222222", "444444444444"] {
        let response = portal
            .get(
                &format!("/assignment/roles?account_id={account_id}"),
                Some("tok-two"),
            )
            .await;
        assert_eq!(
            (response.status(), error_code(&response).as_str()),
            (StatusCode::NOT_FOUND, "ResourceNotFoundException"),
            "case {account_id:?}: an account this session cannot see must be 404 \
             ResourceNotFoundException, got {} {:?}",
            response.status(),
            body_text(&response)
        );
    }

    // A cursor is scoped to the listing that issued it.
    let first = portal
        .get(
            "/assignment/roles?account_id=111111111111&max_result=1",
            Some("tok-two"),
        )
        .await;
    let cursor = body_json(&first)["nextToken"]
        .as_str()
        .expect("a truncated role listing carries a nextToken")
        .to_string();
    let response = portal
        .get(
            &format!("/assignment/roles?account_id=333333333333&next_token={cursor}"),
            Some("tok-two"),
        )
        .await;
    assert_eq!(
        (response.status(), error_code(&response).as_str()),
        (StatusCode::BAD_REQUEST, "InvalidRequestException"),
        "case {cursor:?}: another account's cursor must be refused, got {} {:?}",
        response.status(),
        body_text(&response)
    );
}

/// RULE: `GetRoleCredentials` answers an assigned (account, role) with credentials the
/// emulator minted and stored, expiring in the future; it answers a pair the session is
/// not assigned with 404. Asking twice inside the lifetime returns the stored
/// credential, and distinct pairs get distinct credentials.
#[tokio::test]
async fn get_role_credentials_mints_stores_and_reuses_a_future_credential() {
    let portal = Portal::new();
    let token = "tok-all";
    portal.sign_in(token, &every_account());

    let mut issued: Vec<(String, String)> = Vec::new();
    for (account_id, .., roles) in FIXTURE {
        for role_name in roles.iter() {
            let case = format!("{account_id}/{role_name}");
            let before = state::epoch_millis(SystemTime::now());
            let response = portal
                .get(
                    &format!(
                        "/federation/credentials?account_id={account_id}&role_name={role_name}"
                    ),
                    Some(token),
                )
                .await;
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "case {case:?}: expected 200, got {} {:?}",
                response.status(),
                body_text(&response)
            );
            let credentials = body_json(&response)["roleCredentials"].clone();
            let access_key_id = credentials["accessKeyId"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let secret_access_key = credentials["secretAccessKey"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let session_token = credentials["sessionToken"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let expiration = credentials["expiration"].as_i64().unwrap_or_default();
            assert!(
                !access_key_id.is_empty() && !secret_access_key.is_empty() && !session_token.is_empty(),
                "case {case:?}: every credential member must be present, got {credentials:?}"
            );
            let after = state::epoch_millis(SystemTime::now());
            let lifetime = i64::try_from(ROLE_SESSION_DURATION.as_millis()).unwrap_or(i64::MAX);
            assert!(
                expiration > after && expiration >= before + lifetime && expiration <= after + lifetime,
                "case {case:?}: expiration must be the issue time plus the role session \
                 duration ({lifetime} ms), so inside ({}, {}], got {expiration}",
                before + lifetime,
                after + lifetime
            );

            // Read it back out of state: the response was computed from the record, not
            // from the request.
            let key = (
                token.to_string(),
                (*account_id).to_string(),
                (*role_name).to_string(),
            );
            let stored = portal
                .state()
                .lock()
                .issued_credentials
                .get(&key)
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: no credential was stored for {key:?}"));
            assert_eq!(
                (
                    stored.access_key_id.as_str(),
                    stored.secret_access_key.as_str(),
                    stored.session_token.as_str(),
                    stored.expiration_millis
                ),
                (
                    access_key_id.as_str(),
                    secret_access_key.as_str(),
                    session_token.as_str(),
                    expiration
                ),
                "case {case:?}: the response must be the stored record, got {credentials:?}"
            );

            // Asking again inside the lifetime answers the SAME stored credential.
            let again = portal
                .get(
                    &format!(
                        "/federation/credentials?account_id={account_id}&role_name={role_name}"
                    ),
                    Some(token),
                )
                .await;
            let again = body_json(&again)["roleCredentials"].clone();
            assert_eq!(
                (
                    again["accessKeyId"].as_str().unwrap_or_default(),
                    again["expiration"].as_i64().unwrap_or_default()
                ),
                (access_key_id.as_str(), expiration),
                "case {case:?}: a second fetch inside the lifetime must answer the stored \
                 credential, got {again:?}"
            );
            issued.push((case, access_key_id));
        }
    }

    for (index, (case, access_key_id)) in issued.iter().enumerate() {
        for (other_case, other) in &issued[index + 1..] {
            assert_ne!(
                access_key_id, other,
                "cases {case:?} and {other_case:?}: distinct role sessions must get \
                 distinct credentials, both got {access_key_id:?}"
            );
        }
    }

    // Rows: a role not assigned in that account, a role assigned only in ANOTHER
    // account, and an account this session cannot see.
    let missing: &[(&str, &str)] = &[
        ("111111111111", "NoSuchRole"),
        ("222222222222", "AdministratorAccess"),
        ("333333333333", "ReadOnly"),
        ("999999999999", "ReadOnly"),
    ];
    for (account_id, role_name) in missing {
        let response = portal
            .get(
                &format!("/federation/credentials?account_id={account_id}&role_name={role_name}"),
                Some(token),
            )
            .await;
        assert_eq!(
            (response.status(), error_code(&response).as_str()),
            (StatusCode::NOT_FOUND, "ResourceNotFoundException"),
            "case {account_id}/{role_name}: an unassigned role must be 404 \
             ResourceNotFoundException, got {} {:?}",
            response.status(),
            body_text(&response)
        );
    }
}

/// RULE: `Logout` invalidates the sign-in session in state, so every later call
/// presenting that token is unauthorized — and only that token's session is affected.
#[tokio::test]
async fn logout_invalidates_the_session_for_every_operation() {
    let portal = Portal::new();
    portal.sign_in("tok-out", &["111111111111"]);
    portal.sign_in("tok-stays", &["111111111111"]);

    // It worked before the logout, so the 401s below are the logout's doing.
    let before = portal.get("/assignment/accounts", Some("tok-out")).await;
    assert_eq!(
        before.status(),
        StatusCode::OK,
        "the seeded session must list accounts before logout, got {} {:?}",
        before.status(),
        body_text(&before)
    );

    let logout = portal.logout(Some("tok-out")).await;
    assert_eq!(
        logout.status(),
        StatusCode::OK,
        "Logout must answer 200, got {} {:?}",
        logout.status(),
        body_text(&logout)
    );
    assert_eq!(
        body_text(&logout),
        "",
        "Logout models no output, so its 200 must carry an empty body, got {:?}",
        body_text(&logout)
    );
    assert!(
        !portal
            .state()
            .lock()
            .sessions
            .get("tok-out")
            .expect("the session record is kept, deactivated")
            .active,
        "Logout must deactivate the session in state"
    );

    // Every read operation, plus a second Logout, is now unauthorized for that token.
    let after: &[(&str, &str)] = &[
        ("ListAccounts", "/assignment/accounts"),
        ("ListAccountRoles", "/assignment/roles?account_id=111111111111"),
        (
            "GetRoleCredentials",
            "/federation/credentials?account_id=111111111111&role_name=AdministratorAccess",
        ),
    ];
    for (operation, uri) in after {
        let response = portal.get(uri, Some("tok-out")).await;
        assert_eq!(
            (response.status(), error_code(&response).as_str()),
            (StatusCode::UNAUTHORIZED, "UnauthorizedException"),
            "case {operation:?}: a logged-out token must be 401 UnauthorizedException, \
             got {} {:?}",
            response.status(),
            body_text(&response)
        );

        // The other session is untouched.
        let other = portal.get(uri, Some("tok-stays")).await;
        assert_eq!(
            other.status(),
            StatusCode::OK,
            "case {operation:?}: another session must be unaffected by the logout, got \
             {} {:?}",
            other.status(),
            body_text(&other)
        );
    }

    let twice = portal.logout(Some("tok-out")).await;
    assert_eq!(
        (twice.status(), error_code(&twice).as_str()),
        (StatusCode::UNAUTHORIZED, "UnauthorizedException"),
        "logging out twice must be 401 UnauthorizedException, got {} {:?}",
        twice.status(),
        body_text(&twice)
    );
}

/// RULE: a token that names no usable session — unknown, or expired — is 401 on every
/// operation.
#[tokio::test]
async fn an_unusable_access_token_is_unauthorized_everywhere() {
    let portal = Portal::new();
    // An expired session: `put_session` dates the expiry from now, so a zero ttl is
    // already past without any waiting.
    portal.state().lock().put_session(
        "tok-expired",
        "expired@example.com",
        [("111111111111", roles_of("111111111111"))],
        Duration::ZERO,
    );
    portal.sign_in("tok-live", &["111111111111"]);

    let uris: &[(&str, &str)] = &[
        ("ListAccounts", "/assignment/accounts"),
        ("ListAccountRoles", "/assignment/roles?account_id=111111111111"),
        (
            "GetRoleCredentials",
            "/federation/credentials?account_id=111111111111&role_name=AdministratorAccess",
        ),
    ];
    // Rows: never issued, empty, whitespace, a token-shaped string, and an expired one.
    let tokens = ["tok-unknown", "", " ", "aoak_v1-not-a-token", "tok-expired"];
    for (operation, uri) in uris {
        for token in tokens {
            let response = portal.get(uri, Some(token)).await;
            assert_eq!(
                (response.status(), error_code(&response).as_str()),
                (StatusCode::UNAUTHORIZED, "UnauthorizedException"),
                "case {operation:?} with token {token:?}: expected 401 \
                 UnauthorizedException, got {} {:?}",
                response.status(),
                body_text(&response)
            );
        }
        let logged_out = portal.logout(Some("tok-unknown")).await;
        assert_eq!(
            (logged_out.status(), error_code(&logged_out).as_str()),
            (StatusCode::UNAUTHORIZED, "UnauthorizedException"),
            "case Logout with an unknown token: expected 401 UnauthorizedException, got \
             {} {:?}",
            logged_out.status(),
            body_text(&logged_out)
        );
        // The live session still works, so the 401s above are about the token.
        let live = portal.get(uri, Some("tok-live")).await;
        assert_eq!(
            live.status(),
            StatusCode::OK,
            "case {operation:?}: the live session must still succeed, got {} {:?}",
            live.status(),
            body_text(&live)
        );
    }
}

/// RULE: the service claims a request addressed to its endpoint host OR one signed with
/// its signing name, since its operations are modeled `@optionalAuth` with an empty
/// `@auth` and so arrive unsigned.
#[tokio::test]
async fn the_portal_claims_both_unsigned_and_signed_requests() {
    let portal = Portal::new();
    portal.sign_in("tok-live", &["111111111111"]);

    // Row 1: unsigned, addressed to the endpoint host (what an SDK sends).
    let unsigned = portal.get("/assignment/accounts", Some("tok-live")).await;
    assert_eq!(
        unsigned.status(),
        StatusCode::OK,
        "case unsigned: expected 200, got {} {:?}",
        unsigned.status(),
        body_text(&unsigned)
    );

    // Row 2: signed with this service's SigV4 signing name, addressed to a host that
    // names no service at all.
    let signed = Request::builder()
        .method(Method::GET)
        .uri("https://localhost:9297/assignment/accounts")
        .header("host", "localhost:9297")
        .header("x-amz-sso_bearer_token", "tok-live")
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=EMULATORAKEXAMPLE/20260101/us-east-1/awsssoportal/aws4_request, \
             SignedHeaders=host, Signature=0",
        )
        .body(Bytes::new())
        .expect("the test built a well-formed request");
    let signed = portal.router.dispatch(signed).await;
    assert_eq!(
        signed.status(),
        StatusCode::OK,
        "case signed: expected 200, got {} {:?}",
        signed.status(),
        body_text(&signed)
    );

    // Row 3: a path this service does not model stays a routing failure — the
    // missing-parameter wording above is scoped to the routes it does serve.
    let unmodeled = portal.get("/assignment/nope", Some("tok-live")).await;
    assert_eq!(
        (unmodeled.status(), error_code(&unmodeled).as_str()),
        (StatusCode::NOT_IMPLEMENTED, "NotImplemented"),
        "case unmodeled path: expected the framework's routing answer, got {} {:?}",
        unmodeled.status(),
        body_text(&unmodeled)
    );
}

/// What an invalid-input row expects back.
enum Expect {
    /// Inside every modeled rule, so the request is served.
    Served,
    /// Refused with HTTP 400 and this error code.
    Refused(&'static str),
    /// Refused with HTTP 400 by the codec, before any rule of this service applies, so
    /// the code is the framework's `ValidationException` rather than a modeled one.
    RefusedByTheCodec,
}

/// RULE: a request from an AUTHENTICATED caller that this service cannot read is 400
/// `InvalidRequestException` — an absent required parameter, a `maxResults` outside the
/// modeled 1..100 range, or a `nextToken` it did not issue. (A request presenting no
/// usable session never gets this far: it is 401, whatever else it carries.)
#[tokio::test]
async fn an_unreadable_request_is_an_invalid_request() {
    let portal = Portal::new();
    portal.sign_in("tok-live", &every_account());

    // `max_result` cases: each published bound, one past each bound, and a value that is
    // not an integer at all. The range comes from the model's `MaxResultType`
    // (range 1..100).
    let max_results: &[(&str, Expect)] = &[
        ("1", Expect::Served),
        ("100", Expect::Served),
        ("0", Expect::Refused("InvalidRequestException")),
        ("101", Expect::Refused("InvalidRequestException")),
        ("-1", Expect::Refused("InvalidRequestException")),
        // Not an integer, so the request never becomes a value with a range to check:
        // the codec refuses it (400) before any rule of this service's applies.
        ("not-a-number", Expect::RefusedByTheCodec),
    ];
    for (value, expect) in max_results {
        let response = portal
            .get(
                &format!("/assignment/accounts?max_result={value}"),
                Some("tok-live"),
            )
            .await;
        match expect {
            Expect::Served => assert_eq!(
                response.status(),
                StatusCode::OK,
                "case max_result={value:?}: inside the modeled range, expected 200, got \
                 {} {:?}",
                response.status(),
                body_text(&response)
            ),
            Expect::Refused(code) => assert_eq!(
                (response.status(), error_code(&response).as_str()),
                (StatusCode::BAD_REQUEST, *code),
                "case max_result={value:?}: outside the modeled range, expected 400 \
                 {code}, got {} {:?}",
                response.status(),
                body_text(&response)
            ),
            Expect::RefusedByTheCodec => assert_eq!(
                response.status(),
                StatusCode::BAD_REQUEST,
                "case max_result={value:?}: unreadable as an integer, expected 400, got \
                 {} {:?}",
                response.status(),
                body_text(&response)
            ),
        }
    }

    // Absent required members, for an AUTHENTICATED caller: one row for every (route,
    // `@required` http-bound member) pair this service's model declares except the
    // access-token header, which is the caller's credential and not a parameter — a
    // request that presents no session is unauthenticated whatever else it omits, which
    // `bearer_token_gate.rs` pins over every route. A request omitting a parameter
    // matches no route at all, so this is also the guard that the service's
    // required-member table still agrees with the model.
    let absent: &[(&str, &str, Option<&str>)] = &[
        (
            "ListAccountRoles without an account",
            "/assignment/roles",
            Some("tok-live"),
        ),
        (
            "GetRoleCredentials without an account",
            "/federation/credentials?role_name=ReadOnly",
            Some("tok-live"),
        ),
        (
            "GetRoleCredentials without a role",
            "/federation/credentials?account_id=111111111111",
            Some("tok-live"),
        ),
    ];
    for (case, uri, token) in absent {
        let response = portal.get(uri, *token).await;
        assert_eq!(
            (response.status(), error_code(&response).as_str()),
            (StatusCode::BAD_REQUEST, "InvalidRequestException"),
            "case {case:?}: expected 400 InvalidRequestException, got {} {:?}",
            response.status(),
            body_text(&response)
        );
    }

    // Cursors this service did not issue for THIS listing: not hex, hex of something
    // else, a cursor for the other listing, and a cursor issued to another session.
    let roles_cursor = {
        let first = portal
            .get(
                "/assignment/roles?account_id=111111111111&max_result=1",
                Some("tok-live"),
            )
            .await;
        body_json(&first)["nextToken"]
            .as_str()
            .expect("a truncated role listing carries a nextToken")
            .to_string()
    };
    portal.sign_in("tok-other", &every_account());
    let other_sessions_cursor = {
        let first = portal
            .get("/assignment/accounts?max_result=1", Some("tok-other"))
            .await;
        body_json(&first)["nextToken"]
            .as_str()
            .expect("a truncated account listing carries a nextToken")
            .to_string()
    };
    let cursors = [
        "not-hex".to_string(),
        hex::encode("v1:accounts"),
        hex::encode("v0:accounts:111111111111"),
        roles_cursor,
        other_sessions_cursor,
    ];
    for cursor in cursors {
        let response = portal
            .get(
                &format!("/assignment/accounts?next_token={cursor}"),
                Some("tok-live"),
            )
            .await;
        assert_eq!(
            (response.status(), error_code(&response).as_str()),
            (StatusCode::BAD_REQUEST, "InvalidRequestException"),
            "case next_token={cursor:?}: expected 400 InvalidRequestException, got {} {:?}",
            response.status(),
            body_text(&response)
        );
    }
}
