// SPDX-License-Identifier: CC0-1.0

//! The Bedrock Runtime streaming surface, end to end over HTTP: `ConverseStream`
//! and `InvokeModelWithResponseStream`, plus the request rules every operation that
//! invokes a model follows — the `modelId` it names the model with, and the
//! `projectIdentifier` it names the project the invocation runs under.
//!
//! Every case drives the real router, so what is asserted is the wire behaviour a
//! client sees — status, headers, and the `vnd.amazon.eventstream` frames — never a
//! handler called directly.
//!
//! The rule each test states is asserted over a SET of inputs: the tables differ
//! along what the rule generalises over (model family, message count, text size and
//! script, token cap, identifier form), so no row is the specification on its own.
//! The streaming surfaces are checked AGAINST their non-streaming siblings, which is
//! the invariant that matters: `ConverseStream` frames the turn `Converse` serves,
//! and `InvokeModelWithResponseStream` chunks the payload `InvokeModel` returns.

use std::sync::Arc;

use aws_smithy_eventstream::frame::read_message_from;
use aws_smithy_types::event_stream::HeaderValue;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::state;
use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A host-style endpoint for the service, so a request routes even if the SigV4
/// scope below ever changes.
const ENDPOINT: &str = "https://bedrock-runtime.us-east-1.amazonaws.com";

/// A SigV4 `Authorization` header scoped to this service's signing name
/// (`bedrock`). Request authentication is off in these tests, so the signature is
/// not checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEMULATOR000000000/20260101/us-east-1/bedrock/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The media type an event-stream body carries, in both directions.
const EVENT_STREAM: &str = "application/vnd.amazon.eventstream";

/// The frames a `ConverseStream` turn that runs to completion emits, in order —
/// `ConverseStreamOutput`'s own member names (smithy_query(query="shape",
/// shape="ConverseStreamOutput")). A content block arrives as SEVERAL
/// `contentBlockDelta` events a client appends, so the run of them counts as the
/// one entry here (see [`delta_runs_collapsed`]).
const CONVERSE_STREAM_FRAMES: [&str; 5] = [
    "messageStart",
    "contentBlockDelta",
    "contentBlockStop",
    "messageStop",
    "metadata",
];

/// `frames` with each RUN of consecutive `contentBlockDelta` events reduced to
/// one entry, so a frame sequence can be compared against the event order a turn
/// emits however many increments one block is delivered in.
fn delta_runs_collapsed(frames: &[&str]) -> Vec<String> {
    let mut collapsed: Vec<String> = Vec::new();
    for frame in frames {
        let repeats_a_delta = *frame == "contentBlockDelta"
            && collapsed.last().is_some_and(|last| last == frame);
        if !repeats_a_delta {
            collapsed.push((*frame).to_string());
        }
    }
    collapsed
}

fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// One HTTP exchange.
struct Call {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Call {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "response body is not JSON ({error}): {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The wire error code: the `x-amzn-errortype` header restJson1 sets, or the
    /// body's `__type` when a value could not be rendered as a header.
    fn error_code(&self) -> String {
        if let Some(value) = self.headers.get("x-amzn-errortype") {
            return value.to_str().unwrap_or_default().to_string();
        }
        self.json()
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    fn header(&self, name: &str) -> String {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    }

    /// The essence of the `content-type` header, without any parameters.
    fn media_type(&self) -> String {
        self.header("content-type")
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
    }
}

async fn send(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> Call {
    let mut request = http::Request::builder()
        .method(method)
        .uri(format!("{ENDPOINT}{path}"))
        .header("authorization", AUTHORIZATION)
        .header("host", "bedrock-runtime.us-east-1.amazonaws.com");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let request = request
        .body(bytes::Bytes::from(body))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Call {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

async fn post_json(router: &MultiServiceRouter, path: &str, body: &Value) -> Call {
    send(
        router,
        "POST",
        path,
        &[("content-type", "application/json")],
        serde_json::to_vec(body).expect("a serializable test body"),
    )
    .await
}

/// Decode an event-stream response body, asserting the frame headers an SDK
/// decoder requires. Each entry is `(event type, payload)`.
pub(crate) fn decode_frames(case: &str, body: &[u8]) -> Vec<(String, Value)> {
    let mut buffer = bytes::Bytes::copy_from_slice(body);
    let mut frames = Vec::new();
    while !buffer.is_empty() {
        let message = read_message_from(&mut buffer).unwrap_or_else(|error| {
            panic!("case {case:?}: the body must decode as event-stream frames: {error}")
        });
        let header = |name: &str| {
            message
                .headers()
                .iter()
                .find(|header| header.name().as_str() == name)
                .map(|header| match header.value() {
                    HeaderValue::String(value) => value.as_str().to_string(),
                    other => format!("{other:?}"),
                })
        };
        assert_eq!(
            header(":message-type").as_deref(),
            Some("event"),
            "case {case:?}: a response frame must be an event, got {:?}",
            header(":message-type")
        );
        assert_eq!(
            header(":content-type").as_deref(),
            Some("application/json"),
            "case {case:?}: an event payload is JSON, got {:?}",
            header(":content-type")
        );
        let event_type = header(":event-type")
            .unwrap_or_else(|| panic!("case {case:?}: a response frame named no :event-type"));
        let payload: Value = serde_json::from_slice(message.payload()).unwrap_or_else(|error| {
            panic!("case {case:?}: the {event_type} payload must be JSON: {error}")
        });
        // Bedrock pads every event payload with the information-free `p` member;
        // a client's decoder sees it on every frame.
        assert!(
            payload.get("p").and_then(Value::as_str).is_some(),
            "case {case:?}: the {event_type} payload must carry Bedrock's `p` padding, got \
             {payload}"
        );
        frames.push((event_type, payload));
    }
    frames
}

/// One conversation, and the model it is sent to.
struct ConverseRow {
    case: &'static str,
    /// A model the registry serves and the Converse API supports — one row per
    /// response family, which is what the Converse surface generalises over.
    model_id: &'static str,
    system: Value,
    messages: Value,
    /// `inferenceConfig.maxTokens`, where the row is about the generation cap.
    max_tokens: Option<i64>,
}

fn converse_rows() -> Vec<ConverseRow> {
    vec![
        ConverseRow {
            case: "one short user turn",
            model_id: "anthropic.claude-3-haiku-20240307-v1:0",
            system: json!([]),
            messages: json!([{"role": "user", "content": [{"text": "Name one planet."}]}]),
            max_tokens: None,
        },
        ConverseRow {
            case: "three turns with a system prompt",
            model_id: "amazon.titan-text-premier-v1:0",
            system: json!([{"text": "You are terse."}]),
            messages: json!([
                {"role": "user", "content": [{"text": "Name two planets."}]},
                {"role": "assistant", "content": [{"text": "Mars and Venus."}]},
                {"role": "user", "content": [{"text": "And one moon?"}]},
            ]),
            max_tokens: None,
        },
        ConverseRow {
            case: "non-ascii content",
            model_id: "amazon.nova-pro-v1:0",
            system: json!([]),
            messages: json!([{
                "role": "user",
                "content": [{"text": "Übersetze: 木星 is the largest planet — richtig?"}],
            }]),
            max_tokens: None,
        },
        ConverseRow {
            case: "several content blocks in one turn",
            model_id: "mistral.mistral-7b-instruct-v0:2",
            system: json!([]),
            messages: json!([{
                "role": "user",
                "content": [
                    {"text": "List three primes."},
                    {"text": "Then explain why two of them are twins."},
                ],
            }]),
            max_tokens: None,
        },
        ConverseRow {
            case: "generation capped at three tokens",
            model_id: "meta.llama3-8b-instruct-v1:0",
            system: json!([]),
            messages: json!([{"role": "user", "content": [{"text": "Explain idempotency."}]}]),
            max_tokens: Some(3),
        },
        ConverseRow {
            case: "generation capped well above the completion",
            model_id: "amazon.nova-lite-v1:0",
            system: json!([]),
            messages: json!([{
                "role": "user",
                "content": [{"text": "Reply with only the lowercase English word for 2+3."}],
            }]),
            max_tokens: Some(20),
        },
        ConverseRow {
            case: "a long prompt",
            model_id: "cohere.command-r-plus-v1:0",
            system: json!([{"text": "Answer in one sentence."}]),
            messages: json!([{
                "role": "user",
                "content": [{"text": "word ".repeat(400)}],
            }]),
            max_tokens: None,
        },
    ]
}

impl ConverseRow {
    /// The request body both Converse surfaces are sent.
    fn request(&self) -> Value {
        let mut request = json!({ "messages": self.messages });
        if self.system.as_array().is_some_and(|blocks| !blocks.is_empty()) {
            request["system"] = self.system.clone();
        }
        if let Some(max_tokens) = self.max_tokens {
            request["inferenceConfig"] = json!({ "maxTokens": max_tokens });
        }
        request
    }
}

/// One `InvokeModel` request, per response family, with where that family's
/// response envelope reports the turn.
struct InvokeRow {
    case: &'static str,
    model_id: &'static str,
    body: &'static str,
    /// The event name of every chunk the stream must carry, in order, for a
    /// family whose streaming protocol NAMES its events (see [`event_name`]);
    /// empty for a family whose chunks are a flat envelope and name none.
    events: &'static [&'static str],
    /// Where the unary `InvokeModel` body reports the generated text, the stop
    /// reason and the two token counts — JSON pointers into that family's own
    /// response envelope.
    text_pointer: &'static str,
    stop_pointer: &'static str,
    input_tokens_pointer: &'static str,
    output_tokens_pointer: &'static str,
}

/// Model ids from "Amazon Bedrock base model IDs (on-demand throughput)", the
/// request and response envelopes from "Inference request parameters and response
/// fields for foundation models" and each family's streaming event sequence from
/// that same page, all in the Amazon Bedrock User Guide. One row per response
/// family the registry serves; the last row is a model the Converse API does NOT
/// serve, so response streaming must still serve it.
const INVOKE_ROWS: &[InvokeRow] = &[
    InvokeRow {
        case: "anthropic messages",
        model_id: "anthropic.claude-3-haiku-20240307-v1:0",
        body: r#"{"anthropic_version":"bedrock-2023-05-31","max_tokens":256,"messages":[{"role":"user","content":[{"type":"text","text":"Name one planet."}]}]}"#,
        events: &[
            "message_start",
            "content_block_start",
            "content_block_delta",
            "content_block_stop",
            "message_delta",
            "message_stop",
        ],
        text_pointer: "/content/0/text",
        stop_pointer: "/stop_reason",
        input_tokens_pointer: "/usage/input_tokens",
        output_tokens_pointer: "/usage/output_tokens",
    },
    InvokeRow {
        case: "titan text",
        model_id: "amazon.titan-text-premier-v1:0",
        body: r#"{"inputText":"Write a haiku about persistent state.","textGenerationConfig":{"maxTokenCount":512}}"#,
        // Titan's chunks are one flat envelope whose terminal one fills in the
        // completion reason and the output token count.
        events: &[],
        text_pointer: "/results/0/outputText",
        stop_pointer: "/results/0/completionReason",
        input_tokens_pointer: "/inputTextTokenCount",
        output_tokens_pointer: "/results/0/tokenCount",
    },
    InvokeRow {
        case: "nova",
        model_id: "amazon.nova-lite-v1:0",
        body: r#"{"system":[{"text":"You are terse."}],"messages":[{"role":"user","content":[{"text":"Name two planets."}]}]}"#,
        events: &[
            "messageStart",
            "contentBlockDelta",
            "contentBlockStop",
            "messageStop",
            "metadata",
        ],
        text_pointer: "/output/message/content/0/text",
        stop_pointer: "/stopReason",
        input_tokens_pointer: "/usage/inputTokens",
        output_tokens_pointer: "/usage/outputTokens",
    },
    InvokeRow {
        case: "llama, generation capped at three tokens",
        model_id: "meta.llama3-70b-instruct-v1:0",
        body: r#"{"prompt":"Explain idempotency to a cat.","max_gen_len":3}"#,
        events: &[],
        text_pointer: "/generation",
        stop_pointer: "/stop_reason",
        input_tokens_pointer: "/prompt_token_count",
        output_tokens_pointer: "/generation_token_count",
    },
    InvokeRow {
        case: "mistral",
        model_id: "mistral.mixtral-8x7b-instruct-v0:1",
        body: r#"{"prompt":"List two idempotent verbs.","max_tokens":128}"#,
        events: &[],
        text_pointer: "/outputs/0/text",
        stop_pointer: "/outputs/0/stop_reason",
        input_tokens_pointer: "",
        output_tokens_pointer: "",
    },
    InvokeRow {
        case: "cohere command, which Converse does not serve",
        model_id: "cohere.command-text-v14",
        body: r#"{"prompt":"Describe a page token.","max_tokens":64}"#,
        events: &["text-generation", "stream-end"],
        text_pointer: "/generations/0/text",
        stop_pointer: "/generations/0/finish_reason",
        input_tokens_pointer: "",
        output_tokens_pointer: "",
    },
];

/// The key Amazon Bedrock itself adds to the terminal chunk of a response stream.
const INVOCATION_METRICS: &str = "amazon-bedrock-invocationMetrics";

/// The name a streaming client reads a chunk's event identity from.
///
/// The Anthropic Messages API carries it in the payload's `type` member and the
/// Cohere stream in `event_type`; the Converse-shaped Amazon Nova events are each
/// a single named key. A family whose chunks are a flat envelope (Amazon Titan,
/// Meta Llama, Mistral) names no event, which is `None`.
fn event_name(payload: &Value) -> Option<String> {
    for member in ["type", "event_type"] {
        if let Some(name) = payload.get(member).and_then(Value::as_str) {
            return Some(name.to_string());
        }
    }
    let keys: Vec<&String> = payload
        .as_object()
        .map(|object| {
            object
                .keys()
                .filter(|key| key.as_str() != INVOCATION_METRICS)
                .collect()
        })
        .unwrap_or_default();
    match keys.as_slice() {
        [only] => Some((*only).clone()),
        _ => None,
    }
}

/// The `chunk` payloads of a response-stream body: every frame is a `chunk`, and
/// `PayloadPart$bytes` is a blob, so restJson1 renders it base64.
pub(crate) fn chunk_payloads(case: &str, body: &[u8]) -> Vec<Value> {
    let frames = decode_frames(case, body);
    let order: Vec<&str> = frames.iter().map(|(name, _)| name.as_str()).collect();
    assert!(
        order.iter().all(|name| *name == "chunk"),
        "case {case:?}: a response stream carries only chunk events, got {order:?}"
    );
    frames
        .iter()
        .map(|(_, frame)| {
            let encoded = frame["bytes"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a chunk carried no bytes: {frame}"));
            let decoded = aws_smithy_types::base64::decode(encoded)
                .unwrap_or_else(|error| panic!("case {case:?}: chunk bytes must be base64: {error}"));
            serde_json::from_slice(&decoded).unwrap_or_else(|error| {
                panic!(
                    "case {case:?}: a chunk payload must be JSON: {error} ({})",
                    String::from_utf8_lossy(&decoded)
                )
            })
        })
        .collect()
}

/// Whether `payload` mentions `needle` as a string value anywhere inside it.
fn mentions(payload: &Value, needle: &str) -> bool {
    match payload {
        Value::String(value) => value == needle,
        Value::Array(items) => items.iter().any(|item| mentions(item, needle)),
        Value::Object(members) => members.values().any(|member| mentions(member, needle)),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// ConverseStream
// ---------------------------------------------------------------------------

/// RULE: `ConverseStream` serves the same turn `Converse` serves and frames it as
/// the `ConverseStreamOutput` events — `messageStart`, the `contentBlockDelta`
/// events and `contentBlockStop` per content block, `messageStop`, `metadata` — in
/// a `application/vnd.amazon.eventstream` body, with the assistant text, the stop
/// reason and the token usage identical to what `Converse` reports for the same
/// request.
#[tokio::test]
async fn converse_stream_frames_the_turn_converse_serves() {
    let router = emulator();

    for row in converse_rows() {
        let case = row.case;
        let request = row.request();

        let converse = post_json(
            &router,
            &format!("/model/{}/converse", row.model_id),
            &request,
        )
        .await;
        assert_eq!(
            converse.status,
            200,
            "case {case:?}: expected Converse to answer 200, got {} / {}",
            converse.status,
            String::from_utf8_lossy(&converse.body)
        );
        let converse = converse.json();
        let expected_text = converse["output"]["message"]["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: Converse answered no text: {converse}"))
            .to_string();
        let expected_stop = converse["stopReason"].as_str().unwrap_or_default().to_string();
        let expected_usage = converse["usage"].clone();

        let stream = post_json(
            &router,
            &format!("/model/{}/converse-stream", row.model_id),
            &request,
        )
        .await;
        assert_eq!(
            stream.status,
            200,
            "case {case:?}: expected ConverseStream to answer 200, got {} / {}",
            stream.status,
            String::from_utf8_lossy(&stream.body)
        );
        assert_eq!(
            stream.media_type(),
            EVENT_STREAM,
            "case {case:?}: expected content-type {EVENT_STREAM}, got {:?}",
            stream.header("content-type")
        );

        let frames = decode_frames(case, &stream.body);
        let order: Vec<&str> = frames.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            delta_runs_collapsed(&order),
            CONVERSE_STREAM_FRAMES.to_vec(),
            "case {case:?}: expected the frame order {CONVERSE_STREAM_FRAMES:?}, got {order:?}"
        );
        // The block is delivered as INCREMENTS, so a client that appends what it
        // reads is exercised: a block longer than one increment arrives as
        // several deltas, and one that fits in a single increment as one.
        let delta_count = order
            .iter()
            .filter(|frame| **frame == "contentBlockDelta")
            .count();
        let longer_than_one_increment = usize::try_from(state::count_text_tokens(&expected_text))
            .unwrap_or(usize::MAX)
            > state::stream_framing::TOKENS_PER_DELTA;
        assert_eq!(
            delta_count > 1,
            longer_than_one_increment,
            "case {case:?}: a block of {} token(s) must arrive as {} delta(s), got \
             {delta_count} in {order:?}",
            state::count_text_tokens(&expected_text),
            if longer_than_one_increment {
                "several"
            } else {
                "one"
            }
        );

        let by_name = |name: &str| {
            frames
                .iter()
                .find(|(frame, _)| frame == name)
                .map(|(_, payload)| payload.clone())
                .unwrap_or_else(|| panic!("case {case:?}: no {name} frame in {order:?}"))
        };

        assert_eq!(
            by_name("messageStart")["role"].as_str(),
            Some("assistant"),
            "case {case:?}: messageStart must carry the assistant role, got {}",
            by_name("messageStart")
        );

        let streamed: String = frames
            .iter()
            .filter(|(frame, _)| frame == "contentBlockDelta")
            .map(|(_, payload)| {
                payload["delta"]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            streamed, expected_text,
            "case {case:?}: the deltas must concatenate to the text Converse answers, \
             got {streamed:?}"
        );
        // Every delta of the block carries that block's own index, which is what
        // tells a client which block it is appending to.
        for (_, payload) in frames
            .iter()
            .filter(|(frame, _)| frame == "contentBlockDelta")
        {
            assert_eq!(
                payload["contentBlockIndex"].as_i64(),
                Some(0),
                "case {case:?}: every delta is for content block 0, got {payload}"
            );
        }
        assert_eq!(
            by_name("contentBlockStop")["contentBlockIndex"].as_i64(),
            Some(0),
            "case {case:?}: contentBlockStop must close block 0, got {}",
            by_name("contentBlockStop")
        );

        assert_eq!(
            by_name("messageStop")["stopReason"].as_str(),
            Some(expected_stop.as_str()),
            "case {case:?}: expected stopReason {expected_stop:?}, got {}",
            by_name("messageStop")
        );
        // The cap is a ceiling: only a cap too small for the shortest
        // completion cuts the turn off, and the `messageStop` frame reports that
        // the same way the unary call does.
        let shortest = i64::from(state::count_text_tokens(state::EMULATED_COMPLETION_FLOOR));
        let expected_for_cap = match row.max_tokens {
            Some(cap) if cap < shortest => "max_tokens",
            _ => "end_turn",
        };
        assert_eq!(
            expected_stop, expected_for_cap,
            "case {case:?}: expected stopReason {expected_for_cap:?} for maxTokens={:?} \
             against a {shortest}-token shortest completion, got {expected_stop:?}",
            row.max_tokens
        );

        let metadata = by_name("metadata");
        assert_eq!(
            metadata["usage"], expected_usage,
            "case {case:?}: expected the usage Converse reports ({expected_usage}), got {}",
            metadata["usage"]
        );
        let latency = metadata["metrics"]["latencyMs"].as_i64();
        assert!(
            latency.is_some_and(|millis| millis > 0),
            "case {case:?}: metadata must report the latency measured, got {metadata}"
        );
    }
}

/// RULE: a `ConverseStream` request that breaks a rule the service states is
/// refused with the modelled error for that rule, in the restJson1 JSON envelope —
/// never with an event stream and never with a 5xx.
#[tokio::test]
async fn converse_stream_refuses_a_bad_request_with_the_modelled_error() {
    let router = emulator();
    let claude = "anthropic.claude-3-haiku-20240307-v1:0";

    // (case, modelId, body, status, error code)
    let rows = vec![
        (
            "no messages at all",
            claude,
            json!({}),
            400,
            "ValidationException",
        ),
        (
            "an empty conversation",
            claude,
            json!({ "messages": [] }),
            400,
            "ValidationException",
        ),
        (
            "a model the Converse API does not serve",
            "cohere.command-text-v14",
            json!({ "messages": [{"role": "user", "content": [{"text": "hi"}]}] }),
            400,
            "ValidationException",
        ),
        (
            "an ARN that resolves to nothing",
            "arn%3Aaws%3Abedrock%3Aus-east-1%3A000000000000%3Afoundation-model%2Fnobody.nope-v1",
            json!({ "messages": [{"role": "user", "content": [{"text": "hi"}]}] }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a bare id naming no model",
            "nobody.such-model-v1",
            json!({ "messages": [{"role": "user", "content": [{"text": "hi"}]}] }),
            400,
            "ValidationException",
        ),
        (
            "a value that is not a model identifier",
            "not-a-model",
            json!({ "messages": [{"role": "user", "content": [{"text": "hi"}]}] }),
            400,
            "ValidationException",
        ),
    ];

    for (case, model_id, body, status, code) in rows {
        let call = post_json(&router, &format!("/model/{model_id}/converse-stream"), &body).await;
        assert_eq!(
            call.status,
            status,
            "case {case:?}: expected HTTP {status}, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.error_code(),
            code,
            "case {case:?}: expected {code}, got {:?}",
            call.error_code()
        );
        assert_eq!(
            call.media_type(),
            "application/json",
            "case {case:?}: a restJson1 error is application/json, got {:?}",
            call.header("content-type")
        );
    }
}

// ---------------------------------------------------------------------------
// InvokeModelWithResponseStream
// ---------------------------------------------------------------------------

/// RULE: `InvokeModelWithResponseStream` serves the same inference `InvokeModel`
/// serves, but delivers it as a SEQUENCE of `ResponseStream` `chunk` events, each
/// carrying one incremental event of the invoked model family's own streaming
/// protocol — the generated content arriving before a terminal event that reports
/// the stop reason — with the service's `amazon-bedrock-invocationMetrics`
/// accounting on the terminal chunk exactly once, and never the unary
/// `InvokeModel` response body as a chunk payload. The payload's MIME type is
/// reported in the required `X-Amzn-Bedrock-Content-Type` response header.
#[tokio::test]
async fn response_stream_delivers_the_turn_as_the_model_families_streaming_events() {
    let router = emulator();

    for row in INVOKE_ROWS {
        let case = row.case;
        let invoke = send(
            &router,
            "POST",
            &format!("/model/{}/invoke", row.model_id),
            &[("content-type", "application/json")],
            row.body.as_bytes().to_vec(),
        )
        .await;
        assert_eq!(
            invoke.status,
            200,
            "case {case:?}: expected InvokeModel to answer 200, got {} / {}",
            invoke.status,
            String::from_utf8_lossy(&invoke.body)
        );
        let unary: Value = invoke.json();
        let at = |pointer: &str| {
            unary
                .pointer(pointer)
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: InvokeModel answered no {pointer}: {unary}"))
        };
        let expected_text = at(row.text_pointer)
            .as_str()
            .unwrap_or_default()
            .to_string();
        let expected_stop = at(row.stop_pointer).as_str().unwrap_or_default().to_string();

        let stream = send(
            &router,
            "POST",
            &format!("/model/{}/invoke-with-response-stream", row.model_id),
            &[("content-type", "application/json")],
            row.body.as_bytes().to_vec(),
        )
        .await;
        assert_eq!(
            stream.status,
            200,
            "case {case:?}: expected InvokeModelWithResponseStream to answer 200, got {} / {}",
            stream.status,
            String::from_utf8_lossy(&stream.body)
        );
        assert_eq!(
            stream.media_type(),
            EVENT_STREAM,
            "case {case:?}: expected content-type {EVENT_STREAM}, got {:?}",
            stream.header("content-type")
        );
        assert_eq!(
            stream.header("x-amzn-bedrock-content-type"),
            "application/json",
            "case {case:?}: the modelled required header must report the payload MIME \
             type, got {:?}",
            stream.header("x-amzn-bedrock-content-type")
        );
        assert_eq!(
            stream.header("x-amzn-bedrock-performanceconfig-latency"),
            "standard",
            "case {case:?}: performanceConfigLatency defaults to standard, got {:?}",
            stream.header("x-amzn-bedrock-performanceconfig-latency")
        );

        let chunks = chunk_payloads(case, &stream.body);
        assert!(
            chunks.len() >= 2,
            "case {case:?}: an incremental stream carries the content and a terminal event \
             in separate chunks, got {chunks:?}"
        );

        // The defect this rule replaces: the unary response body, delivered as a
        // chunk payload, is never an event of a model's streaming protocol.
        for (index, chunk) in chunks.iter().enumerate() {
            assert_ne!(
                chunk, &unary,
                "case {case:?}: chunk {index} carries the unary InvokeModel body instead of a \
                 streaming event"
            );
        }

        // Where the family's streaming protocol names its events, the stream
        // carries exactly those, in order.
        if !row.events.is_empty() {
            let names: Vec<String> = chunks
                .iter()
                .map(|chunk| {
                    event_name(chunk).unwrap_or_else(|| {
                        panic!("case {case:?}: a chunk named no event: {chunk}")
                    })
                })
                .collect();
            assert_eq!(
                names,
                row.events.to_vec(),
                "case {case:?}: expected the event sequence {:?}, got {names:?}",
                row.events
            );
        }

        // The generated content arrives before the terminal event, and the stop
        // reason closes the turn once — the same text and stop reason the unary
        // operation reports for this request.
        let terminal = chunks.len() - 1;
        let content_chunk = chunks
            .iter()
            .position(|chunk| mentions(chunk, &expected_text))
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: no chunk carries the text InvokeModel answers \
                     ({expected_text:?}): {chunks:?}"
                )
            });
        assert!(
            content_chunk < terminal,
            "case {case:?}: the generated content must arrive before the terminal chunk, \
             got it at {content_chunk} of {terminal}"
        );
        let closing: Vec<usize> = chunks
            .iter()
            .enumerate()
            .filter(|(_, chunk)| mentions(chunk, &expected_stop))
            .map(|(index, _)| index)
            .collect();
        assert_eq!(
            closing.len(),
            1,
            "case {case:?}: the stop reason {expected_stop:?} closes the stream exactly once, \
             got it on chunks {closing:?}"
        );
        assert!(
            closing[0] > 0,
            "case {case:?}: the stream must not open with the turn already stopped, got the \
             stop reason on chunk {}",
            closing[0]
        );

        // The service's own accounting: once, on the terminal chunk, reporting
        // the counts the unary operation reports for the same request.
        let carrying: Vec<usize> = chunks
            .iter()
            .enumerate()
            .filter(|(_, chunk)| chunk.get(INVOCATION_METRICS).is_some())
            .map(|(index, _)| index)
            .collect();
        assert_eq!(
            carrying,
            vec![terminal],
            "case {case:?}: {INVOCATION_METRICS} belongs on the terminal chunk exactly once, \
             got it on chunks {carrying:?}"
        );
        let metrics = chunks[terminal][INVOCATION_METRICS].clone();
        for (field, pointer) in [
            ("inputTokenCount", row.input_tokens_pointer),
            ("outputTokenCount", row.output_tokens_pointer),
        ] {
            let counted = metrics[field].as_u64();
            assert!(
                counted.is_some_and(|count| count > 0),
                "case {case:?}: {field} must report the tokens counted, got {metrics}"
            );
            // Only a family whose own envelope reports the count can be checked
            // against it; Mistral's and Cohere's report none.
            if !pointer.is_empty() {
                assert_eq!(
                    counted,
                    at(pointer).as_u64(),
                    "case {case:?}: {field} must match the {pointer} InvokeModel reports ({}), \
                     got {metrics}",
                    at(pointer)
                );
            }
        }
        for field in ["invocationLatency", "firstByteLatency"] {
            assert!(
                metrics[field].as_i64().is_some_and(|millis| millis > 0),
                "case {case:?}: {field} must report the latency measured, got {metrics}"
            );
        }
        assert!(
            metrics["firstByteLatency"].as_i64() <= metrics["invocationLatency"].as_i64(),
            "case {case:?}: the first byte cannot be later than the whole invocation, got \
             {metrics}"
        );
    }
}

/// RULE: for a model whose body is the Converse-shaped Amazon Nova one, the chunk
/// sequence is that message protocol's own — it opens with `messageStart`, closes
/// each content block with `contentBlockStop`, and ends the turn with exactly one
/// `messageStop`, after which no `contentBlockDelta` follows.
#[tokio::test]
async fn a_nova_response_stream_opens_with_message_start_and_closes_with_message_stop() {
    let router = emulator();

    // (case, modelId, body) — every Nova text model the registry serves, over the
    // request shapes the body schema allows: with and without a system prompt,
    // with and without a generation cap, and a non-ascii prompt.
    let rows = vec![
        (
            "nova micro, bare conversation, through the inference profile that contains it",
            "us.amazon.nova-micro-v1:0",
            json!({"messages": [{"role": "user", "content": [{"text": "Reply with one word."}]}]}),
        ),
        (
            "nova lite, with a system prompt and a cap",
            "amazon.nova-lite-v1:0",
            json!({
                "system": [{"text": "You are terse."}],
                "messages": [{"role": "user", "content": [{"text": "Name two planets."}]}],
                "inferenceConfig": {"max_new_tokens": 20},
            }),
        ),
        (
            "nova pro, a cap of one token",
            "amazon.nova-pro-v1:0",
            json!({
                "messages": [{"role": "user", "content": [{"text": "Explain idempotency."}]}],
                "inferenceConfig": {"max_new_tokens": 1},
            }),
        ),
        (
            "nova premier, several turns and non-ascii content",
            "amazon.nova-premier-v1:0",
            json!({"messages": [
                {"role": "user", "content": [{"text": "Übersetze: 木星 ist groß."}]},
                {"role": "assistant", "content": [{"text": "Jupiter is large."}]},
                {"role": "user", "content": [{"text": "Und der Mond?"}]},
            ]}),
        ),
    ];

    for (case, model_id, body) in rows {
        let stream = send(
            &router,
            "POST",
            &format!("/model/{model_id}/invoke-with-response-stream"),
            &[("content-type", "application/json")],
            serde_json::to_vec(&body).expect("a serializable test body"),
        )
        .await;
        assert_eq!(
            stream.status,
            200,
            "case {case:?}: expected 200, got {} / {}",
            stream.status,
            String::from_utf8_lossy(&stream.body)
        );

        let chunks = chunk_payloads(case, &stream.body);
        let events: Vec<String> = chunks
            .iter()
            .map(|chunk| {
                event_name(chunk)
                    .unwrap_or_else(|| panic!("case {case:?}: a chunk named no event: {chunk}"))
            })
            .collect();

        assert_eq!(
            events.first().map(String::as_str),
            Some("messageStart"),
            "case {case:?}: the stream must open with messageStart, got {events:?}"
        );
        let stops = events.iter().filter(|name| *name == "messageStop").count();
        assert_eq!(
            stops, 1,
            "case {case:?}: exactly one messageStop closes the turn, got {events:?}"
        );
        let stop_at = events
            .iter()
            .position(|name| name == "messageStop")
            .expect("a messageStop, just counted");
        assert!(
            !events[stop_at..].iter().any(|name| name == "contentBlockDelta"),
            "case {case:?}: no contentBlockDelta may follow messageStop, got {events:?}"
        );
        let last_block = events
            .iter()
            .rposition(|name| name.starts_with("contentBlock"))
            .unwrap_or_else(|| panic!("case {case:?}: no content-block event at all: {events:?}"));
        assert_eq!(
            events[last_block], "contentBlockStop",
            "case {case:?}: the last content-block event closes the block, got {events:?}"
        );
        assert!(
            last_block < stop_at,
            "case {case:?}: the content blocks close before the message stops, got {events:?}"
        );
    }
}

/// RULE: an `InvokeModelWithResponseStream` request that breaks a rule the service
/// states is refused with the modelled error for that rule, in the restJson1 JSON
/// envelope — the same refusals `InvokeModel` makes, since the two take the same
/// request.
#[tokio::test]
async fn response_stream_refuses_a_bad_request_with_the_modelled_error() {
    let router = emulator();
    let claude = "anthropic.claude-3-haiku-20240307-v1:0";
    let prompt = r#"{"anthropic_version":"bedrock-2023-05-31","messages":[]}"#;

    // (case, modelId, headers, body, status, error code)
    let rows = vec![
        (
            "an empty body",
            claude,
            vec![("content-type", "application/json")],
            "",
            400,
            "ValidationException",
        ),
        (
            "a body that is not JSON-typed",
            claude,
            vec![("content-type", "text/plain")],
            prompt,
            400,
            "ValidationException",
        ),
        (
            "an accept the model cannot answer in",
            claude,
            vec![
                ("content-type", "application/json"),
                ("x-amzn-bedrock-accept", "application/xml"),
            ],
            prompt,
            400,
            "ValidationException",
        ),
        (
            "a guardrail identifier with no version",
            claude,
            vec![
                ("content-type", "application/json"),
                ("x-amzn-bedrock-guardrailidentifier", "gr1234567890"),
            ],
            prompt,
            400,
            "ValidationException",
        ),
        (
            "an inline guardrail config with no identifier",
            claude,
            vec![("content-type", "application/json")],
            r#"{"amazon-bedrock-guardrailConfig":{"tagSuffix":"x"},"prompt":"hi"}"#,
            400,
            "ValidationException",
        ),
        (
            "an ARN that resolves to nothing",
            "arn%3Aaws%3Abedrock%3Aus-east-1%3A000000000000%3Afoundation-model%2Fnobody.nope-v1",
            vec![("content-type", "application/json")],
            prompt,
            404,
            "ResourceNotFoundException",
        ),
        (
            "a bare id naming no model",
            "nobody.such-model-v1",
            vec![("content-type", "application/json")],
            prompt,
            400,
            "ValidationException",
        ),
        (
            "a value that is not a model identifier",
            "not-a-model",
            vec![("content-type", "application/json")],
            prompt,
            400,
            "ValidationException",
        ),
        // A recognised model whose modality this emulator renders no response
        // envelope for: nothing ran, so the streaming surface owes the same
        // modelled model-processing error `InvokeModel` owes, never a chunk
        // frame carrying an invented payload. One row per non-text modality the
        // inventory carries.
        (
            "an embedding model, which produces no stream",
            "amazon.titan-embed-text-v2%3A0",
            vec![("content-type", "application/json")],
            r#"{"inputText":"a cat"}"#,
            424,
            "ModelErrorException",
        ),
        (
            "an image model, which produces no stream",
            "amazon.nova-canvas-v1%3A0",
            vec![("content-type", "application/json")],
            r#"{"inputText":"a cat"}"#,
            424,
            "ModelErrorException",
        ),
        (
            "a video model, which produces no stream",
            "amazon.nova-reel-v1%3A0",
            vec![("content-type", "application/json")],
            r#"{"inputText":"a cat"}"#,
            424,
            "ModelErrorException",
        ),
    ];

    for (case, model_id, headers, body, status, code) in rows {
        let call = send(
            &router,
            "POST",
            &format!("/model/{model_id}/invoke-with-response-stream"),
            &headers,
            body.as_bytes().to_vec(),
        )
        .await;
        assert_eq!(
            call.status,
            status,
            "case {case:?}: expected HTTP {status}, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.error_code(),
            code,
            "case {case:?}: expected {code}, got {:?}",
            call.error_code()
        );
        assert_eq!(
            call.media_type(),
            "application/json",
            "case {case:?}: a restJson1 error is application/json, got {:?}",
            call.header("content-type")
        );
    }
}

// ---------------------------------------------------------------------------
// The model-identifier rule, across every operation that takes a modelId
// ---------------------------------------------------------------------------

/// RULE: a `modelId` that resolves to nothing is answered with the error for the
/// identifier space it is in: an ARN of one of the resource types the member names
/// is `ResourceNotFoundException` (404), which is what that error is documented for
/// ("The specified resource ARN was not found"); anything else — a bare model id,
/// a geography-prefixed profile id, a single word — is not a valid model identifier
/// and is `ValidationException` (400). Every operation that takes a `modelId`
/// follows it.
#[tokio::test]
async fn an_arn_that_resolves_to_nothing_is_not_found_and_anything_else_is_invalid() {
    let router = emulator();

    // (case, modelId, status, error code). The identifier forms come from the
    // `modelId` member's own `@pattern` (smithy_query(query="shape",
    // shape="ConverseStreamRequest")): the resource types an ARN may name, and the
    // bare / geography-prefixed base-model id.
    let rows: Vec<(&str, &str, u16, &str)> = vec![
        (
            "a foundation-model ARN",
            "arn:aws:bedrock:us-east-1:000000000000:foundation-model/nobody.nope-v1",
            404,
            "ResourceNotFoundException",
        ),
        (
            "a provisioned-model ARN",
            "arn:aws:bedrock:us-east-1:000000000000:provisioned-model/abcdefghijkl",
            404,
            "ResourceNotFoundException",
        ),
        (
            "an inference-profile ARN",
            "arn:aws:bedrock:us-east-1:000000000000:inference-profile/eu.nobody.nope-v1",
            404,
            "ResourceNotFoundException",
        ),
        (
            "a custom-model-deployment ARN",
            "arn:aws:bedrock:us-east-1:000000000000:custom-model-deployment/abcdefghijkl",
            404,
            "ResourceNotFoundException",
        ),
        (
            "a SageMaker endpoint ARN",
            "arn:aws:sagemaker:us-east-1:000000000000:endpoint/my-endpoint",
            404,
            "ResourceNotFoundException",
        ),
        (
            "a bare base-model id",
            "nobody.such-model-v1",
            400,
            "ValidationException",
        ),
        (
            "a base-model id with a version suffix",
            "nobody.such-model-v1:7",
            400,
            "ValidationException",
        ),
        (
            "a cross-region inference profile id",
            "eu.nobody.such-model-v1",
            400,
            "ValidationException",
        ),
        ("a single word", "not-a-model", 400, "ValidationException"),
        (
            "a word with an underscore",
            "model_one",
            400,
            "ValidationException",
        ),
        (
            // Every ARN is a reference to a resource, whatever its type: one this
            // emulator holds no record of names an absent resource rather than an
            // identifier the service cannot read.
            "an ARN of a resource type the member does not name",
            "arn:aws:bedrock:us-east-1:000000000000:guardrail/abc123",
            404,
            "ResourceNotFoundException",
        ),
    ];

    for (case, model_id, status, code) in rows {
        let encoded = model_id.replace(':', "%3A").replace('/', "%2F");
        let conversation = json!({ "messages": [{"role": "user", "content": [{"text": "hi"}]}] });
        let calls = vec![
            (
                "Converse",
                post_json(&router, &format!("/model/{encoded}/converse"), &conversation).await,
            ),
            (
                "ConverseStream",
                post_json(
                    &router,
                    &format!("/model/{encoded}/converse-stream"),
                    &conversation,
                )
                .await,
            ),
            (
                "InvokeModel",
                send(
                    &router,
                    "POST",
                    &format!("/model/{encoded}/invoke"),
                    &[("content-type", "application/json")],
                    br#"{"prompt":"hi"}"#.to_vec(),
                )
                .await,
            ),
            (
                "InvokeModelWithResponseStream",
                send(
                    &router,
                    "POST",
                    &format!("/model/{encoded}/invoke-with-response-stream"),
                    &[("content-type", "application/json")],
                    br#"{"prompt":"hi"}"#.to_vec(),
                )
                .await,
            ),
            (
                "CountTokens",
                post_json(
                    &router,
                    &format!("/model/{encoded}/count-tokens"),
                    &json!({ "input": { "converse": conversation } }),
                )
                .await,
            ),
        ];
        for (operation, call) in calls {
            assert_eq!(
                call.status,
                status,
                "case {case:?} on {operation}: expected HTTP {status}, got {} / {}",
                call.status,
                String::from_utf8_lossy(&call.body)
            );
            assert_eq!(
                call.error_code(),
                code,
                "case {case:?} on {operation}: expected {code}, got {:?}",
                call.error_code()
            );
        }
    }
}

/// One spelling a `modelId` may name ONE base model by, and which throughput it
/// therefore asks for.
///
/// The branches come from the `modelId` member's own `@pattern`
/// (smithy_query(query="operation",operation="ConverseStream")): the bare base-model
/// id, a `foundation-model` ARN, an `inference-profile` ARN, and the
/// geography-prefixed cross-region profile id, whose geographies are the ones
/// "Supported Regions and models for cross-region inference" publishes.
struct SpellingRow {
    case: &'static str,
    /// The spelling, with `{model}` standing for the base model id it names.
    template: &'static str,
    /// Whether the spelling names the model THROUGH an inference profile, which is
    /// what decides whether the invocation asks for on-demand throughput.
    through_a_profile: bool,
}

const SPELLING_ROWS: &[SpellingRow] = &[
    SpellingRow {
        case: "the bare base-model id",
        template: "{model}",
        through_a_profile: false,
    },
    SpellingRow {
        case: "a foundation-model ARN without an account",
        template: "arn:aws:bedrock:us-east-1::foundation-model/{model}",
        through_a_profile: false,
    },
    SpellingRow {
        case: "the us cross-region inference profile id",
        template: "us.{model}",
        through_a_profile: true,
    },
    SpellingRow {
        case: "the eu cross-region inference profile id",
        template: "eu.{model}",
        through_a_profile: true,
    },
    SpellingRow {
        case: "the apac cross-region inference profile id",
        template: "apac.{model}",
        through_a_profile: true,
    },
    SpellingRow {
        case: "the us-gov cross-region inference profile id",
        template: "us-gov.{model}",
        through_a_profile: true,
    },
    SpellingRow {
        case: "an inference-profile ARN",
        template: "arn:aws:bedrock:us-east-1:000000000000:inference-profile/us.{model}",
        through_a_profile: true,
    },
];

/// The refusal an on-demand invocation of a model that is not offered with it
/// earns, with `{model}` standing for the `modelId` the request carried.
///
/// Pinned here as text, so the sentence is asserted and not merely compared with
/// the implementation's own rendering (which this suite checks against it below).
const ON_DEMAND_REFUSAL: &str = "Invocation of model ID {model} with on-demand throughput \
     isn’t supported. Retry your request with the ID or ARN of an inference profile that \
     contains this model.";

/// Every surface that INVOKES the model a `modelId` names: the operation, the path
/// under `/model/{modelId}`, and whether a served answer is an event stream.
const INVOCATION_SURFACES: &[(&str, &str, bool)] = &[
    ("Converse", "converse", false),
    ("ConverseStream", "converse-stream", true),
    ("InvokeModel", "invoke", false),
    (
        "InvokeModelWithResponseStream",
        "invoke-with-response-stream",
        true,
    ),
];

/// RULE: an invocation that asks for ON-DEMAND throughput on a model that is not
/// offered with it is refused — `400 ValidationException` whose message names the
/// `modelId` the request carried and the inference profile it has to be invoked
/// through — while the SAME model named through a profile that contains it (a
/// cross-region profile id or an `inference-profile` ARN) is served, and a model
/// that IS offered on demand is served by its bare id. Every surface that invokes a
/// model follows it, and on a streaming surface the refusal is plain JSON rather
/// than an event stream.
///
/// The rows span what the rule generalises over: the model (one offered only
/// through a profile, three offered on demand), the identifier spelling
/// ([`SPELLING_ROWS`]: bare id, foundation-model ARN, each published geography
/// prefix, inference-profile ARN) and the operation ([`INVOCATION_SURFACES`]).
#[tokio::test]
async fn an_invocation_that_asks_for_unoffered_on_demand_throughput_is_refused_on_every_surface() {
    let router = emulator();
    // One body both envelopes accept: the Converse API's `messages`, which is also
    // the Amazon Nova `InvokeModel` request shape, so no row fails on its body
    // instead of on the identifier under test.
    let body = json!({ "messages": [{"role": "user", "content": [{"text": "hi"}]}] });

    // (the model id, whether the inventory offers it for on-demand invocation).
    // The profile-only model is the one this emulator's inventory carries that way;
    // the on-demand models are three the same family publishes, so the outcome
    // turns on the model's own offering and not on its family or its envelope.
    let models: [(&str, bool); 4] = [
        ("amazon.nova-micro-v1:0", false),
        ("amazon.nova-lite-v1:0", true),
        ("amazon.nova-pro-v1:0", true),
        ("amazon.nova-2-lite-v1:0", true),
    ];

    for (model, offers_on_demand) in models {
        for spelling in SPELLING_ROWS {
            let model_id = spelling.template.replace("{model}", model);
            let encoded = model_id.replace(':', "%3A").replace('/', "%2F");
            let served = offers_on_demand || spelling.through_a_profile;
            for (operation, path, streamed) in INVOCATION_SURFACES {
                let case = format!("{model} as {} on {operation}", spelling.case);
                let call = post_json(&router, &format!("/model/{encoded}/{path}"), &body).await;
                if served {
                    assert_eq!(
                        call.status,
                        200,
                        "case {case:?}: expected the invocation to be served, got {} / {}",
                        call.status,
                        String::from_utf8_lossy(&call.body)
                    );
                    let expected_media = if *streamed {
                        EVENT_STREAM
                    } else {
                        "application/json"
                    };
                    assert_eq!(
                        call.media_type(),
                        expected_media,
                        "case {case:?}: expected {expected_media}, got {:?}",
                        call.media_type()
                    );
                    continue;
                }
                assert_eq!(
                    call.status,
                    400,
                    "case {case:?}: expected the on-demand refusal, got {} / {}",
                    call.status,
                    String::from_utf8_lossy(&call.body)
                );
                assert!(
                    call.error_code().starts_with("ValidationException"),
                    "case {case:?}: expected ValidationException, got {:?}",
                    call.error_code()
                );
                // Never framed as an event stream, on the streaming surfaces as
                // much as on the others: a client reads this refusal as JSON.
                assert_eq!(
                    call.media_type(),
                    "application/json",
                    "case {case:?}: expected the refusal as JSON, got {:?} / {}",
                    call.media_type(),
                    String::from_utf8_lossy(&call.body)
                );
                let expected = ON_DEMAND_REFUSAL.replace("{model}", &model_id);
                assert_eq!(
                    call.json()["message"].as_str(),
                    Some(expected.as_str()),
                    "case {case:?}: expected {expected:?}, got {}",
                    call.json()
                );
                // The message the service composes is that same sentence, so the
                // wording lives in one place.
                assert_eq!(
                    state::on_demand_throughput_unsupported_message(&model_id),
                    expected,
                    "case {case:?}: the service's own rendering must be the pinned sentence, \
                     got {:?}",
                    state::on_demand_throughput_unsupported_message(&model_id)
                );
            }
        }
    }
}

/// RULE (the other side of the same one): naming a model WITHOUT invoking it is not
/// an on-demand invocation, so `CountTokens` never answers the on-demand refusal for
/// a profile-only model — it resolves the identifier and then answers its own rule
/// about which models it counts for. The rows come from the inventory itself, which
/// is what records whether a model is offered on demand.
#[tokio::test]
async fn naming_a_model_without_invoking_it_is_not_refused_for_on_demand_throughput() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));

