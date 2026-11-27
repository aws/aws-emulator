// SPDX-License-Identifier: CC0-1.0

//! The address a workspace's Grafana console and HTTP API are announced at, across EVERY
//! projection of the workspace that carries it — and that the announced address is one this
//! emulator answers that workspace's Grafana HTTP API on.
//!
//! The rule under test: `WorkspaceDescription$endpoint` and `WorkspaceSummary$endpoint`
//! carry an ADDRESS this emulator is reached at — the bare `host[:port]` the request arrived
//! on, port included, with no scheme and no path — and never a host in a zone Amazon Web
//! Services publishes its own endpoints in, which nothing here answers for. Every operation
//! that reports a workspace reports that one value for it, every workspace of one emulator
//! is announced at the one address it answers on, and a client that splices a Grafana route
//! onto the value (`https://<endpoint>/api/org`, the splice the member's own documentation
//! invites) reaches the Grafana HTTP API of the workspace its credential names rather than
//! leaving this emulator.
//!
//! The rows differ along the address the control-plane call reached this emulator at
//! (loopback by address on two ports, loopback by name, an IPv6 literal, a routable private
//! address, a host naming the real service's own endpoint, and no `Host` at all), along the
//! projection the endpoint was read from (`CreateWorkspace`, `DescribeWorkspace`,
//! `UpdateWorkspace`, the `ListWorkspaces` summary and `DeleteWorkspace`), along the
//! workspace, of which there are two per row, and along the region the create was signed
//! for — which the announced address does not depend on.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_grafana_awsgrafanacontrolplane::state::LIFECYCLE_TRANSITION;
use service_com_amazonaws_grafana_awsgrafanacontrolplane::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The base URL an emulator that is served on no port at all reports for itself
/// ([`smithy_server_runtime::UNSERVED_URL_BASE`] without its scheme): the address announced
/// to a client that reached this process without naming an address of it, which is the truth
/// about a router no port is bound for rather than a name of the real service's.
const UNSERVED_AUTHORITY: &str = "emulator.invalid";

/// How long a case waits for a status the clock owes a workspace before giving up. Far
/// above the transition span, so a loaded host still reaches it; nothing asserts on how
/// long it actually took.
const STATUS_DEADLINE: Duration = LIFECYCLE_TRANSITION.saturating_mul(20);

/// How often such a wait looks again.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// What a call answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 credential scope naming this service in `region`: it is what routes a
/// control-plane request here, and the region the workspace's ARNs belong to.
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/grafana/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// The endpoint a workspace is announced at by an emulator reached at `authority`, spelled
/// out here independently of the crate's own constants: the authority the request arrived
/// on, and nothing else.
fn expected_endpoint(authority: &str) -> String {
    authority.to_string()
}

/// The endpoint PATH this emulator also answers a workspace's Grafana API at — the
/// addressing that names the workspace in the address rather than through the credential a
/// request presents.
fn workspace_path_endpoint(authority: &str, id: &str) -> String {
    format!("{authority}/_aws/grafana/{id}")
}

/// The `Host` header and the request path a client builds to reach `route` under the
/// announced `endpoint` — the splice `https://<endpoint><route>` performed on the parts an
/// HTTP request carries them in.
fn addressed(endpoint: &str, route: &str) -> (String, String) {
    match endpoint.split_once('/') {
        Some((host, path)) => (host.to_string(), format!("/{path}{route}")),
        None if route.is_empty() => (endpoint.to_string(), "/".to_string()),
        None => (endpoint.to_string(), route.to_string()),
    }
}

/// One request through the router, with an explicit `Host` (omitted when empty) and an
/// optional `Authorization` header.
async fn send(
    router: &MultiServiceRouter,
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
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer { status, body }
}

/// One control-plane call in `region`, reaching the emulator at `host`.
async fn call(
    router: &MultiServiceRouter,
    region: &str,
    host: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    send(router, method, uri, host, Some(&signed_for(region)), body).await
}

/// A call to the workspace's own Grafana HTTP API at the address the control plane announced
/// for it: the `route` spliced onto `endpoint`, with no SigV4 credential — a Grafana client
/// has none — and only the bearer `authorization` given.
async fn native(
    router: &MultiServiceRouter,
    endpoint: &str,
    route: &str,
    authorization: Option<&str>,
) -> Answer {
    let (host, path) = addressed(endpoint, route);
    send(router, "GET", &path, &host, authorization, None).await
}

