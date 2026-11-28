// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for `InvokeGlobalInferenceGateway`, the buffered half of the
//! Global Inference Gateway data plane — and the harness the streaming suite beside it
//! reuses.
//!
//! Every case goes over real HTTP through the same `MultiServiceRouter` the production
//! router assembles, so what the tests exercise is what an SDK client gets.
//!
//! The assertions are RULES over TABLES of inputs, never one recorded call:
//!
//! * an invocation resolves the gateway its path names and the model its body names in the
//!   gateway registry, and answers ONE OpenAI `chat.completion` document computed from
//!   that registry, from the store's completion sequence and from the bytes that arrived —
//!   so two calls of one gateway are two distinct completions and neither is a canned blob
//!   or the request echoed back;
//! * only `model` and `stream` are read out of the body, and the body is not stored;
//! * a content type the gateway does not read is refused 415, and a body that is not one
//!   JSON object — or that repeats the top-level `model` member with different values — is
//!   refused 400, neither of them reaching the invocation log;
//! * an absent gateway and an unserved model are 404 about the gateway and about the model
//!   respectively, while a model whose targets have no capacity right now is 503.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_sagemakergatewayruntime_globalinferencegatewayruntime as gateway_runtime;
use smithy_server_runtime::MultiServiceRouter;

use gateway_runtime::state::completion::NO_MODEL_NOTICE;
use gateway_runtime::state::gateway_registry::PUBLISHED_GATEWAYS;
use gateway_runtime::state::{Gateway, InvocationRecord};
use gateway_runtime::ComAmazonawsSagemakergatewayruntimeGlobalinferencegatewayruntimeState as GatewayState;

/// A SigV4 `Authorization` header scoped to this service's signing name (`sagemaker`).
/// Request authentication is off in these tests, so the signature is not checked; the
/// scope is what the router resolves the routing key from.
pub(crate) const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEMULATOR000000000/20260101/us-east-1/sagemaker/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The `Content-Type` the gateway reads, as the documentation states it.
pub(crate) const JSON: &str = "application/json";

/// One HTTP exchange with the data plane.
pub(crate) struct Call {
    pub(crate) status: u16,
    pub(crate) headers: http::HeaderMap,
    pub(crate) body: bytes::Bytes,
}

impl Call {
    pub(crate) fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    /// The wire error SHAPE the answer names: the `x-amzn-errortype` header restJson1 sets,
    /// or the body's `__type` when no header carried it.
    pub(crate) fn error_code(&self) -> String {
        match self.headers.get("x-amzn-errortype") {
            Some(value) => value.to_str().unwrap_or_default().to_string(),
            None => self
                .json()
                .get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        }
    }