    let state = state::service_state(&*engine).expect("the service state store");
    let profile_only: Vec<String> = {
        let guard = state.lock();
        guard
            .models
            .values()
            .filter(|entry| !entry.offers_on_demand)
            .map(|entry| entry.model_id.clone())
            .collect()
    };
    assert!(
        !profile_only.is_empty(),
        "expected the inventory to carry at least one model offered only through an \
         inference profile, got none"
    );

    for model_id in profile_only {
        let encoded = model_id.replace(':', "%3A");
        let call = post_json(
            &router,
            &format!("/model/{encoded}/count-tokens"),
            &json!({ "input": { "converse": {
                "messages": [{"role": "user", "content": [{"text": "hi"}]}],
            } } }),
        )
        .await;
        let message = call.json()["message"].as_str().unwrap_or_default().to_string();
        assert_ne!(
            message,
            ON_DEMAND_REFUSAL.replace("{model}", &model_id),
            "case {model_id:?}: counting a prompt is no on-demand invocation, so the \
             on-demand refusal must not be answered, got {} / {message:?}",
            call.status
        );
        // Whatever this operation answers is its OWN rule's: either the count, or
        // the refusal it owes for a model whose tokenizer it does not publish.
        assert!(
            call.status == 200
                || message == "The provided model doesn't support counting tokens.",
            "case {model_id:?}: expected the count or CountTokens' own refusal, got {} / {message:?}",
            call.status
        );
    }
}

/// RULE: a model version the Bedrock model lifecycle has retired is still a name
/// the service recognises: every operation that takes a `modelId` answers
/// `ResourceNotFoundException` (404) saying the version has reached the end of its
/// life, rather than serving it or treating the id as unknown. An active version of
/// the same family is served.
#[tokio::test]
async fn a_model_version_past_its_end_of_life_is_not_found_with_the_end_of_life_reason() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));

    // The rows come from the registry itself rather than a hand-written list, so
    // this spans every retired version the service seeds and cannot drift from it.
    use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::state::{
        service_state, ModelLifecycle, END_OF_LIFE_MESSAGE,
    };
    let state = service_state(&*engine).expect("the service state store");
    let (retired, active): (Vec<String>, Vec<String>) = {
        let guard = state.lock();
        (
            guard
                .models
                .values()
                .filter(|model| model.lifecycle == ModelLifecycle::EndOfLife)
                .map(|model| model.model_id.clone())
                .collect(),
            guard
                .models
                .values()
                .filter(|model| model.lifecycle == ModelLifecycle::Active)
                .map(|model| model.model_id.clone())
                .collect(),
        )
    };
    assert!(
        retired.len() >= 2 && !active.is_empty(),
        "the registry must seed both retired and active versions, got {} retired and {} \
         active",
        retired.len(),
        active.len()
    );

    for model_id in &retired {
        let encoded = model_id.replace(':', "%3A");
        let conversation = json!({ "messages": [{"role": "user", "content": [{"text": "hi"}]}] });
        let calls = vec![
            (
                "Converse",
                post_json(&router, &format!("/model/{encoded}/converse"), &conversation).await,
            ),
            (
                "ConverseStream",
                post_json(
                    &router,
                    &format!("/model/{encoded}/converse-stream"),
                    &conversation,
                )
                .await,
            ),
            (
                "InvokeModel",
                send(
                    &router,
                    "POST",
                    &format!("/model/{encoded}/invoke"),
                    &[("content-type", "application/json")],
                    br#"{"prompt":"hi"}"#.to_vec(),
                )
                .await,
            ),
            (
                "InvokeModelWithResponseStream",
                send(
                    &router,
                    "POST",
                    &format!("/model/{encoded}/invoke-with-response-stream"),
                    &[("content-type", "application/json")],
                    br#"{"prompt":"hi"}"#.to_vec(),
                )
                .await,
            ),
            (
                "CountTokens",
                post_json(
                    &router,
                    &format!("/model/{encoded}/count-tokens"),
                    &json!({ "input": { "converse": conversation } }),
                )
                .await,
            ),
        ];
        for (operation, call) in calls {
            assert_eq!(
                call.status,
                404,
                "case {model_id:?} on {operation}: a retired version is not found, got {} / {}",
                call.status,
                String::from_utf8_lossy(&call.body)
            );
            assert_eq!(
                call.error_code(),
                "ResourceNotFoundException",
                "case {model_id:?} on {operation}: expected ResourceNotFoundException, got {:?}",
                call.error_code()
            );
            assert_eq!(
                call.json()["message"].as_str(),
                Some(END_OF_LIFE_MESSAGE),
                "case {model_id:?} on {operation}: the reason must be the end-of-life one, \
                 got {}",
                call.json()
            );
        }
    }

    // The counter-case: an active version is never answered as retired.
    for model_id in &active {
        let encoded = model_id.replace(':', "%3A");
        let call = post_json(
            &router,
            &format!("/model/{encoded}/converse"),
            &json!({ "messages": [{"role": "user", "content": [{"text": "hi"}]}] }),
        )
        .await;
        assert_ne!(
            call.status,
            404,
            "case {model_id:?}: an active version must not be answered as not found, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
    }
}

