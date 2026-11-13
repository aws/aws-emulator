// SPDX-License-Identifier: CC0-1.0

//! Deploying a WebSocket API and then calling its `@connections` endpoint, through the
//! WHOLE assembled router — the client sequence this issue reported as unusable.
//!
//! What it pins, in the order a deploy-then-push client performs it:
//!
//! 1. `CreateApi` (`WEBSOCKET`), `CreateIntegration` and `CreateRoute` on API Gateway V2
//!    answer their modelled 201s, and
//! 2. **`CreateDeployment` answers its modelled `201` with a `DeploymentId` and a
//!    `DeploymentStatus`** that `GetDeployment` then reports back. It is the step that
//!    used to answer the scaffold's unmodelled `NotImplemented` — a code no AWS model
//!    declares and no SDK error branch handles — which aborted every client before a
//!    connection could exist at all.
//! 3. The `@connections` operations of that api are then reachable at its invoke URL,
//!    `https://{api-id}.execute-api.{region}.amazonaws.com/{stage}/@connections/{id}`,
//!    and answer from the API Gateway Management API's connection registry: the modelled
//!    200/200/204 for a registered connection, the modelled `GoneException` (410) once it
//!    is deleted or for a well-formed id the registry does not hold, and the
//!    invalid-`ConnectionId` refusal (400 `BadRequestException`) for an id that could
//!    never have been minted. A request through a stage the api never DEPLOYED is refused
//!    403 `ForbiddenException` instead, which is the one answer that needs both services:
//!    the stages are API Gateway V2's records and the refusal is the management API's
//!    envelope.
//!
//! Two services, one engine, one router, so this is a router suite: the deploy half is
//! served by the API Gateway V2 crate (signing name `apigateway`) and the data-plane
//! half by the API Gateway Management API crate (signing name `execute-api`).
//!
//! A connection comes into being in real AWS when a client completes a WebSocket
//! handshake against the invoke endpoint — a data-plane event no modelled operation of
//! either service serves — so it is registered here through the management API's
//! documented registration seam, the one seam that makes a connection exist. The STAGE a
//! real client deploys to is created through `CreateStage`, because the stage segment of
//! the invoke path is what decides whether that endpoint serves anything at all.
//!
//! Assembling the router costs stack, so it runs on an explicitly sized thread for the
//! same reason `boot_serve.rs` does.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_apigatewaymanagementapi_apigatewaymanagementapi as mgmt;

/// Keep this at or above the size `boot_serve.rs` uses.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every row's resources are created and addressed in.
const REGION: &str = "us-east-1";

/// A SigV4 credential scope naming the API Gateway control plane's signing name, which
/// the V2 endpoint shares.
const CONTROL_PLANE_AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/apigateway/aws4_request, \
     SignedHeaders=host, Signature=0123456789abcdef";

/// A SigV4 credential scope naming the `execute-api` signing name an `@connections`
/// caller signs with.
const EXECUTE_API_AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/execute-api/aws4_request, \
     SignedHeaders=host, Signature=0123456789abcdef";

/// The `DeploymentStatus` a released deployment carries, from the values
/// `DeploymentStatus` declares — `PENDING | FAILED | DEPLOYED`.
const DEPLOYED: &str = "DEPLOYED";

/// One row: an api to create and deploy, and the connection to drive against it.
struct Row {
    case: &'static str,
    /// The `CreateApi` body.
    api: fn() -> Value,
    /// The route keys to create under the api before deploying it.
    route_keys: &'static [&'static str],
    /// The `CreateDeployment` body.
    deployment: fn() -> Value,
    /// The stage segment the client's invoke URL carries.
    stage: &'static str,
    connection_id: &'static str,
    source_ip: &'static str,
    user_agent: &'static str,
    /// The payload `PostToConnection` pushes to the connection.
    payload: &'static [u8],
}

