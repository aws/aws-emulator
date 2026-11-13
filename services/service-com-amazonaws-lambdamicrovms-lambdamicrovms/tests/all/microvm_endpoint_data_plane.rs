// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the MicroVM ENDPOINT: the address `RunMicrovm` publishes and
//! what this emulator answers on it.
//!
//! Each test states a RULE and exercises it over a SET of inputs; the values any one
//! row happens to carry are examples, not the specification. The control plane is
//! driven over the wire through the same router the emulator boots (the harness
//! `microvm_lifecycle` owns); the endpoint itself is driven through
//! `state::microvm_endpoint::serve_request`, which is the one decision the core's
//! HTTPS listener hands every request on that address to — the listener's own
//! transport (TLS, CORS, the Host guard) is the core's and is not re-tested here.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_lambdamicrovms_lambdamicrovms::state::microvm_endpoint::{
    auth_token_digest, serve_request, AUTH_TOKEN_HEADER, NO_GUEST_RESPONSE_STATUS,
};
use service_com_amazonaws_lambdamicrovms_lambdamicrovms::state::microvm_network::connector_state;
use service_com_amazonaws_lambdamicrovms_lambdamicrovms::state::ComAmazonawsLambdamicrovmsLambdamicrovmsState as MicrovmState;
use smithy_server_runtime::MultiServiceRouter;

use crate::microvm_lifecycle::{
    built_image, call, emulator_with_engine, image_body, run_microvm,
};

/// One row of the status table: the case, the address a client sends to, the guest
/// path it appends to that address, the method, the token it presents (where it
/// presents one) and the status the endpoint must answer with.
type EndpointCase<'a> = (&'a str, &'a str, &'a str, &'a str, Option<&'a str>, u16);

/// One request a client sends through a MicroVM's published endpoint: the endpoint,
/// the guest's own path appended to it (`<endpoint><path>`, as AWS documents), and the
/// `X-aws-proxy-auth` token where the client presents one.
fn guest_request(
    method: &str,
    endpoint: &str,
    path: &str,
    token: Option<&str>,
) -> http::Request<Vec<u8>> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("{endpoint}{path}"));
    if let Some(token) = token {
        builder = builder.header(AUTH_TOKEN_HEADER, token);
    }
    builder
        .body(Vec::new())
        .expect("a well-formed request to a MicroVM endpoint")
}

/// Send `request` to the endpoint this emulator serves, and report the status, the
/// content type and the body as text.
fn through_endpoint(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    request: &http::Request<Vec<u8>>,
) -> impl std::future::Future<Output = (u16, String, String)> {
    let state = engine.state::<MicrovmState>();
    let connectors = connector_state(&**engine);
    let callback = smithy_server_runtime::EmbeddedEmulator::from_router(router.clone());
    let request = request.clone();
    async move {
        let response = serve_request(&state, connectors.as_ref(), &callback, &request).await;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let body = String::from_utf8_lossy(response.body()).to_string();
        (status, content_type, body)
    }
}

/// How long a poll for one clock-owed convergence may take before the test gives up:
/// generous, because what is asserted is that the transition HAPPENS, not when.
const SETTLE_DEADLINE: Duration = Duration::from_secs(30);

/// How long one poll of a settling resource waits before looking again.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Poll `GetMicrovmImage` until the image `name` has settled with an active version, so
/// what follows waits for the CONDITION rather than for a fixed time.
async fn settled_image(router: &MultiServiceRouter, name: &str) {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, read) = call(
            router,
            "GET",
            &format!("/2025-09-09/microvm-images/{name}"),
            None,
        )
        .await;
        assert_eq!(status, 200, "read the image {name}: got {status} {read}");
        if read["latestActiveImageVersion"].is_string() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the image {name} never settled with an active version: last read {read}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Poll `GetMicrovm` until `microvm_id` reports `state`.
async fn microvm_in_state(router: &MultiServiceRouter, microvm_id: &str, state: &str) {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, read) = call(
            router,
            "GET",
            &format!("/2025-09-09/microvms/{microvm_id}"),
            None,
        )
        .await;
        assert_eq!(status, 200, "read the MicroVM {microvm_id}: got {status} {read}");
        if read["state"].as_str() == Some(state) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the MicroVM {microvm_id} never reached {state}: last read {read}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Run a MicroVM from `image` and return `(microvmId, endpoint)` once it is RUNNING.