/// Create one workspace named `name` in `region`: its id, and the endpoint the create
/// announced.
async fn create_workspace(
    router: &MultiServiceRouter,
    region: &str,
    host: &str,
    name: &str,
) -> (String, String) {
    let created = call(
        router,
        region,
        host,
        "POST",
        "/workspaces",
        Some(json!({
            "accountAccessType": "CURRENT_ACCOUNT",
            "permissionType": "SERVICE_MANAGED",
            "authenticationProviders": ["SAML"],
            "workspaceName": name,
            "workspaceRoleArn": "arn:aws:iam::000000000000:role/grafana-workspace",
        })),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case {name:?}: the workspace must be created, got {} with {:?}",
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
    (id, endpoint)
}

/// Look again until the workspace `id` is ACTIVE — the status the clock owes a new
/// workspace — or the deadline passes. Nothing waits for a fixed time and nothing asserts
/// on elapsed time.
async fn await_active(
    router: &MultiServiceRouter,
    region: &str,
    host: &str,
    id: &str,
) -> Answer {
    let deadline = Instant::now() + STATUS_DEADLINE;
    loop {
        let described = call(
            router,
            region,
            host,
            "GET",
            &format!("/workspaces/{id}"),
            None,
        )
        .await;
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

/// The addresses a client reaches this emulator at, and the authority each one is announced
/// as: the address itself, except where the request named no address of this emulator — no
/// `Host` at all, the real service's own endpoint host, or a name of an operator's own,
/// each of which names a service or nothing of this process — where the base of an emulator
/// served on no port is announced.
fn client_addresses() -> [(&'static str, &'static str, &'static str); 8] {
    [
        (
            "loopback by address, the default port",
            "127.0.0.1:9297",
            "127.0.0.1:9297",
        ),
        (
            "loopback by address, another port",
            "127.0.0.1:34567",
            "127.0.0.1:34567",
        ),
        ("loopback by name", "localhost:9297", "localhost:9297"),
        ("an IPv6 loopback literal", "[::1]:9297", "[::1]:9297"),
        (
            "a routable private address",
            "10.1.2.3:8443",
            "10.1.2.3:8443",
        ),
        (
            "a name of an operator's own",
            "emu.internal.example:8443",
            UNSERVED_AUTHORITY,
        ),
        (
            "the real service's own endpoint host",
            "grafana.us-west-2.amazonaws.com",
            UNSERVED_AUTHORITY,
        ),
        ("no Host header at all", "", UNSERVED_AUTHORITY),
    ]
}

#[tokio::test]
async fn every_projection_reports_the_one_endpoint_the_workspace_is_announced_at() {
    // The regions the creates are signed for: the announced address must not depend on the
    // region, so consecutive rows are read in different ones.
    let regions = ["eu-west-1", "us-west-2", "ap-southeast-3"];
    for (index, (case, host, authority)) in client_addresses().into_iter().enumerate() {
        let region = regions[index % regions.len()];
        let router = emulator();

        // Two workspaces, so a projection reporting the other record's address cannot
        // pass; one emulator answers both at the one address it is reached at.
        let (first_id, first_created_endpoint) =
            create_workspace(&router, region, host, "monitoring").await;
        let (second_id, second_created_endpoint) =
            create_workspace(&router, region, host, "reporting").await;
        assert_eq!(
            first_created_endpoint, second_created_endpoint,
            "case {case:?} in {region:?}: both workspaces are announced at the one address \
             this emulator answers on, got {first_created_endpoint:?} and \
             {second_created_endpoint:?}"
        );

        for (which, id, created_endpoint) in [
            ("the first workspace", &first_id, &first_created_endpoint),
            ("the second workspace", &second_id, &second_created_endpoint),
        ] {
            let want = expected_endpoint(authority);
            assert_eq!(
                created_endpoint, &want,
                "case {case:?}/{which:?} in {region:?}: CreateWorkspace must announce an \
                 address this emulator is reached at, expected {want:?}, got \
                 {created_endpoint:?}"
            );
            // No name in a zone Amazon Web Services publishes its own endpoints in: a
            // client handed one leaves this emulator entirely.
            for forbidden in [".amazonaws.com", ".api.aws", ".on.aws"] {
                assert!(
                    !created_endpoint.contains(forbidden),
                    "case {case:?}/{which:?} in {region:?}: an announced endpoint carries no \
                     {forbidden:?}, got {created_endpoint:?}"
                );
            }
            // No scheme, exactly as the value the real service announces carries none, so
            // the splice a client performs on it stays a well-formed URL.
            assert!(
                !created_endpoint.contains("://"),
                "case {case:?}/{which:?} in {region:?}: an announced endpoint carries no \
                 scheme, got {created_endpoint:?}"
            );

            // An update is accepted only once the workspace is up, so wait for the status
            // the clock owes it rather than for a span of time.
            let described = await_active(&router, region, host, id).await;
            assert_eq!(
                described.status, 200,
                "case {case:?}/{which:?} in {region:?}: DescribeWorkspace must answer 200, \
                 got {} with {:?}",
                described.status, described.body
            );

            // And the announced address is one this emulator answers that workspace's
            // Grafana HTTP API on: addressed with the workspace's own path beneath that
            // same authority, where the address names the workspace, the request is refused
            // by the Grafana API for want of a credential rather than by the router for
            // naming nothing it serves.
            let unauthenticated = native(
                &router,
                &workspace_path_endpoint(created_endpoint, id),
                "/api/org",
                None,
            )
            .await;
            assert_eq!(
                unauthenticated.status, 401,
                "case {case:?}/{which:?} in {region:?}: the announced endpoint must serve the \
                 workspace's Grafana API, got {} with {:?}",
                unauthenticated.status, unauthenticated.body
            );

            let updated = call(
                &router,
                region,
                host,
                "PUT",
                &format!("/workspaces/{id}"),
                Some(json!({"workspaceDescription": format!("{which} updated")})),
            )
            .await;
            assert_eq!(
                updated.status, 202,
                "case {case:?}/{which:?} in {region:?}: UpdateWorkspace must answer 202, got \
                 {} with {:?}",
                updated.status, updated.body
            );

            let listed = call(&router, region, host, "GET", "/workspaces", None).await;
            assert_eq!(
                listed.status, 200,
                "case {case:?}/{which:?} in {region:?}: ListWorkspaces must answer 200, got \
                 {} with {:?}",
                listed.status, listed.body
            );
            let summary = listed.body["workspaces"]
                .as_array()
                .and_then(|workspaces| {
                    workspaces
                        .iter()
                        .find(|workspace| workspace["id"] == json!(id))
                        .cloned()
                })
                .unwrap_or(Value::Null);

            let deleted = call(
                &router,
                region,
                host,
                "DELETE",
                &format!("/workspaces/{id}"),
                None,
            )
            .await;
            assert_eq!(
                deleted.status, 202,
                "case {case:?}/{which:?} in {region:?}: DeleteWorkspace must answer 202, got \
                 {} with {:?}",
                deleted.status, deleted.body
            );

            // One address, reported by every projection that carries it.
            for (projection, reported) in [
                ("DescribeWorkspace", &described.body["workspace"]["endpoint"]),
                ("UpdateWorkspace", &updated.body["workspace"]["endpoint"]),
                ("the ListWorkspaces summary", &summary["endpoint"]),
                ("DeleteWorkspace", &deleted.body["workspace"]["endpoint"]),
            ] {
                assert_eq!(
                    *reported,
                    json!(want),
                    "case {case:?}/{which:?} in {region:?} read from {projection}: expected \
                     the endpoint {want:?}, got {reported:?}"
                );
            }
        }
    }
}

/// The announced endpoint serves the workspace's Grafana HTTP API end to end: a credential
/// the control plane minted for a workspace, presented at the announced address, is answered
/// from that workspace's record — and since one emulator announces one address, the
/// credential is what says which workspace a request there addresses.
///
/// This is the flow a client that reads `endpoint` performs, and the rows differ along the
/// address the client reached this emulator at — every one of which must lead back here —
/// and along which of the two workspaces' credentials is presented.
#[tokio::test]
async fn a_client_following_the_announced_endpoint_reaches_the_workspaces_grafana_api() {
    for (case, host, authority) in client_addresses() {
        let router = emulator();
        let region = "us-west-2";
        let (id, endpoint) = create_workspace(&router, region, host, "observability").await;
        let (other_id, other_endpoint) = create_workspace(&router, region, host, "reporting").await;
        assert_eq!(
            endpoint,
            expected_endpoint(authority),
            "case {case:?}: the create must announce an address of this emulator, got \
             {endpoint:?}"
        );
        assert_eq!(
            other_endpoint, endpoint,
            "case {case:?}: one emulator announces one address, got {endpoint:?} and \
             {other_endpoint:?}"
        );
        await_active(&router, region, host, &id).await;
        await_active(&router, region, host, &other_id).await;

        for (which, workspace_id, organization) in [
            ("the first workspace", &id, "observability"),
            ("the second workspace", &other_id, "reporting"),
        ] {
            let key = call(
                &router,
                region,
                host,
                "POST",
                &format!("/workspaces/{workspace_id}/apikeys"),
                Some(
                    json!({"keyName": "dashboards", "keyRole": "ADMIN", "secondsToLive": 3_600}),
                ),
            )
            .await;
            assert_eq!(
                key.status, 200,
                "case {case:?}/{which}: CreateWorkspaceApiKey must mint the key, got {} with \
                 {:?}",
                key.status, key.body
            );
            let secret = key.body["key"].as_str().unwrap_or_default().to_string();

            let org = native(
                &router,
                &endpoint,
                "/api/org",
                Some(&format!("Bearer {secret}")),
            )
            .await;
            assert_eq!(
                org.status, 200,
                "case {case:?}/{which}: the minted key must authenticate at the announced \
                 endpoint {endpoint:?}, got {} with {:?}",
                org.status, org.body
            );
            assert_eq!(
                org.body["id"],
                json!(1),
                "case {case:?}/{which}: the organization must report its id, got {:?}",
                org.body
            );
            assert_eq!(
                org.body["name"],
                json!(organization),
                "case {case:?}/{which}: the credential must reach the organization \
                 {organization:?}, read off the workspace record, got {:?}",
                org.body
            );
        }
    }
}