    /// The error text, under either casing: this service's modelled errors carry it in
    /// `Message`, and the two refusals its model declares no shape for in `message`.
    pub(crate) fn message(&self) -> String {
        let rendered = self.json();
        rendered
            .get("Message")
            .or_else(|| rendered.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    pub(crate) fn content_type(&self) -> String {
        self.headers
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    }
}

/// One emulator: this service mounted on a shared engine behind the production router.
pub(crate) struct Emulator {
    pub(crate) router: MultiServiceRouter,
    pub(crate) engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(gateway_runtime::EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// The invocations the data plane has logged, oldest first, as the next request would
    /// read them.
    pub(crate) fn log(&self) -> Vec<InvocationRecord> {
        self.engine
            .state::<GatewayState>()
            .lock()
            .invocations()
            .to_vec()
    }

    /// Mark one target of one gateway in or out of capacity, which is the state the 503 is
    /// decided from.
    pub(crate) fn set_capacity(&self, gateway_id: &str, model: &str, available: bool) -> bool {
        self.engine
            .state::<GatewayState>()
            .lock()
            .set_target_capacity(gateway_id, model, available)
    }
}

/// `InvokeGlobalInferenceGateway` over the wire.
pub(crate) fn invocations(gateway_id: &str) -> String {
    format!("/global-inference-gateway/{gateway_id}/invocations")
}

/// `InvokeGlobalInferenceGatewayWithResponseStream` over the wire.
pub(crate) fn response_stream(gateway_id: &str) -> String {
    format!("/global-inference-gateway/{gateway_id}/invocations-response-stream")
}

/// `POST path` with `headers` and the raw `body`, over the same router an SDK client
/// reaches.
pub(crate) async fn post(
    emulator: &Emulator,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Call {
    let mut request = http::Request::builder()
        .method("POST")
        .uri(path)
        .header("authorization", AUTHORIZATION);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let request = request
        .body(bytes::Bytes::copy_from_slice(body))
        .expect("a well-formed test request");
    let response = emulator.router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Call {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// One buffered invocation of `gateway_id` with `body`, declared as JSON.
pub(crate) async fn invoke(emulator: &Emulator, gateway_id: &str, body: &[u8]) -> Call {
    post(
        emulator,
        &invocations(gateway_id),
        &[("content-type", JSON), ("accept", JSON)],
        body,
    )
    .await
}

/// A token no completion this emulator computes can contain, carried in the prompt so a
/// test can tell an answer computed from state from the request echoed back.
pub(crate) const PROMPT_MARKER: &str = "zqx-prompt-marker";

/// An OpenAI chat-completions request body naming `model`.
pub(crate) fn chat_body(model: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "model": model,
        "messages": [{ "role": "user", "content": PROMPT_MARKER }],
    }))
    .expect("the body serializes")
}

/// The first published gateway and one of the models it serves — a pair every suite needs
/// and none of them writes out by hand, so the inventory stays the single source of it.
pub(crate) fn first_published() -> (&'static str, &'static str) {
    let (gateway_id, models) = PUBLISHED_GATEWAYS
        .first()
        .expect("the emulator publishes at least one gateway");
    (
        gateway_id,
        models.first().expect("a published gateway serves a model"),
    )
}

/// The assistant text one `chat.completion` document carries.
pub(crate) fn completion_text(document: &Value) -> String {
    document["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// RULE: a served invocation answers one `chat.completion` document whose model is the
/// model the gateway resolved, whose id is drawn from the store's completion sequence and
/// whose usage is measured over the request and the text served — for EVERY gateway and
/// model the registry holds, not one of them.
#[tokio::test]
async fn every_published_gateway_serves_the_models_its_targets_carry() {
    let emulator = Emulator::new();

    // The rows are the registry itself: every published gateway, and every model each of
    // its targets serves — so a gateway or a model no test names by hand is still covered.
    let mut served = 0usize;
    for (gateway_id, models) in PUBLISHED_GATEWAYS {
        for model in *models {
            let case = format!("{gateway_id}/{model}");
            let body = chat_body(model);
            let answered = invoke(&emulator, gateway_id, &body).await;
            assert_eq!(
                answered.status, 200,
                "case {case:?}: expected HTTP 200, got {} {:?}",
                answered.status,
                answered.json()
            );
            assert_eq!(
                answered.content_type(),
                JSON,
                "case {case:?}: the completion body is announced as {JSON}, got {:?}",
                answered.content_type()
            );
            let document = answered.json();
            assert_eq!(
                document["object"].as_str(),
                Some("chat.completion"),
                "case {case:?}: expected a chat.completion, got {document:?}"
            );
            assert_eq!(
                document["model"].as_str(),
                Some(*model),
                "case {case:?}: expected the resolved model {model:?}, got {:?}",
                document["model"]
            );
            let id = document["id"].as_str().unwrap_or_default().to_string();
            assert!(
                id.starts_with("chatcmpl-") && id.len() > "chatcmpl-".len(),
                "case {case:?}: expected a chatcmpl- completion id, got {id:?}"
            );
            assert_eq!(
                document["choices"][0]["message"]["role"].as_str(),
                Some("assistant"),
                "case {case:?}: the turn is the assistant's, got {document:?}"
            );
            assert_eq!(
                document["choices"][0]["finish_reason"].as_str(),
                Some("stop"),
                "case {case:?}: expected the finish reason stop, got {document:?}"
            );
            let text = completion_text(&document);
            assert!(
                text.starts_with(NO_MODEL_NOTICE),
                "case {case:?}: the completion says no model ran, got {text:?}"
            );
            assert!(
                text.contains(gateway_id) && text.contains(*model),
                "case {case:?}: the completion names what served it, got {text:?}"
            );
            assert!(
                !text.contains(PROMPT_MARKER),
                "case {case:?}: the completion is not the request echoed back, got {text:?}"
            );
            let usage = &document["usage"];
            let prompt = usage["prompt_tokens"].as_u64().unwrap_or_default();
            let completion = usage["completion_tokens"].as_u64().unwrap_or_default();
            assert!(
                prompt > 0 && completion > 0,
                "case {case:?}: both token counts are measured, got {usage:?}"
            );
            assert_eq!(
                usage["total_tokens"].as_u64(),
                Some(prompt + completion),
                "case {case:?}: expected the total {}, got {usage:?}",
                prompt + completion
            );
            served += 1;

            let record = emulator
                .log()
                .last()
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: the served invocation is logged"));
            assert_eq!(
                (
                    record.gateway_id.as_str(),
                    record.model.as_str(),
                    record.request_bytes,
                    record.streaming,
                ),
                (*gateway_id, *model, body.len(), false),
                "case {case:?}: expected the served invocation logged, got {record:?}"
            );
        }
    }
    assert_eq!(
        emulator.log().len(),
        served,
        "every served invocation is logged exactly once"
    );
}

/// RULE: each served invocation takes the NEXT position in the store's completion
/// sequence, so repeating one request answers a different completion id — the answer is
/// computed from state, not replayed from a canned body.
#[tokio::test]
async fn repeating_one_request_answers_a_new_completion_each_time() {
    let emulator = Emulator::new();
    let (gateway_id, model) = first_published();
    let body = chat_body(model);

    // Rows: the same request, served again and again. What must differ is the id and the
    // logged sequence; what must not is the text, which is a function of the request.
    let mut ids: Vec<String> = Vec::new();
    let mut texts: Vec<String> = Vec::new();
    for round in 1..=3usize {
        let answered = invoke(&emulator, gateway_id, &body).await;
        assert_eq!(
            answered.status, 200,
            "case round {round}: expected HTTP 200, got {} {:?}",
            answered.status,
            answered.json()
        );
        let document = answered.json();
        ids.push(document["id"].as_str().unwrap_or_default().to_string());
        texts.push(completion_text(&document));
        assert_eq!(
            emulator.log().len(),
            round,
            "case round {round}: expected {round} logged invocation(s), got {:?}",
            emulator.log().len()
        );
    }

    let mut distinct = ids.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        ids.len(),
        "each call answers its own completion id, got {ids:?}"
    );
    assert!(
        texts.windows(2).all(|pair| pair[0] == pair[1]),
        "the same request is answered with the same text, got {texts:?}"
    );

    let sequence: Vec<u64> = emulator.log().iter().map(|record| record.seq).collect();
    assert!(
        sequence.windows(2).all(|pair| pair[0] < pair[1]),
        "the logged sequence advances with each served call, got {sequence:?}"
    );
    assert_eq!(
        emulator.log().len(),
        emulator
            .engine
            .state::<GatewayState>()
            .lock()
            .invocation_count(gateway_id),
        "every logged call was served by the gateway it names"
    );
}

/// RULE: the gateway reads only `model` and `stream` out of the body and stores none of
/// it — whatever else the body carries, and however large it is, what is recorded is the
/// members read and the body's SIZE.
#[tokio::test]
async fn only_the_model_and_the_stream_members_are_read_out_of_the_body() {
    let emulator = Emulator::new();
    let (gateway_id, model) = first_published();

    // (case, body, the `stream` the caller asked for)
    let rows: Vec<(&str, Vec<u8>, bool)> = vec![
        (
            "the smallest body that names a model",
            serde_json::to_vec(&json!({ "model": model })).expect("serializes"),
            false,
        ),
        ("a chat request", chat_body(model), false),
        (
            "a body asking to stream, which this operation overrides",
            serde_json::to_vec(&json!({ "model": model, "stream": true })).expect("serializes"),
            true,
        ),
        (
            "a body asking not to stream",
            serde_json::to_vec(&json!({ "model": model, "stream": false })).expect("serializes"),
            false,
        ),
        (
            "members the gateway reads none of",
            serde_json::to_vec(&json!({
                "model": model,
                "messages": [{ "role": "user", "content": "qui\u{00e9}n sabe" }],
                "temperature": 0.5,
                "max_tokens": 16,
                "stop": ["\n\n"],
                "metadata": { "nested": { "deep": [1, 2, 3] } },
            }))
            .expect("serializes"),
            false,
        ),
        (
            "a body that repeats the model member with the SAME value",
            format!(r#"{{"model":"{model}","model":"{model}"}}"#).into_bytes(),
            false,
        ),
        (
            "a body one kibibyte of content long",
            serde_json::to_vec(&json!({
                "model": model,
                "messages": [{ "role": "user", "content": "x".repeat(1024) }],
            }))
            .expect("serializes"),
            false,
        ),
    ];

    for (case, body, stream_requested) in rows {
        let answered = invoke(&emulator, gateway_id, &body).await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: expected HTTP 200, got {} {:?}",
            answered.status,
            answered.json()
        );
        let document = answered.json();
        assert_eq!(
            document["object"].as_str(),
            Some("chat.completion"),
            "case {case:?}: the buffered route answers one document whatever `stream` said, \
             got {document:?}"
        );
        let record = emulator
            .log()
            .last()
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the served invocation is logged"));
        assert_eq!(
            (record.request_bytes, record.stream_requested, record.streaming),
            (body.len(), stream_requested, false),
            "case {case:?}: expected the body's size and the `stream` it asked for logged, \
             got {record:?}"
        );
        assert_eq!(
            record.model, model,
            "case {case:?}: expected the model {model:?} logged, got {record:?}"
        );
    }
}

/// RULE: a body the gateway cannot route is refused with a 400 validation error naming
/// what is wrong, and nothing is dispatched — so the invocation log is untouched.
#[tokio::test]
async fn a_body_that_is_not_one_routable_json_object_is_refused() {
    let emulator = Emulator::new();
    let (gateway_id, model) = first_published();
    let served = chat_body(model);

    // Rows differ along WHAT makes the body unroutable: its JSON form, the count of
    // top-level objects, the absence of `model`, and the type of each member read.
    let rows: Vec<(&str, Vec<u8>)> = vec![
        ("a body that is not JSON at all", b"not json".to_vec()),
        ("an empty body", Vec::new()),
        ("a JSON array", br#"[{"model":"m"}]"#.to_vec()),
        ("a JSON string", br#""model""#.to_vec()),
        ("a JSON number", b"7".to_vec()),
        ("JSON null", b"null".to_vec()),
        (
            "two JSON objects rather than one",
            br#"{"model":"m"}{"model":"m"}"#.to_vec(),
        ),
        ("an object naming no model", br#"{"messages":[]}"#.to_vec()),
        ("a model that is not a string", br#"{"model":7}"#.to_vec()),
        ("a model that is null", br#"{"model":null}"#.to_vec()),
        (
            "a repeated model with different values",
            br#"{"model":"one","model":"two"}"#.to_vec(),
        ),
        (
            "a stream member that is not a boolean",
            br#"{"model":"m","stream":"yes"}"#.to_vec(),
        ),
    ];

    for (case, body) in rows {
        let answered = invoke(&emulator, gateway_id, &body).await;
        assert_eq!(
            answered.status, 400,
            "case {case:?}: expected HTTP 400, got {} {:?}",
            answered.status,
            answered.json()
        );
        assert_eq!(
            answered.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answered.error_code()
        );
        assert!(
            answered.message().starts_with("Global Inference Gateway: "),
            "case {case:?}: the gateway's own refusal says so in its message, got {:?}",
            answered.message()
        );
        assert!(
            emulator.log().is_empty(),
            "case {case:?}: a refused request is not dispatched, so nothing is logged, got {:?}",
            emulator.log()
        );
    }

    // The same gateway still serves a body it can route, so the refusals above are about
    // the body and nothing else.
    let answered = invoke(&emulator, gateway_id, &served).await;
    assert_eq!(
        answered.status, 200,
        "case \"a routable body after the refusals\": expected HTTP 200, got {} {:?}",
        answered.status,
        answered.json()
    );
}

/// RULE: the gateway reads `application/json`, with any media-type parameters; it refuses
/// every other content type with a 415 and dispatches nothing.
#[tokio::test]
async fn only_json_content_types_are_read() {
    let emulator = Emulator::new();
    let (gateway_id, model) = first_published();
    let body = chat_body(model);

    // (case, the declared content type, whether the gateway reads it)
    let rows: &[(&str, &str, bool)] = &[
        ("the documented type", "application/json", true),
        (
            "the documented type with the parameter the documentation names",
            "application/json; charset=utf-8",
            true,
        ),
        (
            "parameters without a space, and in another case",
            "application/json;charset=UTF-8",
            true,
        ),
        ("the media type in upper case", "APPLICATION/JSON", true),
        ("another structured suffix", "application/jsonl", false),
        ("text", "text/plain", false),
        ("another AWS protocol's type", "application/x-amz-json-1.1", false),
        ("opaque bytes", "application/octet-stream", false),
        ("a subtype wildcard, which is a range and not a type", "application/*", false),
        ("a value that is no media type at all", "not a media type", false),
    ];

    let mut served = 0usize;
    for (case, content_type, read) in rows {
        let answered = post(
            &emulator,
            &invocations(gateway_id),
            &[("content-type", content_type)],
            &body,
        )
        .await;
        if *read {
            served += 1;
            assert_eq!(
                answered.status, 200,
                "case {case:?} ({content_type}): expected HTTP 200, got {} {:?}",
                answered.status,
                answered.json()
            );
            let record = emulator
                .log()
                .last()
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: the served invocation is logged"));
            assert_eq!(
                record.content_type, *content_type,
                "case {case:?}: expected the declared content type logged, got {record:?}"
            );
        } else {
            assert_eq!(
                answered.status, 415,
                "case {case:?} ({content_type}): expected HTTP 415, got {} {:?}",
                answered.status,
                answered.json()
            );
            assert_eq!(
                answered.error_code(),
                "UnsupportedMediaType",
                "case {case:?}: expected UnsupportedMediaType, got {:?}",
                answered.error_code()
            );
            assert!(
                answered.message().starts_with("Global Inference Gateway: ")
                    && answered.message().contains(content_type),
                "case {case:?}: the refusal names the type it refused, got {:?}",
                answered.message()
            );
        }
        assert_eq!(
            emulator.log().len(),
            served,
            "case {case:?}: only a read request is dispatched, got {:?}",
            emulator.log()
        );
    }
}

/// RULE: an absent gateway, a model no target of the gateway serves, and a model whose
/// targets have no capacity right now are three different answers — 404 about the gateway,
/// 404 about the model, and 503 — and only the served call reaches the log.
#[tokio::test]
async fn the_registry_decides_between_absent_unserved_and_without_capacity() {
    let emulator = Emulator::new();
    let (gateway_id, model) = first_published();
    // A model the FIRST published gateway does not serve, taken from another published
    // gateway: a model one gateway serves is not thereby served by another.
    let elsewhere = PUBLISHED_GATEWAYS
        .iter()
        .find(|(other, _)| *other != gateway_id)
        .and_then(|(_, models)| models.first().copied())
        .expect("the emulator publishes a second gateway");

    // (case, the gateway addressed, the model named, status, error shape, resource type)
    let rows: Vec<(&str, &str, &str, u16, &str, &str)> = vec![
        (
            "a gateway id the registry does not hold",
            "nosuchgateway",
            model,
            404,
            "ResourceNotFoundException",
            "gateway",
        ),
        (
            "a model no target of this gateway serves",
            gateway_id,
            "no-such-model",
            404,
            "ResourceNotFoundException",
            "model",
        ),
        (
            "a model another gateway of the registry serves",
            gateway_id,
            elsewhere,
            404,
            "ResourceNotFoundException",
            "model",
        ),
    ];

    for (case, gateway, named, status, code, resource_type) in rows {
        let answered = invoke(&emulator, gateway, &chat_body(named)).await;
        assert_eq!(
            answered.status, status,
            "case {case:?}: expected HTTP {status}, got {} {:?}",
            answered.status,
            answered.json()
        );
        assert_eq!(
            answered.error_code(),
            code,
            "case {case:?}: expected {code}, got {:?}",
            answered.error_code()
        );
        let document = answered.json();
        assert_eq!(
            document["ResourceType"].as_str(),
            Some(resource_type),
            "case {case:?}: expected the resource type {resource_type:?}, got {document:?}"
        );
        let addressed = if resource_type == "gateway" {
            gateway
        } else {
            named
        };
        assert_eq!(
            document["ResourceId"].as_str(),
            Some(addressed),
            "case {case:?}: expected the resource id {addressed:?}, got {document:?}"
        );
        assert!(
            answered.message().starts_with("Global Inference Gateway: "),
            "case {case:?}: the gateway's own refusal says so in its message, got {:?}",
            answered.message()
        );
        assert!(
            emulator.log().is_empty(),
            "case {case:?}: a refused request is not dispatched, got {:?}",
            emulator.log()
        );
    }

    // The same model, one target at a time: out of capacity is 503 and NOT 404, and
    // putting the capacity back serves it again. Capacity is the only thing that changed.
    assert!(
        emulator.set_capacity(gateway_id, model, false),
        "the registry holds the target whose capacity the case changes"
    );
    let refused = invoke(&emulator, gateway_id, &chat_body(model)).await;
    assert_eq!(
        refused.status, 503,
        "case \"a served model with no available target\": expected HTTP 503, got {} {:?}",
        refused.status,
        refused.json()
    );
    assert_eq!(
        refused.error_code(),
        "ServiceUnavailableException",
        "case \"a served model with no available target\": expected ServiceUnavailableException, \
         got {:?}",
        refused.error_code()
    );
    assert!(
        refused.message().starts_with("Global Inference Gateway: ") && refused.message().contains(model),
        "case \"a served model with no available target\": the refusal names the model, got {:?}",
        refused.message()
    );
    assert!(
        emulator.log().is_empty(),
        "case \"a served model with no available target\": the request was not dispatched, got {:?}",
        emulator.log()
    );

    assert!(
        emulator.set_capacity(gateway_id, model, true),
        "the target is still in the registry"
    );
    let served = invoke(&emulator, gateway_id, &chat_body(model)).await;
    assert_eq!(
        served.status, 200,
        "case \"capacity restored\": expected HTTP 200, got {} {:?}",
        served.status,
        served.json()
    );
    assert_eq!(
        emulator.log().len(),
        1,
        "case \"capacity restored\": exactly the served call is logged, got {:?}",
        emulator.log()
    );
}

/// RULE: the registry — not a list written into the handler — decides which gateways
/// exist, so a gateway put into it is served and one taken out of it is not.
#[tokio::test]
async fn a_gateway_the_registry_gains_is_served_and_one_it_loses_is_not() {
    let emulator = Emulator::new();
    let injected = "injectedgateway";
    let model = "private-model-v1";

    let before = invoke(&emulator, injected, &chat_body(model)).await;
    assert_eq!(
        before.status, 404,
        "case \"before the gateway is in the registry\": expected HTTP 404, got {} {:?}",
        before.status,
        before.json()
    );

    emulator
        .engine
        .state::<GatewayState>()
        .lock()
        .put_gateway(Gateway::new(
            injected,
            "us-east-1",
            &[(model, true), ("drained-model-v1", false)],
            std::time::SystemTime::now(),
        ));

    // (case, the model named, the status owed)
    let rows: &[(&str, &str, u16)] = &[
        ("the model its available target serves", model, 200),
        ("the model its drained target serves", "drained-model-v1", 503),
        ("a model it does not serve", "absent-model-v1", 404),
    ];
    for (case, named, status) in rows {
        let answered = invoke(&emulator, injected, &chat_body(named)).await;
        assert_eq!(
            answered.status, *status,
            "case {case:?}: expected HTTP {status}, got {} {:?}",
            answered.status,
            answered.json()
        );
    }

    let gateway = emulator
        .engine
        .state::<GatewayState>()
        .lock()
        .gateway(injected)
        .cloned()
        .expect("the injected gateway is in the registry");
    assert_eq!(
        gateway.arn,
        format!(
            "arn:aws:sagemaker:us-east-1:{}:global-inference-gateway/{injected}",
            engine::EMULATOR_ACCOUNT_ID
        ),
        "a gateway carries the ARN IAM policies name it by, got {:?}",
        gateway.arn
    );
}