async fn running_microvm(router: &MultiServiceRouter, image: &str) -> (String, String) {
    let (status, run) = run_microvm(router, json!({"imageIdentifier": image})).await;
    assert_eq!(status, 200, "run a MicroVM from {image}: got {status} {run}");
    let microvm_id = run["microvmId"].as_str().unwrap_or_default().to_string();
    microvm_in_state(router, &microvm_id, "RUNNING").await;
    (
        microvm_id,
        run["endpoint"].as_str().unwrap_or_default().to_string(),
    )
}

/// Mint an authentication token for `microvm_id` over the wire and return its value.
async fn auth_token(
    router: &MultiServiceRouter,
    microvm_id: &str,
    expiration_in_minutes: u32,
    allowed_ports: Value,
) -> String {
    let (status, response) = call(
        router,
        "POST",
        &format!("/2025-09-09/microvms/{microvm_id}/auth-token"),
        Some(json!({
            "expirationInMinutes": expiration_in_minutes,
            "allowedPorts": allowed_ports,
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "mint a token for {microvm_id}: got {status} {response}"
    );
    response["authToken"][AUTH_TOKEN_HEADER]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The `host:port` an endpoint URL addresses, as a socket address — so a test can
/// prove the published address is one a client on this machine can open.
fn endpoint_socket_addr(endpoint: &str) -> std::net::SocketAddr {
    let authority = endpoint
        .strip_prefix("https://")
        .unwrap_or_default()
        .split('/')
        .next()
        .unwrap_or_default();
    authority
        .parse()
        .unwrap_or_else(|error| panic!("endpoint {endpoint:?} has no socket address: {error}"))
}

/// RULE: a published `endpoint` is an ADDRESS THE EMULATOR ANSWERS ON — its own
/// loopback HTTPS listener, with the MicroVM named in the path — so a client can open
/// it, every MicroVM of one emulator is published on that one address, and the reader
/// (`GetMicrovm`) reports the same endpoint the launch did.
#[tokio::test(flavor = "multi_thread")]
async fn a_published_microvm_endpoint_is_an_address_a_client_can_open() {
    let (router, _engine) = emulator_with_engine();
    built_image(&router, "endpoint-bearing").await;

    // Rows differ in what the launch asks for, because none of it changes WHERE the
    // MicroVM is published: the request members, the hook port its image declares, and
    // how many MicroVMs the store already holds.
    let rows: Vec<(&str, Value)> = vec![
        ("required members only", json!({"imageIdentifier": "endpoint-bearing"})),
        (
            "a second MicroVM of the same image",
            json!({"imageIdentifier": "endpoint-bearing"}),
        ),
        (
            "a bounded lifetime and an idle policy",
            json!({
                "imageIdentifier": "endpoint-bearing",
                "maximumDurationInSeconds": 900,
                "idlePolicy": {
                    "maxIdleDurationSeconds": 60,
                    "suspendedDurationSeconds": 0,
                    "autoResumeEnabled": true,
                },
            }),
        ),
        (
            "network connectors on both sides",
            json!({
                "imageIdentifier": "endpoint-bearing",
                "ingressNetworkConnectors": ["HTTP_INGRESS"],
                "egressNetworkConnectors": ["INTERNET_EGRESS"],
            }),
        ),
    ];

    let mut published: Vec<(String, String)> = Vec::new();
    for (case, body) in rows {
        let (status, run) = run_microvm(&router, body).await;
        assert_eq!(status, 200, "case {case:?}: got {status} {run}");
        let microvm_id = run["microvmId"].as_str().unwrap_or_default().to_string();
        let endpoint = run["endpoint"].as_str().unwrap_or_default().to_string();

        assert!(
            endpoint.starts_with("https://"),
            "case {case:?}: expected an HTTPS endpoint, got {endpoint:?}"
        );
        assert!(
            endpoint.ends_with(&format!("/microvms/{microvm_id}")),
            "case {case:?}: expected the endpoint to name the MicroVM, got {endpoint:?}"
        );

        // The address is this emulator's own: a loopback host and a port a client on
        // this machine can connect to. Opening the connection is the whole point —
        // before the fix the announced host resolved off this machine, so a client got
        // a transport failure and no HTTP status at all. Only the emulator's own
        // loopback port is ever dialled, and the row above has already refused any
        // address that is not loopback.
        let addr = endpoint_socket_addr(&endpoint);
        assert!(
            addr.ip() == std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
                || addr.ip() == std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
            "case {case:?}: expected the emulator's own loopback address, got {addr}"
        );
        assert_ne!(addr.port(), 0, "case {case:?}: expected a bound port");
        let dialled = match addr.ip() {
            std::net::IpAddr::V4(_) => std::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, addr.port())),
            std::net::IpAddr::V6(_) => std::net::TcpStream::connect((std::net::Ipv6Addr::LOCALHOST, addr.port())),
        };
        if let Err(error) = dialled {
            panic!("case {case:?}: expected {addr} to accept a connection, got {error}");
        }

        // The reader reports the stored record, so it publishes the same address.
        let (status, read) = call(
            &router,
            "GET",
            &format!("/2025-09-09/microvms/{microvm_id}"),
            None,
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: read back: got {status} {read}");
        assert_eq!(
            read["endpoint"].as_str(),
            Some(endpoint.as_str()),
            "case {case:?}: GetMicrovm must report the published endpoint, got {read}"
        );

        published.push((microvm_id, endpoint));
    }

    // Every MicroVM of one emulator is published on the one address the store opened,
    // and each is addressed by its own path.
    let authorities: Vec<std::net::SocketAddr> = published
        .iter()
        .map(|(_, endpoint)| endpoint_socket_addr(endpoint))
        .collect();
    assert!(
        authorities.windows(2).all(|pair| pair[0] == pair[1]),
        "expected one address for every MicroVM of the store, got {authorities:?}"
    );
    let mut paths: Vec<&str> = published
        .iter()
        .map(|(_, endpoint)| endpoint.as_str())
        .collect();
    paths.sort_unstable();
    let count = paths.len();
    paths.dedup();
    assert_eq!(
        paths.len(),
        count,
        "expected a distinct endpoint per MicroVM, got {paths:?}"
    );
}

/// RULE: a published endpoint answers an HTTP STATUS for every request — `404` where
/// the address names no MicroVM that can be connected to, `403` where no live token
/// `CreateMicrovmAuthToken` minted for THAT MicroVM (and covering the port its image
/// publishes its listener on) is presented in `X-aws-proxy-auth`, and otherwise the
/// GUEST's own answer — which, for a MicroVM whose image holds no guest program this
/// emulator's engine can run (these images' code artifacts were never served), is the
/// status the proxy answers for a MicroVM it reached nothing inside of. Never a transport
/// failure, whatever the guest path, the method or the token.
#[tokio::test(flavor = "multi_thread")]
async fn a_published_endpoint_answers_an_http_status_for_every_request_class() {
    let (router, engine) = emulator_with_engine();

    // One image DECLARES the port its guest listener runs on (`Hooks$port`), one
    // declares none — the two cases a token's `allowedPorts` is judged against.
    let (status, created) = call(
        &router,
        "POST",
        "/2025-09-09/microvm-images",
        Some(image_body(
            "ported",
            json!({"hooks": {"port": 8080}}),
        )),
    )
    .await;
    assert_eq!(status, 201, "create the ported image: got {status} {created}");
    built_image(&router, "portless").await;
    // Both images' builds are clock-owed: wait for the CONDITION each test row needs —
    // an active version to launch from — rather than for a fixed time.
    settled_image(&router, "ported").await;
    settled_image(&router, "portless").await;

    let (ported_id, ported) = running_microvm(&router, "ported").await;
    let (_, portless) = running_microvm(&router, "portless").await;
    let (other_id, _other) = running_microvm(&router, "ported").await;

    let every_port = auth_token(&router, &ported_id, 15, json!([{"allPorts": {}}])).await;
    let exact_port = auth_token(&router, &ported_id, 15, json!([{"port": 8080}])).await;
    let covering_range = auth_token(
        &router,
        &ported_id,
        15,
        json!([{"range": {"startPort": 8000, "endPort": 9000}}]),
    )
    .await;
    let other_port = auth_token(&router, &ported_id, 15, json!([{"port": 9090}])).await;
    let other_range = auth_token(
        &router,
        &ported_id,
        15,
        json!([{"range": {"startPort": 9000, "endPort": 9100}}]),
    )
    .await;
    let portless_id = portless
        .rsplit('/')
        .next()
        .expect("the endpoint names the MicroVM")
        .to_string();
    let portless_every_port = auth_token(&router, &portless_id, 1, json!([{"allPorts": {}}])).await;
    let portless_one_port = auth_token(&router, &portless_id, 1, json!([{"port": 443}])).await;
    let other_microvm_token = auth_token(&router, &other_id, 15, json!([{"allPorts": {}}])).await;

    // The base of the published address, which names no MicroVM by itself.
    let base = ported
        .strip_suffix(&format!("/microvms/{ported_id}"))
        .expect("the endpoint is the store's address plus the MicroVM")
        .to_string();

    // (case, the address, the guest path appended to it, the method, the token, status)
    let rows: Vec<EndpointCase> = vec![
        ("no token at all", &ported, "/health", "GET", None, 403),
        (
            "a token value nothing minted",
            &ported,
            "/health",
            "GET",
            Some("not-a-token-this-emulator-minted"),
            403,
        ),
        (
            "a live token minted for another MicroVM",
            &ported,
            "/health",
            "GET",
            Some(&other_microvm_token),
            403,
        ),
        (
            "a token whose allowedPorts name another port",
            &ported,
            "/health",
            "GET",
            Some(&other_port),
            403,
        ),
        (
            "a token whose range excludes the published port",
            &ported,
            "/health",
            "GET",
            Some(&other_range),
            403,
        ),
        (
            "a token for every port",
            &ported,
            "/health",
            "GET",
            Some(&every_port),
            NO_GUEST_RESPONSE_STATUS,
        ),
        (
            "a token naming the published port exactly",
            &ported,
            "/health",
            "GET",
            Some(&exact_port),
            NO_GUEST_RESPONSE_STATUS,
        ),
        (
            "a token whose range covers the published port",
            &ported,
            "/health",
            "GET",
            Some(&covering_range),
            NO_GUEST_RESPONSE_STATUS,
        ),
        (
            "a MicroVM whose image publishes no port, with a token for every port",
            &portless,
            "/health",
            "GET",
            Some(&portless_every_port),
            NO_GUEST_RESPONSE_STATUS,
        ),
        (
            "a MicroVM whose image publishes no port, with a token scoped to one port",
            &portless,
            "/health",
            "GET",
            Some(&portless_one_port),
            403,
        ),
        (
            "a guest path carrying a query",
            &ported,
            "/connect?host=10.90.1.17&port=443",
            "GET",
            Some(&every_port),
            NO_GUEST_RESPONSE_STATUS,
        ),
        (
            "the guest's root path",
            &ported,
            "/",
            "GET",
            Some(&every_port),
            NO_GUEST_RESPONSE_STATUS,
        ),
        (
            "no path past the endpoint at all",
            &ported,
            "",
            "GET",
            Some(&every_port),
            NO_GUEST_RESPONSE_STATUS,
        ),
        (
            "a POST to the guest",
            &ported,
            "/health",
            "POST",
            Some(&every_port),
            NO_GUEST_RESPONSE_STATUS,
        ),
        (
            "a HEAD of the guest",
            &ported,
            "/health",
            "HEAD",
            Some(&every_port),
            NO_GUEST_RESPONSE_STATUS,
        ),
        (
            "an address naming a MicroVM this store never minted",
            &base,
            "/microvms/mvm-ffffffffffffffff/health",
            "GET",
            Some(&every_port),
            404,
        ),
        (
            "an address under no MicroVM path",
            &base,
            "/health",
            "GET",
            Some(&every_port),
            404,
        ),
        (
            "the listener's own root",
            &base,
            "/",
            "GET",
            Some(&every_port),
            404,
        ),
        (
            "a MicroVM path with no identifier",
            &base,
            "/microvms/",
            "GET",
            Some(&every_port),
            404,
        ),
    ];

    for (case, endpoint, path, method, token, expected) in rows {
        let request = guest_request(method, endpoint, path, token);
        let (status, content_type, body) = through_endpoint(&router, &engine, &request).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} {body:?}"
        );
        assert_eq!(
            content_type, "text/plain; charset=utf-8",
            "case {case:?}: expected a plain-text answer, got {content_type:?}"
        );
        if method == "HEAD" {
            assert!(
                body.is_empty(),
                "case {case:?}: a HEAD carries no body, got {body:?}"
            );
        } else {
            assert!(
                !body.is_empty(),
                "case {case:?}: expected the endpoint's own wording, got an empty body"
            );
        }
    }
}

/// RULE: a token is accepted only while it is LIVE — the clock's own expiry, and the
/// MicroVM's: once `expirationInMinutes` has run out, or the MicroVM can no longer be
/// connected to, the same request that was served is refused with `403`, and the
/// address of a MicroVM on its way out answers `404`.
#[tokio::test(flavor = "multi_thread")]
async fn a_token_stops_being_accepted_once_the_clock_or_the_microvm_has_ended_it() {
    let (router, engine) = emulator_with_engine();
    built_image(&router, "expiring").await;
    let (microvm_id, endpoint) = running_microvm(&router, "expiring").await;
    let (doomed_id, doomed) = running_microvm(&router, "expiring").await;

    let token = auth_token(&router, &microvm_id, 15, json!([{"allPorts": {}}])).await;
    let doomed_token = auth_token(&router, &doomed_id, 15, json!([{"allPorts": {}}])).await;

    // The control: both tokens are live, so both endpoints answer past the token check.
    for (case, endpoint, token) in [
        ("the MicroVM whose token is about to expire", &endpoint, &token),
        ("the MicroVM about to be terminated", &doomed, &doomed_token),
    ] {
        let request = guest_request("GET", endpoint, "/health", Some(token));
        let (status, _, body) = through_endpoint(&router, &engine, &request).await;
        assert_eq!(
            status,
            NO_GUEST_RESPONSE_STATUS,
            "case {case:?}: a live token must pass the token check, got {status} {body:?}"
        );
    }

    // The clock expires the token: the store holds its digest with the instant it stops
    // being accepted, so moving that instant into the past is the same thing as waiting
    // out `expirationInMinutes`.
    {
        let state = engine.state::<MicrovmState>();
        let mut store = state.lock();
        let digest = auth_token_digest(&token);
        let record = store
            .auth_tokens
            .get_mut(&digest)
            .expect("the minted token is recorded against the MicroVM");
        record.expires_at = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .unwrap_or_else(Instant::now);
    }
    let request = guest_request("GET", &endpoint, "/health", Some(&token));
    let (status, _, body) = through_endpoint(&router, &engine, &request).await;
    assert_eq!(
        status, 403,
        "an expired token must be refused, got {status} {body:?}"
    );

    // And a MicroVM that can no longer be connected to is published nowhere: its
    // address answers 404 from the moment it starts terminating, token or no token.
    let (status, terminated) = call(
        &router,
        "DELETE",
        &format!("/2025-09-09/microvms/{doomed_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "terminate the MicroVM: got {status} {terminated}"
    );
    for (case, token) in [
        ("with the token minted for it", Some(doomed_token.as_str())),
        ("with no token at all", None),
    ] {
        let request = guest_request("GET", &doomed, "/health", token);
        let (status, _, body) = through_endpoint(&router, &engine, &request).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a MicroVM on its way out is published nowhere, got {status} {body:?}"
        );
    }
    // The termination is clock-owed, so this waits for the MicroVM to REACH its terminal
    // state rather than for a fixed time.
    microvm_in_state(&router, &doomed_id, "TERMINATED").await;
    let request = guest_request("GET", &doomed, "/health", Some(&doomed_token));
    let (status, _, body) = through_endpoint(&router, &engine, &request).await;
    assert_eq!(
        status, 404,
        "a terminated MicroVM is published nowhere, got {status} {body:?}"
    );
}
