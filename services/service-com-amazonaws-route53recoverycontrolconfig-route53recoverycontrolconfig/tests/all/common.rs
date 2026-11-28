// SPDX-License-Identifier: CC0-1.0

//! The HTTP harness every suite in this target shares: one router over a fresh engine,
//! one request helper, and the create calls the other suites build their fixtures with.
//!
//! It lives here so the suites cannot drift apart on how a request is signed, how an ARN
//! is encoded into a path, or what a create call asserts.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::Value;
use service_com_amazonaws_route53recoverycontrolconfig_route53recoverycontrolconfig::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The signing name the model gives this service; it is what the router keys on.
pub const SIGNING_NAME: &str = "route53-recovery-control-config";

/// The Region the test signs for. Recovery-control resource ARNs are account-wide, so
/// it must NOT appear in them.
pub const REGION: &str = "us-west-2";

/// A router with only this service mounted, over a fresh engine.
pub fn router() -> MultiServiceRouter {
    router_with_engine().0
}

/// The same router, together with the engine it is mounted on — for a suite that has to
/// reach this service's store directly, because the state it reads is written from
/// outside this API.
pub fn router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// The same router, reachable at `base` instead of the unserved default
/// ([`smithy_server_runtime::UNSERVED_URL_BASE`]): `base` is what `ctx.endpoint()`
/// answers, and so what an address a handler PUBLISHES for a client to dial is built
/// from.
///
/// `tests/all/cluster_endpoint_addressing.rs` drives several bases through one request
/// path, because an address that named a fixed host rather than the emulator's own would
/// pass against only one of them.
pub fn router_at(base: &str) -> MultiServiceRouter {
    let (mut router, _engine) = router_with_engine();
    router.set_url_base(
        smithy_server_runtime::EndpointUrl::parse(base)
            .unwrap_or_else(|| panic!("{base:?} is a usable emulator base URL")),
    );
    router
}

/// An ARN as one path segment or query value: every character an ARN can hold that is
/// not safe there (`:` and `/`) is percent-encoded, which is how an SDK sends it.
pub fn enc(arn: &str) -> String {
    arn.chars()
        .map(|c| match c {
            ':' => "%3A".to_string(),
            '/' => "%2F".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// Send one request and return `(status, parsed body)`.
pub async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    call_in_region(router, REGION, method, path, body).await
}

/// Send one request and return `(status, the error code its `x-amzn-errortype` header
/// names, parsed body)`.
///
/// For a refusal whose error code travels in that header and NOT in the body's `__type` —
/// which is how the layer ahead of this service states its own refusals, so a suite about
/// one has to read the header to see the code at all.
pub async fn answered(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, String, Value) {
    answered_in_region(router, REGION, method, path, body).await
}

/// The same request, addressed to and signed for `region` instead of [`REGION`] — for a
/// suite whose rule is about the Region a call is made in, rather than about one Region.
pub async fn call_in_region(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let (status, _error_type, parsed) =
        answered_in_region(router, region, method, path, body).await;
    (status, parsed)
}

/// The one request helper the three above are built from: `(status, the error code the
/// `x-amzn-errortype` header names, parsed body)`, addressed to and signed for `region`.
pub async fn answered_in_region(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, String, Value) {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(&value).expect("the request body serializes"))
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{SIGNING_NAME}.{region}.amazonaws.com{path}"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR/20260101/{region}/{SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(payload)
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, error_type, parsed)
}

/// Create one cluster and return its `Cluster` body.
pub async fn create_cluster(router: &MultiServiceRouter, body: Value) -> Value {
    let (status, created) = call(router, "POST", "/cluster", Some(body.clone())).await;
    assert_eq!(
        status, 200,
        "case {body:?}: expected CreateCluster 200, got {status} {created:?}"
    );
    created["Cluster"].clone()
}

/// Create one control panel and return its `ControlPanel` body.
pub async fn create_control_panel(router: &MultiServiceRouter, body: Value) -> Value {
    let (status, created) = call(router, "POST", "/controlpanel", Some(body.clone())).await;
    assert_eq!(
        status, 200,
        "case {body:?}: expected CreateControlPanel 200, got {status} {created:?}"
    );
    created["ControlPanel"].clone()
}

/// Create one routing control and return its `RoutingControl` body.
pub async fn create_routing_control(router: &MultiServiceRouter, body: Value) -> Value {
    let (status, created) = call(router, "POST", "/routingcontrol", Some(body.clone())).await;
    assert_eq!(
        status, 200,
        "case {body:?}: expected CreateRoutingControl 200, got {status} {created:?}"
    );
    created["RoutingControl"].clone()
}

/// A fresh control panel holding `controls` routing controls; answers its ARN and the
/// routing control ARNs in creation order.
///
/// The fixture every safety-rule suite starts from, so they cannot drift apart on how a
/// cluster, its control panel and its routing controls are built.
pub async fn panel_with_controls(
    router: &MultiServiceRouter,
    label: &str,
    controls: usize,
) -> (String, Vec<String>) {
    let cluster = create_cluster(
        router,
        serde_json::json!({ "ClusterName": format!("{label}-cluster") }),
    )
    .await;
    let cluster_arn = cluster["ClusterArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let panel = create_control_panel(
        router,
        serde_json::json!({
            "ClusterArn": cluster_arn,
            "ControlPanelName": format!("{label}-panel")
        }),
    )
    .await;
    let panel_arn = panel["ControlPanelArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let mut arns = Vec::new();
    for n in 0..controls {
        let control = create_routing_control(
            router,
            serde_json::json!({
                "ClusterArn": cluster_arn,
                "ControlPanelArn": panel_arn,
                "RoutingControlName": format!("{label}-control-{n}")
            }),
        )
        .await;
        arns.push(
            control["RoutingControlArn"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }
    (panel_arn, arns)
}

/// The `__type` of an error body, without its optional `namespace#` prefix.
pub fn error_code(body: &Value) -> String {
    let raw = body
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    raw.rsplit('#').next().unwrap_or_default().to_string()
}
