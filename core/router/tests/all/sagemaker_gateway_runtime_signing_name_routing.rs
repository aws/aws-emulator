// SPDX-License-Identifier: CC0-1.0

//! Cross-service routing for the SageMaker-family `sagemaker` SigV4 signing name, as the
//! Global Inference Gateway runtime's traffic reaches it.
//!
//! Every SageMaker-family model declares the sigv4 name `sagemaker` while serving its own
//! endpoint (`gateway-runtime.sagemaker`, `inferencerouter-runtime.sagemaker`,
//! `runtime.sagemaker`, …), and the router resolves a request's routing key from the SigV4
//! credential scope first — so each of them sees the key `sagemaker`. AWS tells the
//! endpoints apart by HOST, which a client configured with one local endpoint does not
//! vary, so the only discriminator left on the wire is each model's own URI path space.
//!
//! The rule these rows pin: a request is claimed by the service whose MODEL binds the path
//! space it addresses, and by no other — so every one of these data planes is reachable on
//! the assembled router, and none of them is answered by the router's own
//! `500 AmbiguousService` (which is what several claimants earn) or `404 UnknownService`
//! (which is what none earn).
//!
//! Rows span the dimension the rule generalises over: which family member's path space the
//! request addresses, and — within the Global Inference Gateway's own space — which of its
//! two routes is addressed, including the one whose answer is an event stream.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::{MultiServiceRouter, Service};

/// A SigV4 `Authorization` header whose credential scope names the signing name the whole
/// family shares. The signature is never checked: the routers here run with request
/// authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTSAGEMAKER0000/20260101/us-east-1/sagemaker/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// A bare emulator endpoint, which names no family member's endpoint: the host a client
/// configured with one local endpoint sends.
const BARE_HOST: &str = "localhost:9297";

type GatewayRuntime =
    service_com_amazonaws_sagemakergatewayruntime_globalinferencegatewayruntime::EmulatedService<
        engine::InMemoryEngine,
    >;
type InferenceRouterRuntime =
    service_com_amazonaws_sagemakerinferencerouterruntime_sagemakerinferencerouterruntime::EmulatedService<
        engine::InMemoryEngine,
    >;
type EndpointRuntime =
    service_com_amazonaws_sagemakerruntime_amazonsagemakerruntime::EmulatedService<
        engine::InMemoryEngine,
    >;

/// Which of the family's data planes claim one request, by name, in registration order.
fn claimants(parts: &http::request::Parts) -> Vec<&'static str> {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let gateway = GatewayRuntime::new(Arc::clone(&engine));
    let inference_router = InferenceRouterRuntime::new(Arc::clone(&engine));
    let endpoint = EndpointRuntime::new(engine);
    let mut claimed = Vec::new();
    if gateway.can_handle(parts) {
        claimed.push("GlobalInferenceGatewayRuntime");
    }
    if inference_router.can_handle(parts) {
        claimed.push("SageMakerInferenceRouterRuntime");
    }
    if endpoint.can_handle(parts) {
        claimed.push("AmazonSageMakerRuntime");
    }
    claimed
}

/// The request parts the router hands to `Service::can_handle`.
fn parts(method: &str, uri: &str) -> http::request::Parts {
    http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", BARE_HOST)
        .header("content-type", "application/json")
        .header("authorization", AUTHORIZATION)
        .body(Bytes::new())
        .expect("the test request is well formed")
        .into_parts()
        .0
}

/// A router with the three data planes of the family mounted on one shared engine, exactly
/// as `router/src/lib.rs` mounts them.
fn router_with_the_family() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(GatewayRuntime::new(Arc::clone(&engine)));
    router.register_service(InferenceRouterRuntime::new(Arc::clone(&engine)));
    router.register_service(EndpointRuntime::new(engine));
    router
}