/// RULE: an invoke body is the prompt and inference parameters in the model
/// family's own JSON request format, so a body that does not parse as a JSON object
/// is refused as a malformed input request, and one that omits a member that
/// family's documented request schema requires is refused naming that member
/// (400 `ValidationException`) — on the streaming invoke path as on the
/// non-streaming one, which is why both read the rule from `crate::state`.
#[tokio::test]
async fn a_body_the_model_cannot_read_is_a_malformed_input_request() {
    let router = emulator();
    use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::state::MALFORMED_BODY_MESSAGE;

    /// The refusal for a body that omits a required member, as the service words
    /// a schema violation.
    fn missing_key(name: &str) -> String {
        format!(
            "Malformed input request: #: required key [{name}] not found, please reformat your \
             input and try again."
        )
    }

    // (case, modelId, body, expected refusal — None when the body is served).
    // Rows differ along the family whose request schema decides, and along the way
    // the body fails it: not JSON at all, a JSON scalar or array rather than an
    // object, another family's prompt member, and each required member of the
    // family's own envelope left out in turn. Required members per family from
    // "Inference request parameters and response fields for foundation models" in
    // the Amazon Bedrock User Guide.
    let rows: Vec<(&str, &str, &str, Option<String>)> = vec![
        (
            "anthropic: its own messages envelope",
            "anthropic.claude-3-haiku-20240307-v1:0",
            r#"{"anthropic_version":"bedrock-2023-05-31","max_tokens":64,"messages":[]}"#,
            None,
        ),
        (
            "anthropic: max_tokens left out",
            "anthropic.claude-3-haiku-20240307-v1:0",
            r#"{"anthropic_version":"bedrock-2023-05-31","messages":[]}"#,
            Some(missing_key("max_tokens")),
        ),
        (
            "anthropic: anthropic_version left out",
            "anthropic.claude-3-haiku-20240307-v1:0",
            r#"{"max_tokens":64,"messages":[]}"#,
            Some(missing_key("anthropic_version")),
        ),
        (
            "anthropic: the legacy text-completions prompt alone",
            "anthropic.claude-3-haiku-20240307-v1:0",
            r#"{"prompt":"\n\nHuman: hi\n\nAssistant:"}"#,
            Some(missing_key("messages")),
        ),
        (
            "anthropic: bytes that are not JSON",
            "anthropic.claude-3-haiku-20240307-v1:0",
            "not json at all",
            Some(MALFORMED_BODY_MESSAGE.to_string()),
        ),
        (
            "anthropic: a JSON array",
            "anthropic.claude-3-haiku-20240307-v1:0",
            r#"[{"role":"user"}]"#,
            Some(MALFORMED_BODY_MESSAGE.to_string()),
        ),
        (
            "anthropic: a JSON string",
            "anthropic.claude-3-haiku-20240307-v1:0",
            r#""hello""#,
            Some(MALFORMED_BODY_MESSAGE.to_string()),
        ),
        (
            "titan: its own envelope",
            "amazon.titan-text-premier-v1:0",
            r#"{"inputText":"hi"}"#,
            None,
        ),
        (
            "titan: another family's prompt member",
            "amazon.titan-text-premier-v1:0",
            r#"{"prompt":"hi"}"#,
            Some(missing_key("inputText")),
        ),
        (
            "nova: its own envelope",
            "amazon.nova-lite-v1:0",
            r#"{"messages":[{"role":"user","content":[{"text":"hi"}]}]}"#,
            None,
        ),
        (
            "nova: another family's prompt member",
            "amazon.nova-lite-v1:0",
            r#"{"inputText":"hi"}"#,
            Some(missing_key("messages")),
        ),
        (
            "llama: its own envelope",
            "meta.llama3-8b-instruct-v1:0",
            r#"{"prompt":"hi"}"#,
            None,
        ),
        (
            "llama: a chat transcript it does not take",
            "meta.llama3-8b-instruct-v1:0",
            r#"{"messages":[]}"#,
            Some(missing_key("prompt")),
        ),
        (
            "mistral: its own prompt envelope",
            "mistral.mixtral-8x7b-instruct-v0:1",
            r#"{"prompt":"hi"}"#,
            None,
        ),
        (
            "mistral: the chat transcript it also takes",
            "mistral.mixtral-8x7b-instruct-v0:1",
            r#"{"messages":[]}"#,
            None,
        ),
        (
            "cohere command: its own envelope",
            "cohere.command-text-v14",
            r#"{"prompt":"hi"}"#,
            None,
        ),
        (
            "cohere command r: the chat envelope",
            "cohere.command-r-plus-v1:0",
            r#"{"message":"hi"}"#,
            None,
        ),
        (
            "cohere: an empty object",
            "cohere.command-text-v14",
            "{}",
            Some(missing_key("prompt")),
        ),
    ];

    for (case, model_id, body, expected) in rows {
        for path in ["invoke", "invoke-with-response-stream"] {
            let call = send(
                &router,
                "POST",
                &format!("/model/{model_id}/{path}"),
                &[("content-type", "application/json")],
                body.as_bytes().to_vec(),
            )
            .await;
            match &expected {
                None => assert_eq!(
                    call.status,
                    200,
                    "case {case:?} on {path}: the family's own envelope is served, got {} / {}",
                    call.status,
                    String::from_utf8_lossy(&call.body)
                ),
                Some(message) => {
                    assert_eq!(
                        call.status,
                        400,
                        "case {case:?} on {path}: expected HTTP 400, got {} / {}",
                        call.status,
                        String::from_utf8_lossy(&call.body)
                    );
                    assert_eq!(
                        call.error_code(),
                        "ValidationException",
                        "case {case:?} on {path}: expected ValidationException, got {:?}",
                        call.error_code()
                    );
                    assert_eq!(
                        call.json()["message"].as_str(),
                        Some(message.as_str()),
                        "case {case:?} on {path}: expected the refusal {message:?}, got {}",
                        call.json()
                    );
                }
            }
        }
    }
}