/// The rows differ along the api's route set, whether the deployment carries a
/// description, the stage the client addresses, the connection-id form and the payload.
///
/// Every `connection_id` is of the shape API Gateway MINTS — standard base64 of
/// `mgmt::state::CONNECTION_ID_BYTES` bytes, the 16-character single-padded token — since
/// that is the only shape a connection can be registered under, and the shape the three
/// `@connections` operations read an id by.
const ROWS: &[Row] = &[
    Row {
        case: "the three route keys a socket api needs, deployed with no description",
        api: || {
            json!({
                "name": "deploy-then-push",
                "protocolType": "WEBSOCKET",
                "routeSelectionExpression": "$request.body.action"
            })
        },
        route_keys: &["$connect", "$disconnect", "$default"],
        deployment: || json!({}),
        stage: "prod",
        connection_id: "Lx9ZKcvPmjQCJhA=",
        source_ip: "198.51.100.17",
        user_agent: "sdk-client/2.1",
        payload: b"{\"action\":\"echo\",\"message\":\"hi\"}",
    },
    Row {
        case: "a $connect-only socket api, deployed with a description",
        api: || {
            json!({
                "name": "connect-only",
                "protocolType": "WEBSOCKET",
                "routeSelectionExpression": "$request.body.action",
                "description": "one route"
            })
        },
        route_keys: &["$connect"],
        deployment: || json!({ "description": "the first cut" }),
        stage: "dev",
        connection_id: "EzCyjTUad5uklZ0=",
        source_ip: "2001:db8::2:1",
        user_agent: "curl/8.4.0",
        payload: b"",
    },
];

/// Dispatch one request and answer its status and body text.
async fn call(
    router: &MultiServiceRouter,
    authorization: &str,
    host: &str,
    method: &str,
    path: &str,
    body: Vec<u8>,
) -> (u16, String) {
    let (status, _headers, text) =
        call_with_headers(router, authorization, host, method, path, body).await;
    (status, text)
}

/// As [`call`], and also the response headers: a restJson1 client resolves the error
/// SHAPE of a failure from `x-amzn-errortype` and decodes the envelope with
/// `content-type`, so both are part of the answer an `@connections` caller gets.
async fn call_with_headers(
    router: &MultiServiceRouter,
    authorization: &str,
    host: &str,
    method: &str,
    path: &str,
    body: Vec<u8>,
) -> (u16, http::HeaderMap, String) {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header("content-type", "application/json")
        .header("authorization", authorization)
        .body(bytes::Bytes::from(body))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (
        status,
        response.headers().clone(),
        String::from_utf8_lossy(response.body().as_ref()).to_string(),
    )
}

/// One header's value as text, or `"<absent>"`.
fn header(headers: &http::HeaderMap, name: &str) -> String {
    headers
        .get(name)
        .map(|value| String::from_utf8_lossy(value.as_bytes()).to_string())
        .unwrap_or_else(|| "<absent>".to_string())
}

/// One API Gateway V2 control-plane call, as JSON.
async fn control_plane(
    router: &MultiServiceRouter,
    case: &str,
    what: &str,
    method: &str,
    path: &str,
    body: &Value,
    expected_status: u16,
) -> Value {
    let (status, text) = call(
        router,
        CONTROL_PLANE_AUTHORIZATION,
        &format!("apigateway.{REGION}.amazonaws.com"),
        method,
        path,
        body.to_string().into_bytes(),
    )
    .await;
    assert!(
        !text.contains("NotImplemented"),
        "case {case:?}: {what} must be served, not answered with the scaffold's \
         unmodelled NotImplemented: got {status} {text}"
    );
    assert_eq!(
        status, expected_status,
        "case {case:?}: expected HTTP {expected_status} from {what}, got {status} {text}"
    );
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("case {case:?}: {what} must answer JSON ({e}): {text}"))
}

/// A required string member of a response body.
fn as_str(case: &str, what: &str, body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| panic!("case {case:?}: {what} must carry {member:?}, got {body}"))
}

/// The error SHAPE a restJson1 failure names: the `x-amzn-errortype` response header, where
/// a restJson1 client resolves it. NOT the body — a restJson1 error body carries the error
/// shape's own modelled members and nothing else, so a memberless shape's body is `{}`.
fn error_code(headers: &http::HeaderMap) -> String {
    header(headers, "x-amzn-errortype")
}

