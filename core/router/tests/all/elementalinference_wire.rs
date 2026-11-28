// SPDX-License-Identifier: CC0-1.0

//! The wire every Elemental Inference suite in this test crate drives, written ONCE: the
//! router that mounts the service, the signed request builder, the answer it parses, and
//! the calls those suites make.
//!
//! A suite that writes its own drifts from the others: a credential scope that names a
//! different service or Region changes which endpoint the request reached, and this
//! service is REGIONAL, so that changes what the suite is actually asserting. The
//! plumbing therefore lives here and each suite states only its own rows.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// This service's SigV4 signing name (`OPERATIONS.md`, "Wire protocol"): the router
/// resolves the claiming service from it.
pub(crate) const SIGNING_NAME: &str = "elemental-inference";

/// The Region every request below is signed for. The service reads it out of the SigV4
/// credential scope, so the ARNs it mints carry exactly this Region.
pub(crate) const REGION: &str = "us-west-2";

/// Elemental Inference over one shared engine, exactly as `core/router/src/lib.rs` mounts
/// it.
pub(crate) fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These tests exercise the service, not SigV4: requests carry an unsigned
    // placeholder Authorization header, which is there only so the router can resolve the
    // signing name.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_elementalinference_elementalinference::EmulatedService::new(engine),
    );
    router
}

/// Send one request and return its status and parsed body (`Value::Null` when empty).
pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Vec::new, |body| {
        serde_json::to_vec(&body).expect("serialize the request body")
    });
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/{SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|e| {
            panic!(
                "{method} {uri}: body is not JSON ({e}): {}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, parsed)
}

/// How long [`poll_until`] keeps looking before it gives up.
///
/// Generous on purpose: the condition it waits for is a transition this service owes the
/// clock, which the next call materialises, so the loop normally ends on its first or
/// second look. The deadline is only there so a suite fails instead of hanging, and it is
/// wide enough that a loaded or slow host does not change the outcome.
const POLL_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

/// How long [`poll_until`] waits between two looks.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(20);

/// Look again at `GET uri` until the status and body it answers satisfy `ready`, and
/// return that answer.
///
/// A suite that needs a record the clock has already settled LOOKS AGAIN rather than
/// waiting a fixed time: this service never sleeps for a transition — it stores the due
/// instant and materialises it on the next call — so the condition, not the elapsed time,
/// is what the test depends on, and the outcome is the same on a fast host and a loaded
/// one. Panics, naming the case and the last answer, once [`POLL_DEADLINE`] has passed.
pub(crate) async fn poll_until(
    router: &MultiServiceRouter,
    uri: &str,
    case: &str,
    ready: impl Fn(u16, &Value) -> bool,
) -> (u16, Value) {
    let deadline = std::time::Instant::now() + POLL_DEADLINE;
    loop {
        let (status, body) = call(router, "GET", uri, None).await;
        if ready(status, &body) {
            return (status, body);
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: GET {uri} never reached the state the case waits for, last answer \
             {status} with {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The error code an Elemental Inference refusal carries (`restJson1` renders it in
/// `__type`), or the empty string on a success.
pub(crate) fn error_code(body: &Value) -> &str {
    body["__type"].as_str().unwrap_or("")
}

/// The `message` of a refusal, or the empty string when it carries none.
pub(crate) fn message_of(body: &Value) -> &str {
    body["message"].as_str().unwrap_or("")
}

/// A `CreateFunction` body: `name`, `target` and `request_expression` are the three
/// members the operation requires
/// (`smithy_query(query="operation",operation="CreateFunction")`).
pub(crate) fn function_body(name: &str, target: Value, request_expression: &str) -> Value {
    json!({
        "name": name,
        "target": target,
        "requestExpression": request_expression
    })
}

/// The `passthrough` member of the `FunctionTarget` union: "Send the request nowhere."
pub(crate) fn passthrough_target() -> Value {
    json!({"passthrough": {}})
}

/// The `bedrock` member of the `FunctionTarget` union. `api`'s "only valid value is
/// INVOKE_MODEL" (`BedrockFunctionTarget$api`), and the model is a model id, which the
/// member's pattern spells `<provider>.<rest>`.
pub(crate) fn bedrock_target() -> Value {
    json!({"bedrock": {"api": "INVOKE_MODEL", "model": "anthropic.claude-3-haiku"}})
}

/// A `CreateFeed` body named `name` carrying `outputs`.
pub(crate) fn feed_body(name: &str, outputs: Value) -> Value {
    json!({"name": name, "outputs": outputs})
}

/// One output named `name` carrying `config`, enabled — the three members `CreateOutput`
/// requires.
pub(crate) fn output(name: &str, config: Value) -> Value {
    json!({"name": name, "outputConfig": config, "status": "ENABLED"})
}

/// A cropping `outputConfig`, the smallest member of the `OutputConfig` union a feed can
/// carry.
pub(crate) fn cropping_config() -> Value {
    json!({"cropping": {"templateGroups": [
        {"name": "group1", "templateUris": ["s3://templates/wide.psd"]}
    ]}})
}

/// A clipping `outputConfig` with no data source attached, which a schedule action can
/// then attach a game to.
pub(crate) fn clipping_config() -> Value {
    json!({"clipping": {"callbackMetadata": "season opener"}})
}

/// Create one feed through the wire path and return its id.
pub(crate) async fn create_feed(router: &MultiServiceRouter, name: &str, outputs: Value) -> String {
    let (status, created) = call(router, "POST", "/v1/feed", Some(feed_body(name, outputs))).await;
    assert_eq!(
        status, 202,
        "CreateFeed({name:?}) is accepted, got {status} with {created}"
    );
    created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("a created feed carries an id, got {created}"))
        .to_string()
}