/// RULE: a streamed inference is recorded in the service's inference log the way a
/// non-streamed one is, under the API that served it — so the log reports what the
/// service actually served rather than what any one surface returned.
#[tokio::test]
async fn a_streamed_inference_is_recorded_in_the_inference_log() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));

    let claude = "anthropic.claude-3-haiku-20240307-v1:0";
    let conversation = json!({ "messages": [{"role": "user", "content": [{"text": "hi"}]}] });
    post_json(&router, &format!("/model/{claude}/converse-stream"), &conversation).await;
    send(
        &router,
        "POST",
        &format!("/model/{claude}/invoke-with-response-stream"),
        &[("content-type", "application/json")],
        // The Anthropic Messages API's own required members, so the body is one
        // the model's documented request schema accepts.
        br#"{"anthropic_version":"bedrock-2023-05-31","max_tokens":64,"messages":[{"role":"user","content":[{"type":"text","text":"hi"}]}]}"#.to_vec(),
    )
    .await;

    let state = service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::state::service_state(&*engine)
        .expect("the service state store");
    let logged: Vec<_> = state.lock().invocations.iter().cloned().collect();
    let kinds: Vec<_> = logged.iter().map(|entry| entry.kind).collect();
    use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::state::InvokeKind;
    assert_eq!(
        kinds,
        vec![InvokeKind::ConverseStream, InvokeKind::SyncStream],
        "the two streamed calls must be logged under the API that served each, got {kinds:?}"
    );
    for entry in &logged {
        assert_eq!(
            entry.model_id, claude,
            "every entry names the model served, got {entry:?}"
        );
        assert!(
            entry.input_tokens > 0 && entry.output_tokens > 0,
            "every entry carries the counts actually served, got {entry:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The project an invocation runs under, across every surface that invokes a model
// ---------------------------------------------------------------------------

/// One surface that invokes a model, and where it carries `projectIdentifier`.
///
/// The Converse surfaces bind the member in the request BODY and the invoke
/// surfaces bind it to the `X-Amzn-Bedrock-ProjectIdentifier` header
/// (`smithy_query(query="operation",operation="Converse,ConverseStream,InvokeModel,\
/// InvokeModelWithResponseStream")`), so the same member reaches the service two
/// ways and the rule has to hold on both.
struct ProjectSurface {
    operation: &'static str,
    /// The path under `/model/{modelId}`.
    path: &'static str,
    /// Whether the member travels in the body; otherwise it is the header above.
    in_body: bool,
    /// Whether a served answer is an event stream.
    streamed: bool,
}

/// The header the invoke surfaces bind `projectIdentifier` to.
const PROJECT_HEADER: &str = "x-amzn-bedrock-projectidentifier";

/// Every surface that invokes a model, with the binding each gives the member.
const PROJECT_SURFACES: &[ProjectSurface] = &[
    ProjectSurface {
        operation: "Converse",
        path: "converse",
        in_body: true,
        streamed: false,
    },
    ProjectSurface {
        operation: "ConverseStream",
        path: "converse-stream",
        in_body: true,
        streamed: true,
    },
    ProjectSurface {
        operation: "InvokeModel",
        path: "invoke",
        in_body: false,
        streamed: false,
    },
    ProjectSurface {
        operation: "InvokeModelWithResponseStream",
        path: "invoke-with-response-stream",
        in_body: false,
        streamed: true,
    },
];

/// One value of `projectIdentifier`, and what the surface owes for it.
struct ProjectRow {
    case: &'static str,
    /// The value the request carries; `None` omits the member altogether.
    value: Option<String>,
    /// The requirement the value breaks, as the per-member refusal states it, or
    /// `None` when the value is one the member admits and the call is served.
    breaks: Option<&'static str>,
    /// Whether the value can be carried by a surface that binds the member to a
    /// HEADER: an empty header field value is indistinguishable from an absent
    /// one, so the member's minimum length is exercised where it is a body member.
    on_a_header: bool,
}

/// The published bounds of `ProjectIdentifier`: `@length min 1 max 2048`
/// (`smithy_query(query="shape",shape="ProjectIdentifier")`).
const PROJECT_IDENTIFIER_MIN_LENGTH: usize = 1;
const PROJECT_IDENTIFIER_MAX_LENGTH: usize = 2048;

/// The table: the forms `ProjectIdentifier`'s own `@pattern` admits — the default
/// project named by omitting the member and by naming it, a bare project id, and a
/// `project/` ARN — and values outside the shape, breaking its pattern and each
/// published length bound. Sources: the member's `@pattern`
/// `^([a-zA-Z0-9_-]+|arn:aws(…):bedrock:[0-9a-z-]{1,20}:[0-9]{12}:project/[a-zA-Z0-9_-]+)$`
/// and its `@length`, plus the member's own documentation for what omitting it and
/// the literal `default` mean
/// (`smithy_query(query="doc",shape="ConverseRequest",member="projectIdentifier")`).
fn project_rows() -> Vec<ProjectRow> {
    vec![
        ProjectRow {
            case: "the member omitted, which names the account's default project",
            value: None,
            breaks: None,
            on_a_header: true,
        },
        ProjectRow {
            case: "the default project named outright",
            value: Some("default".to_string()),
            breaks: None,
            on_a_header: true,
        },
        ProjectRow {
            case: "a project id of the admitted character set",
            value: Some("analytics_team-2".to_string()),
            breaks: None,
            on_a_header: true,
        },
        ProjectRow {
            case: "a project ARN",
            value: Some(format!(
                "arn:aws:bedrock:us-east-1:{}:project/analytics",
                ::engine::emulator_account_id()
            )),
            breaks: None,
            on_a_header: true,
        },
        ProjectRow {
            case: "a project id at the member's minimum length",
            value: Some("a".repeat(PROJECT_IDENTIFIER_MIN_LENGTH)),
            breaks: None,
            on_a_header: true,
        },
        ProjectRow {
            case: "a project id at the member's maximum length",
            value: Some("a".repeat(PROJECT_IDENTIFIER_MAX_LENGTH)),
            breaks: None,
            on_a_header: true,
        },
        ProjectRow {
            case: "a project id one character past the member's maximum length",
            value: Some("a".repeat(PROJECT_IDENTIFIER_MAX_LENGTH + 1)),
            breaks: Some("Member must have length less than or equal to 2048"),
            on_a_header: true,
        },
        ProjectRow {
            case: "a project id one character below the member's minimum length",
            value: Some(String::new()),
            breaks: Some("Member must have length greater than or equal to 1"),
            on_a_header: false,
        },
        ProjectRow {
            case: "a project id carrying characters the member admits nowhere",
            value: Some("my project!".to_string()),
            breaks: Some("Member must satisfy regular expression pattern"),
            on_a_header: true,
        },
        ProjectRow {
            case: "a bare resource path rather than an id or an ARN",
            value: Some("project/analytics".to_string()),
            breaks: Some("Member must satisfy regular expression pattern"),
            on_a_header: true,
        },
        ProjectRow {
            case: "an ARN of another resource of this service",
            value: Some(format!(
                "arn:aws:bedrock:us-east-1:{}:guardrail/abcd1234efgh",
                ::engine::emulator_account_id()
            )),
            breaks: Some("Member must satisfy regular expression pattern"),
            on_a_header: true,
        },
    ]
}

/// RULE: the project an invocation runs under is named by `projectIdentifier`, which
/// every surface that invokes a model takes — in the body on the Converse surfaces
/// and as a header on the invoke surfaces — and the value is checked against that
/// member's OWN constraints: every form its pattern admits, inside its published
/// length, is served exactly as the same request without the member is (the member
/// selects a project, it does not change what the model answers), and a value
/// outside them is refused `400 ValidationException` with the per-member envelope
/// naming `projectIdentifier` and the requirement it broke.
#[tokio::test]
async fn the_project_an_invocation_names_is_taken_on_every_surface_that_invokes_a_model() {
    let router = emulator();
    // A model offered for on-demand invocation whose `InvokeModel` request shape is
    // the Converse API's own `messages` envelope, so one body serves all four
    // surfaces and no case fails on its body instead of on the member under test.
    let model = "amazon.nova-lite-v1:0";
    let conversation = json!({ "messages": [{"role": "user", "content": [{"text": "hi"}]}] });

    for surface in PROJECT_SURFACES {
        let operation = surface.operation;
        let uri = format!("/model/{model}/{}", surface.path);
        for row in project_rows() {
            if !surface.in_body && !row.on_a_header {
                continue;
            }
            let case = format!("{} on {operation}", row.case);
            let mut body = conversation.clone();
            let mut headers = vec![("content-type", "application/json")];
            match (&row.value, surface.in_body) {
                (Some(value), true) => {
                    body["projectIdentifier"] = Value::String(value.clone());
                }
                (Some(value), false) => headers.push((PROJECT_HEADER, value.as_str())),
                (None, _) => {}
            }
            let call = send(
                &router,
                "POST",
                &uri,
                &headers,
                serde_json::to_vec(&body).expect("a serializable test body"),
            )
            .await;

            let Some(requirement) = row.breaks else {
                assert_eq!(
                    call.status,
                    200,
                    "case {case:?}: expected the invocation to be served, got {} / {}",
                    call.status,
                    String::from_utf8_lossy(&call.body)
                );
                let expected_media = match surface.streamed {
                    true => EVENT_STREAM,
                    false => "application/json",
                };
                assert_eq!(
                    call.media_type(),
                    expected_media,
                    "case {case:?}: expected {expected_media}, got {:?}",
                    call.media_type()
                );
                continue;
            };
            assert_eq!(
                call.status,
                400,
                "case {case:?}: expected the member's own refusal, got {} / {}",
                call.status,
                String::from_utf8_lossy(&call.body)
            );
            assert_eq!(
                call.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {:?}",
                call.error_code()
            );
            let message = call.json()["message"].as_str().unwrap_or_default().to_string();
            assert!(
                message.contains("projectIdentifier"),
                "case {case:?}: expected the envelope to name projectIdentifier, got {message:?}"
            );
            assert!(
                message.contains(requirement),
                "case {case:?}: expected the clause {requirement:?}, got {message:?}"
            );
        }
    }
}
