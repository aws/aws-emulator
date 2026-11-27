// SPDX-License-Identifier: CC0-1.0

//! The workspace's own (native) Grafana HTTP API, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, the
//! data plane, the persisted state and serialization — the same path a client
//! takes.
//!
//! The rule under test: the `endpoint` the control plane reports for a workspace
//! is an address THIS emulator is reached at — the bare `host[:port]` the
//! control-plane request arrived on, with no scheme and no path — and this emulator
//! serves that workspace's Grafana HTTP API there: a request spliced onto the
//! announced endpoint carrying `Authorization: Bearer <credential the control plane
//! minted for that workspace>` is answered from the workspace record, and a
//! credential the workspace no longer holds (deleted, expired, or belonging to a
//! deleted workspace) authenticates nothing.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_grafana_awsgrafanacontrolplane::state::LIFECYCLE_TRANSITION;
use service_com_amazonaws_grafana_awsgrafanacontrolplane::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for, which is the region the workspaces
/// they create live in — and so the region label of the endpoint host each
/// announces.
const REGION: &str = "eu-west-1";

/// The endpoint a workspace is announced at for a client that reached this emulator at
/// `authority`: that authority itself, spelled out here independently of the crate's own
/// constants so a wrong composition in `src/state/` fails.
fn expected_endpoint(authority: &str) -> String {
    authority.to_string()
}

/// The endpoint PATH this emulator also answers a workspace's Grafana API at — the
/// addressing for a client that names the workspace in the address rather than through the
/// credential it presents.
fn workspace_path_endpoint(authority: &str, id: &str) -> String {
    format!("{authority}/_aws/grafana/{id}")
}

/// The host Amazon Managed Grafana itself publishes for the workspace `id`: not the
/// announced endpoint (nothing resolves that name to this process), but an address this
/// emulator still ANSWERS for a client whose own resolver points it here.
fn aws_published_host(id: &str) -> String {
    format!("{id}.grafana-workspace.{REGION}.amazonaws.com")
}

/// The `Host` header and the request path a client builds to reach `route` under the
/// `endpoint` the control plane announced — the splice `https://<endpoint><route>` performed
/// on the parts an HTTP request carries them in. A bare host carries no path, so there the
/// route is the whole path.
fn addressed(endpoint: &str, route: &str) -> (String, String) {
    match endpoint.split_once('/') {
        Some((host, path)) => (host.to_string(), format!("/{path}{route}")),
        None if route.is_empty() => (endpoint.to_string(), "/".to_string()),
        None => (endpoint.to_string(), route.to_string()),
    }
}

/// How long a case waits for something the clock owes (a status, a key's expiry)
/// before giving up. Far above every span below, so a loaded host still reaches it;
/// nothing asserts on how long it actually took.
const CLOCK_DEADLINE: Duration = LIFECYCLE_TRANSITION.saturating_mul(20);

/// How often such a wait looks again.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// A SigV4 credential scope naming this service; it is what routes a control-plane
/// request here. The data-plane requests below carry no SigV4 credential at all —
/// a Grafana client has none.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/grafana/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The endpoint host most cases reach this emulator on: the loopback address and
/// the port the router binds by default.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// The published bounds of `secondsToLive` on both credential-minting operations
/// (`@range(min: 1, max: 2592000)`). Source: `smithy_query(query="operation",
/// operation="CreateWorkspaceApiKey,CreateWorkspaceServiceAccountToken")`.
const MIN_SECONDS_TO_LIVE: i64 = 1;
const MAX_SECONDS_TO_LIVE: i64 = 2_592_000;

/// The emulator under test.
struct Emulator {
    router: MultiServiceRouter,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    Emulator { router }
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    fn string(&self, member: &str) -> String {
        self.body[member].as_str().unwrap_or_default().to_string()
    }
}

/// One request through the router, with an explicit `Host` and `Authorization`.
async fn request(
    emulator: &Emulator,
    method: &str,
    uri: &str,
    host: &str,
    authorization: Option<&str>,
    body: Option<Value>,
) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder().method(method).uri(uri);
    if !host.is_empty() {
        builder = builder.header("host", host);
    }
    if let Some(authorization) = authorization {
        builder = builder.header("authorization", authorization);
    }
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// A control-plane call, signed for this service and reached on `host`.
async fn control(
    emulator: &Emulator,
    method: &str,
    uri: &str,
    host: &str,
    body: Option<Value>,
) -> Answer {
    request(
        emulator,
        method,
        uri,
        host,
        Some(SIGNED_FOR_EU_WEST_1),
        body,
    )
    .await
}