#[test]
fn a_websocket_api_deploys_and_its_connections_endpoint_serves_the_registry() {
    std::thread::Builder::new()
        .name("apigatewayv2-websocket-deploy".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            // `enable_io`: a WEBSOCKET api's data plane is a SOCKET the core opens when the
            // api is created, so the runtime this check creates one on has to be able to
            // bind one.
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_io()
                .build()
                .expect("build a current-thread runtime");
            runtime.block_on(async {
                let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
                let mut router = build_router(Arc::clone(&engine));
                // Signature verification belongs to the emulator, not to this check.
                router.set_request_authentication(false);

                for row in ROWS {
                    // 1. The api, its integration and its routes.
                    let created = control_plane(
                        &router,
                        row.case,
                        "CreateApi",
                        "POST",
                        "/v2/apis",
                        &(row.api)(),
                        201,
                    )
                    .await;
                    let api_id = as_str(row.case, "CreateApi", &created, "apiId");
                    // Every row is a WEBSOCKET api, so its `ApiEndpoint` is the URI a
                    // WebSocket client library opens: the RFC 6455 scheme `wss` on the
                    // address that api's own data-plane socket is served at, which is the
                    // one a client on this machine can complete a handshake against (the
                    // apigatewayv2 crate's `api_endpoint_scheme` and
                    // `websocket_transport_data_plane` suites own that rule). The
                    // `@connections` management endpoint below is a separate, HTTPS control
                    // surface.
                    let announced = created
                        .get("apiEndpoint")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    assert!(
                        announced.starts_with("wss://"),
                        "case {:?}: the api must report a wss:// endpoint, got {created}",
                        row.case
                    );
                    assert!(
                        !announced.contains("amazonaws.com"),
                        "case {:?}: the announced endpoint must be the socket the api is \
                         reached at, not a host nothing here serves, got {created}",
                        row.case
                    );

                    let integration = control_plane(
                        &router,
                        row.case,
                        "CreateIntegration",
                        "POST",
                        &format!("/v2/apis/{api_id}/integrations"),
                        &json!({ "integrationType": "MOCK" }),
                        201,
                    )
                    .await;
                    let integration_id =
                        as_str(row.case, "CreateIntegration", &integration, "integrationId");

                    for route_key in row.route_keys {
                        let route = control_plane(
                            &router,
                            row.case,
                            "CreateRoute",
                            "POST",
                            &format!("/v2/apis/{api_id}/routes"),
                            &json!({
                                "routeKey": route_key,
                                "target": format!("integrations/{integration_id}")
                            }),
                            201,
                        )
                        .await;
                        assert_eq!(
                            route.get("routeKey").and_then(Value::as_str),
                            Some(*route_key),
                            "case {:?}: expected the route key {route_key:?}, got {route}",
                            row.case
                        );
                    }

                    // 2. The deploy step this issue was filed about.
                    let deployment = control_plane(
                        &router,
                        row.case,
                        "CreateDeployment",
                        "POST",
                        &format!("/v2/apis/{api_id}/deployments"),
                        &(row.deployment)(),
                        201,
                    )
                    .await;
                    let deployment_id =
                        as_str(row.case, "CreateDeployment", &deployment, "deploymentId");
                    assert!(
                        !deployment_id.is_empty(),
                        "case {:?}: CreateDeployment must mint a DeploymentId, got {deployment}",
                        row.case
                    );
                    assert_eq!(
                        deployment.get("deploymentStatus").and_then(Value::as_str),
                        Some(DEPLOYED),
                        "case {:?}: expected DeploymentStatus {DEPLOYED}, got {deployment}",
                        row.case
                    );

                    let fetched = control_plane(
                        &router,
                        row.case,
                        "GetDeployment",
                        "GET",
                        &format!("/v2/apis/{api_id}/deployments/{deployment_id}"),
                        &json!({}),
                        200,
                    )
                    .await;
                    assert_eq!(
                        as_str(row.case, "GetDeployment", &fetched, "deploymentId"),
                        deployment_id,
                        "case {:?}: GetDeployment must report the deployment that was \
                         created, got {fetched}",
                        row.case
                    );

                    // 3. The STAGE the client's invoke URL names, with that deployment
                    //    released to it. The stage is what makes the api's `@connections`
                    //    endpoint serve anything at all: an addressing whose stage no api
                    //    has deployed is not the caller's to invoke (the 403 below).
                    let stage = control_plane(
                        &router,
                        row.case,
                        "CreateStage",
                        "POST",
                        &format!("/v2/apis/{api_id}/stages"),
                        &json!({ "stageName": row.stage, "deploymentId": deployment_id }),
                        201,
                    )
                    .await;
                    assert_eq!(
                        stage.get("stageName").and_then(Value::as_str),
                        Some(row.stage),
                        "case {:?}: expected the stage {:?} to be created, got {stage}",
                        row.case,
                        row.stage
                    );

                    // 4. The @connections endpoint of that api, at its invoke URL.
                    let host = format!("{api_id}.execute-api.{REGION}.amazonaws.com");
                    let connected_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
                    assert!(
                        mgmt::state::register_connection(
                            &*engine,
                            row.connection_id,
                            mgmt::state::Identity {
                                source_ip: row.source_ip.to_string(),
                                user_agent: row.user_agent.to_string(),
                            },
                            connected_at,
                        ),
                        "case {:?}: the registration seam must register the connection",
                        row.case
                    );

                    let connection_path =
                        format!("/{}/@connections/{}", row.stage, row.connection_id);
                    let (status, text) = call(
                        &router,
                        EXECUTE_API_AUTHORIZATION,
                        &host,
                        "GET",
                        &connection_path,
                        Vec::new(),
                    )
                    .await;
                    assert_eq!(
                        status, 200,
                        "case {:?}: GetConnection at {connection_path:?} must answer 200, \
                         got {status} {text}",
                        row.case
                    );
                    let reported: Value = serde_json::from_str(&text).unwrap_or_else(|e| {
                        panic!("case {:?}: GetConnection must answer JSON ({e}): {text}", row.case)
                    });
                    assert_eq!(
                        reported.pointer("/identity/sourceIp").and_then(Value::as_str),
                        Some(row.source_ip),
                        "case {:?}: expected the registered identity, got {reported}",
                        row.case
                    );

                    let (status, text) = call(
                        &router,
                        EXECUTE_API_AUTHORIZATION,
                        &host,
                        "POST",
                        &connection_path,
                        row.payload.to_vec(),
                    )
                    .await;
                    assert_eq!(
                        status, 200,
                        "case {:?}: PostToConnection at {connection_path:?} must answer 200, \
                         got {status} {text}",
                        row.case
                    );
                    let stored = mgmt::state::connection(&*engine, row.connection_id)
                        .unwrap_or_else(|| {
                            panic!("case {:?}: the connection must still exist", row.case)
                        });
                    assert_eq!(
                        stored.messages.as_slice(),
                        &[row.payload.to_vec()],
                        "case {:?}: expected the pushed payload to be delivered, got {:?}",
                        row.case,
                        stored.messages
                    );

                    // An id that could never have been minted names no connection
                    // anywhere, so it is refused as an invalid `ConnectionId` — the
                    // endpoint is answering from the value and not from the path.
                    let (status, headers, text) = call_with_headers(
                        &router,
                        EXECUTE_API_AUTHORIZATION,
                        &host,
                        "GET",
                        &format!("/{}/@connections/never-registered", row.stage),
                        Vec::new(),
                    )
                    .await;
                    assert_eq!(
                        status, 400,
                        "case {:?}: a malformed id must answer 400, got {status} {text}",
                        row.case
                    );
                    assert_eq!(
                        error_code(&headers),
                        "BadRequestException",
                        "case {:?}: a malformed id must answer BadRequestException, got \
                         {text}",
                        row.case
                    );

                    // A WELL-FORMED id the registry does not hold is the modelled Gone
                    // instead: it could have named a connection, and a caller prunes its
                    // own record on that answer.
                    let (status, headers, text) = call_with_headers(
                        &router,
                        EXECUTE_API_AUTHORIZATION,
                        &host,
                        "GET",
                        &format!("/{}/@connections/AAAAAAAAAAAAAAA=", row.stage),
                        Vec::new(),
                    )
                    .await;
                    assert_eq!(
                        status, 410,
                        "case {:?}: a well-formed id naming no connection must answer 410, \
                         got {status} {text}",
                        row.case
                    );
                    assert_eq!(
                        error_code(&headers),
                        "GoneException",
                        "case {:?}: a well-formed id naming no connection must answer \
                         GoneException, got {text}",
                        row.case
                    );

                    // The STAGE is part of the endpoint: a stage this api never deployed
                    // serves nothing, whichever operation the request names — and the LIVE
                    // connection above is what shows the refusal is decided before the
                    // connection store is consulted.
                    for undeployed in ["nope", "Prod", "staging"] {
                        for method in ["GET", "POST", "DELETE"] {
                            let (status, headers, text) = call_with_headers(
                                &router,
                                EXECUTE_API_AUTHORIZATION,
                                &host,
                                method,
                                &format!("/{undeployed}/@connections/{}", row.connection_id),
                                Vec::new(),
                            )
                            .await;
                            assert_eq!(
                                status, 403,
                                "case {:?}: {method} through the undeployed stage \
                                 {undeployed:?} must answer 403, got {status} {text}",
                                row.case
                            );
                            assert_eq!(
                                error_code(&headers),
                                "ForbiddenException",
                                "case {:?}: {method} through {undeployed:?} must answer \
                                 ForbiddenException, got {status} {text}",
                                row.case
                            );
                            assert_eq!(
                                serde_json::from_str::<Value>(&text).ok(),
                                Some(json!({ "message": "Forbidden" })),
                                "case {:?}: expected the sentence API Gateway refuses an \
                                 endpoint with, got {text}",
                                row.case
                            );
                        }
                    }

                    let (status, text) = call(
                        &router,
                        EXECUTE_API_AUTHORIZATION,
                        &host,
                        "DELETE",
                        &connection_path,
                        Vec::new(),
                    )
                    .await;
                    assert_eq!(
                        status, 204,
                        "case {:?}: DeleteConnection at {connection_path:?} must answer 204, \
                         got {status} {text}",
                        row.case
                    );
                    let (status, headers, text) = call_with_headers(
                        &router,
                        EXECUTE_API_AUTHORIZATION,
                        &host,
                        "GET",
                        &connection_path,
                        Vec::new(),
                    )
                    .await;
                    assert_eq!(
                        status, 410,
                        "case {:?}: the deleted connection must answer 410, got {status} {text}",
                        row.case
                    );
                    assert_eq!(
                        error_code(&headers),
                        "GoneException",
                        "case {:?}: the deleted connection must answer GoneException, got \
                         {status} {text}",
                        row.case
                    );
                    // `GoneException` declares no members, so there is nothing to serialize
                    // into its body: it is the empty JSON document exactly, and the shape
                    // travels in `x-amzn-errortype` alone.
                    assert_eq!(
                        serde_json::from_str::<Value>(&text).ok(),
                        Some(json!({})),
                        "case {:?}: a memberless error shape carries no body member, got \
                         {text}",
                        row.case
                    );
                }
            });
        })
        .expect("spawn the deploy-then-push thread")
        .join()
        .expect("the deploy-then-push check must pass");
}