/// One answer from the router: the HTTP status and the error code it named, if any (read
/// from the `x-amzn-errortype` header the restJson1 protocol frames an error with, else
/// from the body's `__type`).
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &[u8]) -> (u16, String) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", BARE_HOST)
        .header("content-type", "application/json")
        .header("authorization", AUTHORIZATION)
        .body(Bytes::copy_from_slice(body))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let code = if header_code.is_empty() {
        serde_json::from_slice::<Value>(response.body())
            .ok()
            .and_then(|parsed| {
                parsed
                    .get("__type")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default()
    } else {
        header_code
    };
    (
        status,
        code.rsplit(['#', '/']).next().unwrap_or_default().to_string(),
    )
}

#[test]
fn each_sagemaker_data_plane_claims_the_path_space_its_own_model_binds() {
    // Every route below comes from the three models' `smithy.api#http` bindings, as each
    // crate's `OPERATIONS.md` lists them. The rows are (case, method, uri, the claimants).
    type Row<'r> = (&'r str, &'r str, &'r str, Vec<&'r str>);
    let rows: Vec<Row> = vec![
        (
            "InvokeGlobalInferenceGateway",
            "POST",
            "/global-inference-gateway/default/invocations",
            vec!["GlobalInferenceGatewayRuntime"],
        ),
        (
            "InvokeGlobalInferenceGatewayWithResponseStream",
            "POST",
            "/global-inference-gateway/default/invocations-response-stream",
            vec!["GlobalInferenceGatewayRuntime"],
        ),
        (
            "InvokeInferenceRouter",
            "POST",
            "/inference-routers/router1/invocations",
            vec!["SageMakerInferenceRouterRuntime"],
        ),
        (
            "InvokeEndpoint",
            "POST",
            "/endpoints/endpoint1/invocations",
            vec!["AmazonSageMakerRuntime"],
        ),
        (
            "a path space no model of the family binds",
            "POST",
            "/nothing-here",
            vec![],
        ),
    ];

    for (case, method, uri, expected) in rows {
        let claimed = claimants(&parts(method, uri));
        assert_eq!(
            claimed, expected,
            "case {case:?} ({method} {uri}): expected it to be claimed by {expected:?}, got \
             {claimed:?}"
        );
    }
}

#[tokio::test]
async fn a_gateway_invocation_is_served_rather_than_refused_by_the_router() {
    let router = router_with_the_family();
    // The gateway and model come from the registry the Global Inference Gateway runtime
    // publishes, so the row names no gateway by hand.
    let (gateway_id, model) =
        service_com_amazonaws_sagemakergatewayruntime_globalinferencegatewayruntime::state::gateway_registry::PUBLISHED_GATEWAYS
            .first()
            .map(|(gateway_id, models)| {
                (
                    *gateway_id,
                    *models.first().expect("a published gateway serves a model"),
                )
            })
            .expect("the emulator publishes a gateway");
    let body = serde_json::to_vec(&json!({
        "model": model,
        "messages": [{ "role": "user", "content": "routing probe" }],
    }))
    .expect("the body serializes");

    // (case, the route addressed)
    let rows: Vec<(&str, String)> = vec![
        (
            "the buffered route",
            format!("/global-inference-gateway/{gateway_id}/invocations"),
        ),
        (
            "the streaming route",
            format!("/global-inference-gateway/{gateway_id}/invocations-response-stream"),
        ),
    ];

    for (case, uri) in rows {
        let (status, code) = call(&router, "POST", &uri, &body).await;
        assert_eq!(
            (status, code.as_str()),
            (200, ""),
            "case {case:?} (POST {uri}): expected the service's own 200, got {status} {code:?}"
        );
    }

    // And the two refusals no family route may earn, on a path the Global Inference
    // Gateway's own model binds nothing for: the request reaches ITS endpoint and is
    // answered there, never by the router's cross-service refusals.
    let (status, code) = call(
        &router,
        "GET",
        &format!("/global-inference-gateway/{gateway_id}/invocations"),
        &body,
    )
    .await;
    assert_ne!(
        code, "AmbiguousService",
        "case \"a method the model does not bind\": one service claims this path space, got \
         {status} {code:?}"
    );
    assert_ne!(
        code, "UnknownService",
        "case \"a method the model does not bind\": the path space is claimed, got {status} \
         {code:?}"
    );
}