/// A call to the workspace's own Grafana HTTP API: `route` spliced onto the
/// `endpoint` the control plane announced, reached the way a Grafana client reaches
/// it — no SigV4 credential, only the bearer `authorization` given.
async fn native(
    emulator: &Emulator,
    endpoint: &str,
    route: &str,
    authorization: Option<&str>,
) -> Answer {
    let (host, path) = addressed(endpoint, route);
    request(emulator, "GET", &path, &host, authorization, None).await
}

/// Look again at `route` under `endpoint` until it answers `want`, or the deadline
/// passes: the wait for something the clock owes, with nothing asserted about how
/// long it took.
async fn await_native_status(
    emulator: &Emulator,
    endpoint: &str,
    route: &str,
    authorization: Option<&str>,
    want: u16,
    what: &str,
) -> Answer {
    let deadline = Instant::now() + CLOCK_DEADLINE;
    loop {
        let answered = native(emulator, endpoint, route, authorization).await;
        if answered.status == want {
            return answered;
        }
        assert!(
            Instant::now() < deadline,
            "{what}: expected HTTP {want} from the workspace's Grafana API, last read {} \
             with {:?}",
            answered.status,
            answered.body
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// A workspace reached on `host`, named `name` when one is given, plus its
/// announced endpoint. The wait is the one the clock owes a new workspace: it is
/// `CREATING` until the transition falls due, so this looks again until the status
/// is there.
async fn active_workspace(
    emulator: &Emulator,
    host: &str,
    name: Option<&str>,
    grafana_version: Option<&str>,
) -> (String, String) {
    let mut body = json!({
        "accountAccessType": "CURRENT_ACCOUNT",
        "permissionType": "SERVICE_MANAGED",
        "authenticationProviders": ["SAML"],
    });
    if let Some(name) = name {
        body["workspaceName"] = json!(name);
    }
    if let Some(version) = grafana_version {
        body["grafanaVersion"] = json!(version);
    }
    let created = control(emulator, "POST", "/workspaces", host, Some(body)).await;
    assert_eq!(
        created.status, 202,
        "the workspace this case needs must be created, got {} with {:?}",
        created.status, created.body
    );
    let id = created.body["workspace"]["id"]
        .as_str()
        .expect("CreateWorkspace answers an id")
        .to_string();
    let endpoint = created.body["workspace"]["endpoint"]
        .as_str()
        .expect("CreateWorkspace answers an endpoint")
        .to_string();

    let described = await_active(emulator, host, &id).await;
    assert_eq!(
        described.body["workspace"]["endpoint"]
            .as_str()
            .unwrap_or_default(),
        endpoint,
        "DescribeWorkspace must report the endpoint the create announced"
    );
    (id, endpoint)
}

/// Look again at the workspace `id` until it is ACTIVE, or the deadline passes.
async fn await_active(emulator: &Emulator, host: &str, id: &str) -> Answer {
    let deadline = Instant::now() + CLOCK_DEADLINE;
    loop {
        let described = control(emulator, "GET", &format!("/workspaces/{id}"), host, None).await;
        if described.body["workspace"]["status"] == json!("ACTIVE") {
            return described;
        }
        assert!(
            Instant::now() < deadline,
            "workspace {id} never reached ACTIVE, last read {} with {:?}",
            described.status,
            described.body
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// A workspace API key, as `CreateWorkspaceApiKey` minted it.
async fn api_key(
    emulator: &Emulator,
    host: &str,
    workspace_id: &str,
    key_name: &str,
    key_role: &str,
    seconds_to_live: i64,
) -> String {
    let created = control(
        emulator,
        "POST",
        &format!("/workspaces/{workspace_id}/apikeys"),
        host,
        Some(json!({
            "keyName": key_name,
            "keyRole": key_role,
            "secondsToLive": seconds_to_live,
        })),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "CreateWorkspaceApiKey must mint the key this case needs, got {} with {:?}",
        created.status, created.body
    );
    created.string("key")
}

/// The `Authorization` header a client presents a bearer credential in.
fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

/// Every addressing of a workspace reaches its Grafana API: the endpoint the control plane
/// ANNOUNCES — the bare address this emulator is reached at, where the credential the
/// request presents names the workspace — the workspace's own path beneath that address,
/// and the host Amazon Managed Grafana itself publishes for the workspace, which this
/// emulator also answers for a client whose own resolver points it here.
///
/// The rows differ along the addressing, and each is exercised with a credential the
/// control plane minted for that workspace, so an address that reached the router's own
/// refusal instead of the Grafana API fails here. The two addressings that name the
/// workspace in the ADDRESS are exercised with no credential at all as well: there the
/// workspace is known without one, so the Grafana API itself refuses the request.
#[tokio::test]
async fn every_addressing_of_a_workspace_reaches_its_grafana_api() {
    let emulator = emulator();
    let (id, endpoint) = active_workspace(&emulator, EMULATOR_HOST, Some("addressing"), None).await;
    let expected = expected_endpoint(EMULATOR_HOST);
    assert_eq!(
        endpoint, expected,
        "the announced endpoint must be the address this emulator was reached at, expected \
         {expected:?}, got {endpoint:?}"
    );
    let key = api_key(&emulator, EMULATOR_HOST, &id, "addressing", "ADMIN", 3_600).await;

    for (case, address, names_the_workspace) in [
        ("the announced endpoint", endpoint.clone(), false),
        (
            "the workspace's path beneath it",
            workspace_path_endpoint(EMULATOR_HOST, &id),
            true,
        ),
        (
            "the host Amazon Managed Grafana publishes",
            aws_published_host(&id),
            true,
        ),
    ] {
        let org = native(&emulator, &address, "/api/org", Some(&bearer(&key))).await;
        assert_eq!(
            org.status, 200,
            "case {case:?}: {address} must serve the workspace's Grafana API, got {} with \
             {:?}",
            org.status, org.body
        );
        assert_eq!(
            org.body["id"],
            json!(1),
            "case {case:?}: the organization must report its id, got {:?}",
            org.body
        );

        // And where the ADDRESS names the workspace, an unauthenticated request is refused
        // by the workspace's Grafana API rather than by the router, which is what an
        // address naming nothing this emulator serves would answer.
        if !names_the_workspace {
            continue;
        }
        let answered = native(&emulator, &address, "/api/org", None).await;
        assert_eq!(
            answered.status, 401,
            "case {case:?}: an unauthenticated request must be refused by the workspace's \
             Grafana API, got {} with {:?}",
            answered.status, answered.body
        );
        assert_eq!(
            answered.error_code(),
            "",
            "case {case:?}: the refusal must come from the Grafana API, not the router, \
             got {:?}",
            answered.body
        );
    }
}

/// One row of the API-key table: a workspace to mint the key on, and the key.
struct KeyCase {
    case: &'static str,
    workspace_name: Option<&'static str>,
    grafana_version: Option<&'static str>,
    key_name: &'static str,
    key_role: &'static str,
    seconds_to_live: i64,
}

/// A key `CreateWorkspaceApiKey` minted authenticates at the workspace endpoint
/// until `DeleteWorkspaceApiKey` removes it, and the organization it reports is
/// read off the workspace record.
///
/// The rows differ along what the rule generalises over: every `keyRole` the
/// documented set carries, both published `secondsToLive` bounds and a value
/// between them, a named and an unnamed workspace (so the reported organization
/// name comes from two different places), and a pinned Grafana version.
#[tokio::test]
async fn a_minted_api_key_authenticates_at_the_endpoint_until_it_is_deleted() {
    let cases = [
        KeyCase {
            case: "an admin key on a named workspace, one hour",
            workspace_name: Some("observability"),
            grafana_version: None,
            key_name: "dashboards",
            key_role: "ADMIN",
            seconds_to_live: 3_600,
        },
        KeyCase {
            case: "an editor key on an unnamed workspace, the longest lifetime",
            workspace_name: None,
            grafana_version: None,
            key_name: "etl",
            key_role: "EDITOR",
            seconds_to_live: MAX_SECONDS_TO_LIVE,
        },
        KeyCase {
            case: "a viewer key on a workspace pinned to Grafana 8.4",
            workspace_name: Some("legacy"),
            grafana_version: Some("8.4"),
            key_name: "read-only",
            key_role: "VIEWER",
            seconds_to_live: 600,
        },
        KeyCase {
            case: "a punctuated key name on a workspace with a punctuated name",
            workspace_name: Some("team.one_two-three"),
            grafana_version: Some("9.4"),
            key_name: "Team.Key_1-ci",
            key_role: "ADMIN",
            seconds_to_live: 86_400,
        },
        KeyCase {
            case: "a tilde-and-dot workspace name, an editor key",
            workspace_name: Some("team~obs.2026"),
            grafana_version: Some("10.4"),
            key_name: "key-2026",
            key_role: "EDITOR",
            seconds_to_live: 120,
        },
    ];

    for KeyCase {
        case,
        workspace_name: name,
        grafana_version: version,
        key_name,
        key_role,
        seconds_to_live,
    } in cases
    {
        let emulator = emulator();
        let (id, endpoint) = active_workspace(&emulator, EMULATOR_HOST, name, version).await;
        let key = api_key(
            &emulator,
            EMULATOR_HOST,
            &id,
            key_name,
            key_role,
            seconds_to_live,
        )
        .await;

        let org = native(&emulator, &endpoint, "/api/org", Some(&bearer(&key))).await;
        assert_eq!(
            org.status, 200,
            "case {case:?}: the fresh key must authenticate at {endpoint}, got {} with {:?}",
            org.status, org.body
        );
        assert_eq!(
            org.body["id"],
            json!(1),
            "case {case:?}: the organization must report its id, got {:?}",
            org.body
        );
        assert_eq!(
            org.string("name"),
            name.unwrap_or(&id),
            "case {case:?}: the organization name must be read off the workspace record, \
             got {:?}",
            org.body
        );

        // Revocation is observable at the destination: the deleted key stops
        // working, and the second delete is refused by the control plane.
        let deleted = control(
            &emulator,
            "DELETE",
            &format!("/workspaces/{id}/apikeys/{key_name}"),
            EMULATOR_HOST,
            None,
        )
        .await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: DeleteWorkspaceApiKey must remove the key, got {} with {:?}",
            deleted.status, deleted.body
        );

        let revoked = native(&emulator, &endpoint, "/api/org", Some(&bearer(&key))).await;
        assert_eq!(
            revoked.status, 401,
            "case {case:?}: the deleted key must no longer authenticate, got {} with {:?}",
            revoked.status, revoked.body
        );

        let again = control(
            &emulator,
            "DELETE",
            &format!("/workspaces/{id}/apikeys/{key_name}"),
            EMULATOR_HOST,
            None,
        )
        .await;
        assert_eq!(
            (again.status, again.error_code()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: deleting the removed key again must answer a 404 \
             ResourceNotFoundException, got {} with {:?}",
            again.status,
            again.body
        );
    }
}

/// Where the ADDRESS names the workspace, only a live bearer credential of THAT workspace
/// is accepted; everything else is refused with 401 and the organization is not reported.
///
/// The rows differ along the form of the `Authorization` header (absent, empty,
/// scheme-only, another scheme, a token with no scheme, a lower-cased scheme) and
/// along whose credential it carries (this workspace's key, another workspace's
/// key, a token the emulator never minted).
#[tokio::test]
async fn only_a_live_bearer_credential_of_that_workspace_is_accepted() {
    let emulator = emulator();
    let (id, _) = active_workspace(&emulator, EMULATOR_HOST, Some("mine"), None).await;
    let (other_id, _) = active_workspace(&emulator, EMULATOR_HOST, Some("theirs"), None).await;
    let mine = api_key(&emulator, EMULATOR_HOST, &id, "mine", "ADMIN", 3_600).await;
    let theirs = api_key(&emulator, EMULATOR_HOST, &other_id, "theirs", "ADMIN", 3_600).await;
    // The addressing that names the workspace itself, so a request carrying no usable
    // credential is still known to address this workspace and is refused by its Grafana API.
    let addressed = workspace_path_endpoint(EMULATOR_HOST, &id);

    let cases: Vec<(&str, Option<String>, u16)> = vec![
        ("no Authorization header at all", None, 401),
        ("an empty Authorization header", Some(String::new()), 401),
        ("the scheme with no token", Some("Bearer".to_string()), 401),
        ("the scheme with an empty token", Some("Bearer ".to_string()), 401),
        ("another authentication scheme", Some(format!("Basic {mine}")), 401),
        ("the token with no scheme", Some(mine.clone()), 401),
        (
            "a SigV4 Authorization header",
            Some(SIGNED_FOR_EU_WEST_1.to_string()),
            401,
        ),
        (
            "a token this emulator never minted",
            Some(bearer("not-a-token-this-emulator-minted")),
            401,
        ),
        (
            "another workspace's live key",
            Some(bearer(&theirs)),
            401,
        ),
        ("this workspace's live key", Some(bearer(&mine)), 200),
        (
            "this workspace's live key, lower-cased scheme",
            Some(format!("bearer {mine}")),
            200,
        ),
    ];

    for (case, authorization, expected) in cases {
        let answered = native(&emulator, &addressed, "/api/org", authorization.as_deref()).await;
        assert_eq!(
            answered.status, expected,
            "case {case:?}: expected HTTP {expected} from the workspace's Grafana API, got \
             {} with {:?}",
            answered.status, answered.body
        );
        if expected != 200 {
            assert!(
                answered.body.get("id").is_none(),
                "case {case:?}: a refused request must report no organization, got {:?}",
                answered.body
            );
        }
    }
}

/// At the ANNOUNCED endpoint — the bare address this emulator is reached at, which names no
/// workspace — the credential the request presents is what names the workspace: each
/// workspace's own live key reaches that workspace's organization, a credential this
/// emulator never minted is refused with 401, and a request carrying no bearer credential
/// at all names no workspace and reports no organization.
///
/// The rows differ along whose credential is presented and the form it is presented in.
#[tokio::test]
async fn at_the_announced_endpoint_the_credential_names_the_workspace() {
    let emulator = emulator();
    let (id, endpoint) = active_workspace(&emulator, EMULATOR_HOST, Some("mine"), None).await;
    let (other_id, other_endpoint) =
        active_workspace(&emulator, EMULATOR_HOST, Some("theirs"), None).await;
    assert_eq!(
        endpoint, other_endpoint,
        "two workspaces of one emulator are announced at the one address it answers on, \
         got {endpoint:?} and {other_endpoint:?}"
    );
    let mine = api_key(&emulator, EMULATOR_HOST, &id, "mine", "ADMIN", 3_600).await;
    let theirs = api_key(&emulator, EMULATOR_HOST, &other_id, "theirs", "ADMIN", 3_600).await;

    // (case, the Authorization header presented, the organization name it must reach)
    let reaches: Vec<(&str, String, &str)> = vec![
        ("this workspace's key", bearer(&mine), "mine"),
        ("the other workspace's key", bearer(&theirs), "theirs"),
        (
            "this workspace's key, lower-cased scheme",
            format!("bearer {mine}"),
            "mine",
        ),
    ];
    for (case, authorization, organization) in reaches {
        let org = native(&emulator, &endpoint, "/api/org", Some(&authorization)).await;
        assert_eq!(
            org.status, 200,
            "case {case:?}: the announced endpoint must serve the workspace the credential \
             names, got {} with {:?}",
            org.status, org.body
        );
        assert_eq!(
            org.body["name"],
            json!(organization),
            "case {case:?}: expected the organization {organization:?}, got {:?}",
            org.body
        );
    }

    // A bearer credential no workspace of this emulator holds is refused by the Grafana
    // API, which is what makes a revoked credential observable at the address it was used
    // at.
    let unknown = native(
        &emulator,
        &endpoint,
        "/api/org",
        Some(&bearer("not-a-token-this-emulator-minted")),
    )
    .await;
    assert_eq!(
        unknown.status, 401,
        "a credential this emulator never minted must be refused with 401, got {} with {:?}",
        unknown.status, unknown.body
    );

    // And a request that presents no bearer credential names no workspace at this address,
    // so nothing of a workspace is reported for it.
    for (case, authorization) in [
        ("no Authorization header at all", None),
        ("another authentication scheme", Some(format!("Basic {mine}"))),
    ] {
        let answered = native(&emulator, &endpoint, "/api/org", authorization.as_deref()).await;
        assert_ne!(
            answered.status, 200,
            "case {case:?}: a request naming no workspace must not be served, got {} with \
             {:?}",
            answered.status, answered.body
        );
        assert!(
            answered.body.get("id").is_none(),
            "case {case:?}: a request naming no workspace must report no organization, got \
             {:?}",
            answered.body
        );
    }
}

/// A key is gone once its `secondsToLive` has run out — without any call in
/// between — so it stops authenticating at the endpoint, while a key with a longer
/// lifetime still does.
#[tokio::test]
async fn a_credential_past_its_seconds_to_live_authenticates_nothing() {
    let emulator = emulator();
    let (id, endpoint) = active_workspace(&emulator, EMULATOR_HOST, Some("expiry"), None).await;
    let expiring = api_key(
        &emulator,
        EMULATOR_HOST,
        &id,
        "expiring",
        "ADMIN",
        MIN_SECONDS_TO_LIVE,
    )
    .await;
    let lasting = api_key(
        &emulator,
        EMULATOR_HOST,
        &id,
        "lasting",
        "VIEWER",
        MAX_SECONDS_TO_LIVE,
    )
    .await;

    for (case, token) in [("the expiring key", &expiring), ("the lasting key", &lasting)] {
        let answered = native(&emulator, &endpoint, "/api/org", Some(&bearer(token))).await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: both keys authenticate while they are live, got {} with {:?}",
            answered.status, answered.body
        );
    }

    // The expiry is owed by the clock, not by a call: nothing is done here but look
    // again until the key the shortest lifetime was minted with stops authenticating.
    let expired = await_native_status(
        &emulator,
        &endpoint,
        "/api/org",
        Some(&bearer(&expiring)),
        401,
        "a key past its secondsToLive must authenticate nothing",
    )
    .await;
    assert_eq!(
        expired.status, 401,
        "a key past its secondsToLive must authenticate nothing, got {} with {:?}",
        expired.status, expired.body
    );
    let still_live = native(&emulator, &endpoint, "/api/org", Some(&bearer(&lasting))).await;
    assert_eq!(
        still_live.status, 200,
        "a key whose secondsToLive has not run out must still authenticate, got {} with {:?}",
        still_live.status, still_live.body
    );
}

/// A service-account token is the other credential the control plane mints for
/// calling the Grafana HTTP API: it authenticates at the endpoint, and the use is
/// recorded where `ListWorkspaceServiceAccountTokens` reports it.
///
/// The rows differ along the account's `grafanaRole` and the token's lifetime.
#[tokio::test]
async fn a_service_account_token_authenticates_and_records_its_use() {
    let cases: [(&str, &str, i64); 3] = [
        ("an admin account, one hour", "ADMIN", 3_600),
        ("an editor account, the longest lifetime", "EDITOR", MAX_SECONDS_TO_LIVE),
        ("a viewer account, two minutes", "VIEWER", 120),
    ];

    for (case, role, seconds_to_live) in cases {
        let emulator = emulator();
        let (id, endpoint) =
            active_workspace(&emulator, EMULATOR_HOST, Some("accounts"), None).await;

        let account = control(
            &emulator,
            "POST",
            &format!("/workspaces/{id}/serviceaccounts"),
            EMULATOR_HOST,
            Some(json!({"name": format!("automation-{role}"), "grafanaRole": role})),
        )
        .await;
        assert_eq!(
            account.status, 200,
            "case {case:?}: the service account must be created, got {} with {:?}",
            account.status, account.body
        );
        let account_id = account.string("id");

        let token = control(
            &emulator,
            "POST",
            &format!("/workspaces/{id}/serviceaccounts/{account_id}/tokens"),
            EMULATOR_HOST,
            Some(json!({"name": "ci", "secondsToLive": seconds_to_live})),
        )
        .await;
        assert_eq!(
            token.status, 200,
            "case {case:?}: the token must be created, got {} with {:?}",
            token.status, token.body
        );
        let key = token.body["serviceAccountToken"]["key"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let listed_before = control(
            &emulator,
            "GET",
            &format!("/workspaces/{id}/serviceaccounts/{account_id}/tokens"),
            EMULATOR_HOST,
            None,
        )
        .await;
        assert_eq!(
            listed_before.body["serviceAccountTokens"][0]["lastUsedAt"],
            Value::Null,
            "case {case:?}: nothing has authorized with the token yet, got {:?}",
            listed_before.body
        );

        let org = native(&emulator, &endpoint, "/api/org", Some(&bearer(&key))).await;
        assert_eq!(
            org.status, 200,
            "case {case:?}: the service-account token must authenticate at the endpoint, \
             got {} with {:?}",
            org.status, org.body
        );

        let listed_after = control(
            &emulator,
            "GET",
            &format!("/workspaces/{id}/serviceaccounts/{account_id}/tokens"),
            EMULATOR_HOST,
            None,
        )
        .await;
        assert!(
            listed_after.body["serviceAccountTokens"][0]["lastUsedAt"].is_number(),
            "case {case:?}: the token that authorized must report when it was last used, \
             got {:?}",
            listed_after.body
        );
    }
}

/// The endpoint answers for a live, ACTIVE workspace only: before the status the
/// clock owes it the Grafana server is not serving, and once the workspace is
/// deleted nothing is served there at all.
#[tokio::test]
async fn the_endpoint_answers_only_for_a_live_active_workspace() {
    let emulator = emulator();
    let created = control(
        &emulator,
        "POST",
        "/workspaces",
        EMULATOR_HOST,
        Some(json!({
            "accountAccessType": "CURRENT_ACCOUNT",
            "permissionType": "SERVICE_MANAGED",
            "authenticationProviders": ["SAML"],
        })),
    )
    .await;
    let id = created.body["workspace"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let endpoint = created.body["workspace"]["endpoint"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let key = api_key(&emulator, EMULATOR_HOST, &id, "early", "ADMIN", 3_600).await;

    // A workspace announces its endpoint while it is still CREATING; its Grafana
    // server is not serving there yet.
    let early = native(&emulator, &endpoint, "/api/org", Some(&bearer(&key))).await;
    assert_eq!(
        early.status, 503,
        "a workspace that is still CREATING serves no Grafana API yet, got {} with {:?}",
        early.status, early.body
    );

    // The transition to ACTIVE is owed by the clock: look again until the Grafana API
    // is serving there.
    let serving = await_native_status(
        &emulator,
        &endpoint,
        "/api/org",
        Some(&bearer(&key)),
        200,
        "once the workspace is ACTIVE its endpoint serves the Grafana API",
    )
    .await;
    assert_eq!(
        serving.status, 200,
        "once the workspace is ACTIVE its endpoint serves the Grafana API, got {} with {:?}",
        serving.status, serving.body
    );

    // An endpoint whose workspace id names no workspace, and the endpoint of a
    // workspace that has been deleted: neither is served.
    let unknown = workspace_path_endpoint(EMULATOR_HOST, "g-0123456789");
    let absent = native(&emulator, &unknown, "/api/org", Some(&bearer(&key))).await;
    assert_eq!(
        absent.status, 404,
        "an endpoint naming no workspace must answer 404, got {} with {:?}",
        absent.status, absent.body
    );

    assert_eq!(
        control(
            &emulator,
            "DELETE",
            &format!("/workspaces/{id}"),
            EMULATOR_HOST,
            None
        )
        .await
        .status,
        202,
        "DeleteWorkspace must accept the deletion"
    );
    // The deletion is ACCEPTED, not finished: a workspace in `DELETING` is no longer
    // serving its Grafana API, and once the removal the clock owes the store falls due
    // the endpoint names no workspace at all.
    let deleting = native(&emulator, &endpoint, "/api/org", Some(&bearer(&key))).await;
    assert_eq!(
        deleting.status, 503,
        "a workspace whose deletion is in flight serves no Grafana API, got {} with {:?}",
        deleting.status, deleting.body
    );
    let gone = await_native_status(
        &emulator,
        &workspace_path_endpoint(EMULATOR_HOST, &id),
        "/api/org",
        Some(&bearer(&key)),
        404,
        "a removed workspace serves nothing at its endpoint",
    )
    .await;
    assert_eq!(
        gone.status, 404,
        "a removed workspace serves nothing at its endpoint, got {} with {:?}",
        gone.status, gone.body
    );
    // Its credential went with the record, so at the announced endpoint — where the
    // credential is what names the workspace — it now names none.
    let revoked = native(&emulator, &endpoint, "/api/org", Some(&bearer(&key))).await;
    assert_eq!(
        revoked.status, 401,
        "a credential of a removed workspace names no workspace at the announced endpoint, \
         got {} with {:?}",
        revoked.status, revoked.body
    );
}

/// A route under the endpoint this emulator does not serve answers 501 — never an
/// empty 200 — and a path that is not a workspace endpoint at all is not claimed
/// by this service.
#[tokio::test]
async fn a_route_the_emulator_does_not_serve_answers_501() {
    let emulator = emulator();
    let (id, endpoint) = active_workspace(&emulator, EMULATOR_HOST, Some("routes"), None).await;
    let key = api_key(&emulator, EMULATOR_HOST, &id, "routes", "ADMIN", 3_600).await;

    // Rows: real Grafana HTTP API routes this emulator has not written, the
    // endpoint root, and a route differing only in trailing slash or casing. The
    // addressing that names the workspace carries the endpoint root too, which at the
    // bare announced endpoint is no Grafana route at all.
    let unserved = [
        "/api/dashboards/home",
        "/api/datasources",
        "/api/users",
        "/api/org/users",
        "/api/org/",
        "/api/ORG",
        "",
    ];
    let addressed = workspace_path_endpoint(EMULATOR_HOST, &id);
    for route in unserved {
        for (addressing, address) in [
            ("the workspace's path", &addressed),
            ("the announced endpoint", &endpoint),
        ] {
            // The endpoint root is the Grafana API's own root only where the address
            // names the workspace; at the announced endpoint it names nothing of this
            // plane's.
            if route.is_empty() && addressing == "the announced endpoint" {
                continue;
            }
            let answered = native(&emulator, address, route, Some(&bearer(&key))).await;
            assert_eq!(
                answered.status, 501,
                "route {route:?} under {addressing} is not implemented and must answer 501, \
                 got {} with {:?}",
                answered.status, answered.body
            );
            assert!(
                answered.body["message"].is_string(),
                "route {route:?} under {addressing}: the refusal must say what is not \
                 implemented, got {:?}",
                answered.body
            );
        }
    }

    // And an address that only looks like one of the workspace's: a host whose first label
    // must satisfy `WorkspaceId`'s ^g-[0-9a-f]{10}$ with the labels a published workspace
    // host carries after it, and a path under another plane or naming no workspace id — or
    // this service claims nothing and the router answers its own refusal. Each row is
    // asserted with the credential it would be dialled with: a host Amazon Web Services
    // publishes another endpoint at is never this plane's, whatever credential is
    // presented, while a host outside those zones is an address this emulator may be
    // served at, so there it is the absence of a credential that leaves the request
    // unclaimed.
    let unclaimed: Vec<(String, String, Option<String>)> = vec![
        (
            format!("not-a-workspace.grafana-workspace.{REGION}.amazonaws.com"),
            "/api/org".to_string(),
            Some(bearer(&key)),
        ),
        (
            format!("g-0123456789abc.grafana-workspace.{REGION}.amazonaws.com"),
            "/api/org".to_string(),
            Some(bearer(&key)),
        ),
        (
            format!("{id}.grafana.{REGION}.amazonaws.com"),
            "/api/org".to_string(),
            Some(bearer(&key)),
        ),
        (
            format!("{id}.grafana-workspace.{REGION}.amazonaws.com.example.test"),
            "/api/org".to_string(),
            None,
        ),
        (
            format!("{id}.grafana-workspace.amazonaws.com"),
            "/api/org".to_string(),
            Some(bearer(&key)),
        ),
        (
            EMULATOR_HOST.to_string(),
            format!("/_aws/athena/{id}/api/org"),
            Some(bearer(&key)),
        ),
        (
            EMULATOR_HOST.to_string(),
            "/_aws/grafana/not-a-workspace/api/org".to_string(),
            Some(bearer(&key)),
        ),
        (
            EMULATOR_HOST.to_string(),
            format!("/grafana/{id}/api/org"),
            Some(bearer(&key)),
        ),
        // Another service's traffic at this emulator's own authority, under the same path
        // prefix: a request routed by its own SigV4 credential scope presents no bearer
        // credential, and one that presents none at all names no workspace here.
        (
            EMULATOR_HOST.to_string(),
            "/api/v2/clusters".to_string(),
            Some(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/eu-west-1/kafka/aws4_request, \
                 SignedHeaders=host, Signature=abc"
                    .to_string(),
            ),
        ),
        (
            EMULATOR_HOST.to_string(),
            "/api/v1/namespaces".to_string(),
            None,
        ),
    ];
    for (host, path, authorization) in unclaimed {
        let answered =
            request(&emulator, "GET", &path, &host, authorization.as_deref(), None).await;
        assert_ne!(
            answered.status, 200,
            "{host}{path} is no workspace endpoint, so this service must not serve the \
             Grafana API there, got {} with {:?}",
            answered.status, answered.body
        );
        assert!(
            answered.body["message"].as_str().is_none_or(|message| {
                !message.contains("not implemented in this emulator")
            }),
            "{host}{path} is no workspace endpoint, so no Grafana route may answer for it, \
             got {:?}",
            answered.body
        );
    }

    // A DNS name is case-insensitive, so the host Amazon Managed Grafana publishes for the
    // workspace addresses it in any casing.
    let shouted = native(
        &emulator,
        &aws_published_host(&id).to_ascii_uppercase(),
        "/api/org",
        Some(&bearer(&key)),
    )
    .await;
    assert_eq!(
        shouted.status, 200,
        "the published host in upper case names the same workspace, got {} with {:?}",
        shouted.status, shouted.body
    );
}