/// The API Gateway Management API and the Amazon Connect Participant Service are TWO
/// services behind ONE SigV4 signing name (`execute-api`), which no Smithy trait
/// expresses. With both mounted, a claim made on the signing name alone makes both claim
/// every `execute-api`-signed request and the router answers `500 AmbiguousService`, so
/// NEITHER is reachable — the whole `@connections` surface answered a retryable server
/// fault instead of its modelled 2xx or 4xx.
///
/// Each therefore claims only its own modelled path space. The rows span both path
/// spaces, the invoke-path prefix form the management API is addressed through, and both
/// endpoint hosts.
#[test]
fn the_two_execute_api_services_route_apart_under_one_signing_name() {
    /// One request both services could claim on their signing name, and what must
    /// happen to it.
    struct Claim {
        name: &'static str,
        host: &'static str,
        method: &'static str,
        path: &'static str,
        body: &'static str,
        /// The status the OWNING service answers, where pinning it does not depend on
        /// an operation that is still scaffolded. `None` pins only that exactly one
        /// service claimed the request.
        status: Option<u16>,
        /// The error SHAPE that owning service names, beside `status`.
        error_code: Option<&'static str>,
    }

    // This emulator holds NO api, so no stage of one is deployed: a `@connections` row
    // addressed through an invoke path (a stage, a custom domain's base path) is refused by
    // the management API as an endpoint that serves nothing (403 `ForbiddenException`),
    // while one at the MODELLED path addresses no stage and is refused on the
    // `ConnectionId`'s own shape instead — it could never have been MINTED — with 400
    // `BadRequestException`. Either way the answer is one only that service can give.
    const CLAIMS: &[Claim] = &[
        Claim {
            name: "GetConnection at the modelled path",
            host: "abcdef1234.execute-api.us-east-1.amazonaws.com",
            method: "GET",
            path: "/@connections/no-such-connection",
            body: "",
            status: Some(400),
            error_code: Some("BadRequestException"),
        },
        Claim {
            name: "GetConnection through a stage invoke path",
            host: "abcdef1234.execute-api.us-east-1.amazonaws.com",
            method: "GET",
            path: "/prod/@connections/no-such-connection",
            body: "",
            status: Some(403),
            error_code: Some("ForbiddenException"),
        },
        Claim {
            name: "PostToConnection through a stage invoke path",
            host: "abcdef1234.execute-api.us-east-1.amazonaws.com",
            method: "POST",
            path: "/prod/@connections/no-such-connection",
            body: "hello",
            status: Some(403),
            error_code: Some("ForbiddenException"),
        },
        Claim {
            name: "DeleteConnection through a custom domain base path",
            host: "sockets.example.com",
            method: "DELETE",
            path: "/v1/socket/@connections/no-such-connection",
            body: "",
            status: Some(403),
            error_code: Some("ForbiddenException"),
        },
        Claim {
            name: "the participant service's own path space",
            host: "participant.connect.us-east-1.amazonaws.com",
            method: "POST",
            path: "/participant/connection",
            body: r#"{"Type":["WEBSOCKET"]}"#,
            status: None,
            error_code: None,
        },
        Claim {
            name: "another of the participant service's paths",
            host: "participant.connect.us-east-1.amazonaws.com",
            method: "POST",
            path: "/participant/transcript",
            body: "{}",
            status: None,
            error_code: None,
        },
    ];

    std::thread::Builder::new()
        .name("execute-api-routing".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            // `enable_io` for the same reason as the check above: creating a WEBSOCKET api
            // opens that api's own socket.
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_io()
                .build()
                .expect("build a current-thread runtime");
            runtime.block_on(async {
                let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
                let mut router = build_router(engine);
                router.set_request_authentication(false);

                for claim in CLAIMS {
                    let (status, headers, text) = call_with_headers(
                        &router,
                        EXECUTE_API_AUTHORIZATION,
                        claim.host,
                        claim.method,
                        claim.path,
                        claim.body.as_bytes().to_vec(),
                    )
                    .await;
                    assert!(
                        !text.contains("AmbiguousService"),
                        "case {:?}: two services claimed one request: expected exactly one \
                         claimant, got {status} {text}",
                        claim.name
                    );
                    assert!(
                        !text.contains("UnknownService"),
                        "case {:?}: no service claimed the request: expected exactly one \
                         claimant, got {status} {text}",
                        claim.name
                    );
                    if let Some(expected) = claim.status {
                        assert_eq!(
                            status, expected,
                            "case {:?}: expected HTTP {expected} from the owning service, \
                             got {status} {text}",
                            claim.name
                        );
                        // And in the OWNING service's envelope, not the router's: the
                        // refusal this test exists for answered `500 AmbiguousService` in
                        // an `application/x-amz-json-1.1` body, so the media type and the
                        // error shape are as much a part of the fix as the status. Both
                        // are the management API's restJson1 answer — the shape that
                        // service refuses this addressing with.
                        assert_eq!(
                            header(&headers, "content-type"),
                            "application/json",
                            "case {:?}: expected the restJson1 media type from the owning \
                             service, got {:?} with {status} {text}",
                            claim.name,
                            header(&headers, "content-type")
                        );
                        if let Some(expected_code) = claim.error_code {
                            assert_eq!(
                                error_code(&headers),
                                expected_code,
                                "case {:?}: expected x-amzn-errortype to name the owning \
                                 service's {expected_code}, got {:?} with {status} {text}",
                                claim.name,
                                error_code(&headers)
                            );
                        }
                    }
                }
            });
        })
        .expect("spawn the execute-api routing thread")
        .join()
        .expect("the execute-api routing check must pass");
}
