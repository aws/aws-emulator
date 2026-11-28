// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for `InvokeGlobalInferenceGatewayWithResponseStream`, the
//! streaming half of the Global Inference Gateway data plane.
//!
//! The rules asserted here:
//!
//! * the streamed answer is an `application/vnd.amazon.eventstream` body: the
//!   `initial-response` frame a client reads the output shape out of, then one
//!   `PayloadPart` frame per `chat.completion.chunk` of the turn;
//! * the stream delivers the SAME turn the buffered route returns — concatenating the
//!   chunks' `delta.content` reconstructs exactly the text the buffered document carries,
//!   and the usage the terminal chunk states is the usage that document states;
//! * both routes resolve a request through the SAME registry lookup, body read and
//!   content-type gate, so each refusal is answered with one status, one error shape and
//!   one message whichever route is addressed;
//! * a streamed invocation is recorded as streamed, so the log tells the two routes apart.
//!
//! The frames are read with the maintained event-stream decoder, exactly as a client's own
//! decoder reads them.

use aws_smithy_eventstream::frame::read_message_from;
use aws_smithy_types::event_stream::HeaderValue;
use serde_json::{json, Value};
use service_com_amazonaws_sagemakergatewayruntime_globalinferencegatewayruntime as gateway_runtime;

use gateway_runtime::state::completion::NO_MODEL_NOTICE;

use crate::gateway_invocation::{
    chat_body, completion_text, first_published, invocations, invoke, post, response_stream, Call,
    Emulator, JSON,
};

/// The media type an event-stream body carries (source:
/// `service_common::event_stream::EVENT_STREAM_CONTENT_TYPE`).
const EVENT_STREAM: &str = "application/vnd.amazon.eventstream";

/// The `:event-type` of the frame an event-stream response opens with (source:
/// `service_common::event_stream::INITIAL_RESPONSE_EVENT`).
const INITIAL_RESPONSE: &str = "initial-response";

/// The `ResponseStream` member one completion chunk is framed as (source:
/// `smithy_query(query="shape", shape="ResponseStream")`).
const PAYLOAD_PART: &str = "PayloadPart";

/// One streamed invocation of `gateway_id` with `body`.
async fn invoke_stream(emulator: &Emulator, gateway_id: &str, body: &[u8]) -> Call {
    post(
        emulator,
        &response_stream(gateway_id),
        &[("content-type", JSON), ("accept", JSON)],
        body,
    )
    .await
}

/// One decoded frame of a response: its `:event-type`, its `:content-type`, and its payload.
#[derive(Debug)]
struct Frame {
    event_type: String,
    content_type: Option<String>,
    payload: Vec<u8>,
}

impl Frame {
    /// The frame's payload as the JSON document it announces.
    fn document(&self) -> Value {
        serde_json::from_slice(&self.payload).unwrap_or(Value::Null)
    }
}

/// Every frame of `body`, in arrival order, read with the maintained decoder.
fn frames(case: &str, body: &bytes::Bytes) -> Vec<Frame> {
    let mut buffer = body.clone();
    let mut read = Vec::new();
    while !buffer.is_empty() {
        let message = read_message_from(&mut buffer)
            .unwrap_or_else(|error| panic!("case {case:?}: the body is a frame stream: {error}"));
        let header = |name: &str| {
            message
                .headers()
                .iter()
                .find(|header| header.name().as_str() == name)
                .and_then(|header| match header.value() {
                    HeaderValue::String(value) => Some(value.as_str().to_string()),
                    _ => None,
                })
        };
        read.push(Frame {
            event_type: header(":event-type").unwrap_or_default(),
            content_type: header(":content-type"),
            payload: message.payload().to_vec(),
        });
    }
    read
}

/// RULE: a streamed invocation answers the turn as an event stream — the initial-response
/// frame, then one `PayloadPart` frame per chunk — and the chunks reconstruct exactly the
/// document the buffered route returns for the same request.
#[tokio::test]
async fn the_streamed_chunks_reconstruct_the_buffered_completion() {
    let emulator = Emulator::new();
    let (gateway_id, model) = first_published();

    // Rows differ along the body the same turn is computed from: the sizes and shapes of
    // request the rule has to hold over.
    let rows: Vec<(&str, Vec<u8>)> = vec![
        (
            "the smallest body that names a model",
            serde_json::to_vec(&json!({ "model": model })).expect("serializes"),
        ),
        ("a chat request", chat_body(model)),
        (
            "a body asking to stream",
            serde_json::to_vec(&json!({ "model": model, "stream": true })).expect("serializes"),
        ),
        (
            "a body one kibibyte of content long",
            serde_json::to_vec(&json!({
                "model": model,
                "messages": [{ "role": "user", "content": "x".repeat(1024) }],
            }))
            .expect("serializes"),
        ),
    ];

    for (case, body) in rows {
        let streamed = invoke_stream(&emulator, gateway_id, &body).await;
        assert_eq!(
            streamed.status, 200,
            "case {case:?}: expected HTTP 200, got {} {:?}",
            streamed.status,
            streamed.json()
        );
        assert_eq!(
            streamed.content_type(),
            EVENT_STREAM,
            "case {case:?}: a streamed answer is announced as {EVENT_STREAM}, got {:?}",
            streamed.content_type()
        );

        let read = frames(case, &streamed.body);
        assert!(
            read.len() >= 3,
            "case {case:?}: expected the initial response, at least one chunk and the \
             terminal chunk, got {read:?}"
        );
        assert_eq!(
            read[0].event_type, INITIAL_RESPONSE,
            "case {case:?}: the body opens with the frame a client reads the output shape \
             out of, got {:?}",
            read[0].event_type
        );
        let chunks = &read[1..];
        for frame in chunks {
            assert_eq!(
                frame.event_type, PAYLOAD_PART,
                "case {case:?}: every chunk travels as a {PAYLOAD_PART} event, got {frame:?}"
            );
            assert_eq!(
                frame.content_type.as_deref(),
                Some(JSON),
                "case {case:?}: a chunk announces the media type it is in, got {frame:?}"
            );
            assert_eq!(
                frame.document()["object"].as_str(),
                Some("chat.completion.chunk"),
                "case {case:?}: every chunk is a chat.completion.chunk, got {:?}",
                frame.document()
            );
            assert_eq!(
                frame.document()["model"].as_str(),
                Some(model),
                "case {case:?}: every chunk names the resolved model, got {:?}",
                frame.document()
            );
        }

        let ids: Vec<String> = chunks
            .iter()
            .map(|frame| {
                frame.document()["id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert!(
            ids.windows(2).all(|pair| pair[0] == pair[1]),
            "case {case:?}: every chunk of one turn carries that turn's completion id, \
             got {ids:?}"
        );

        let streamed_text: String = chunks
            .iter()
            .map(|frame| {
                frame.document()["choices"][0]["delta"]["content"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert!(
            streamed_text.starts_with(NO_MODEL_NOTICE),
            "case {case:?}: the streamed completion says no model ran, got {streamed_text:?}"
        );

        let terminal = chunks
            .last()
            .unwrap_or_else(|| panic!("case {case:?}: the stream has a terminal chunk"))
            .document();
        assert_eq!(
            terminal["choices"][0]["finish_reason"].as_str(),
            Some("stop"),
            "case {case:?}: the terminal chunk states the finish reason, got {terminal:?}"
        );

        // The same request, buffered: the two framings deliver one answer.
        let buffered = invoke(&emulator, gateway_id, &body).await;
        assert_eq!(
            buffered.status, 200,
            "case {case:?}: expected the buffered route to serve it too, got {} {:?}",
            buffered.status,
            buffered.json()
        );
        let document = buffered.json();
        assert_eq!(
            streamed_text,
            completion_text(&document),
            "case {case:?}: the chunks reconstruct the buffered text, got {streamed_text:?}"
        );
        assert_eq!(
            terminal["usage"], document["usage"],
            "case {case:?}: both framings report one usage, got {:?} and {:?}",
            terminal["usage"], document["usage"]
        );
        assert_ne!(
            terminal["id"], document["id"],
            "case {case:?}: two calls are two completions, got one id {:?}",
            terminal["id"]
        );
    }
}

/// RULE: the log tells the two routes apart — a streamed invocation is recorded as
/// streamed, with the headers the request carried.
#[tokio::test]
async fn the_log_records_which_route_served_an_invocation() {
    let emulator = Emulator::new();
    let (gateway_id, model) = first_published();
    let body = chat_body(model);
    let token = "eyJhbGciOiJub25lIn0.e30.";

    // (case, path, the headers sent, whether the answer is streamed, whether a customer
    //  identity token was presented, the Accept asked for)
    type Row<'r> = (&'r str, String, Vec<(&'r str, &'r str)>, bool, bool, Option<&'r str>);
    let rows: Vec<Row> = vec![
        (
            "the buffered route, with an Accept and a token",
            invocations(gateway_id),
            vec![
                ("content-type", JSON),
                ("accept", JSON),
                ("x-amzn-sagemaker-customer-identity-token", token),
            ],
            false,
            true,
            Some(JSON),
        ),
        (
            "the buffered route, with neither",
            invocations(gateway_id),
            vec![("content-type", JSON)],
            false,
            false,
            None,
        ),
        (
            "the streaming route, with an Accept and a token",
            response_stream(gateway_id),
            vec![
                ("content-type", JSON),
                ("accept", JSON),
                ("x-amzn-sagemaker-customer-identity-token", token),
            ],
            true,
            true,
            Some(JSON),
        ),
        (
            "the streaming route, with neither",
            response_stream(gateway_id),
            vec![("content-type", JSON)],
            true,
            false,
            None,
        ),
    ];

    for (case, path, headers, streaming, token_present, accept) in rows {
        let answered = post(&emulator, &path, &headers, &body).await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: expected HTTP 200, got {} {:?}",
            answered.status,
            answered.json()
        );
        let record = emulator
            .log()
            .last()
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the served invocation is logged"));
        assert_eq!(
            (
                record.streaming,
                record.identity_token_present,
                record.accept.as_deref(),
                record.model.as_str(),
            ),
            (streaming, token_present, accept, model),
            "case {case:?}: expected the invocation logged as served, got {record:?}"
        );
        assert!(
            !format!("{record:?}").contains(token),
            "case {case:?}: the customer identity token is redacted from the log, got {record:?}"
        );
    }
}

/// RULE: both routes read a request and resolve it against the registry the same way, so
/// one request earns one status, one error shape and one message whichever of them is
/// addressed.
#[tokio::test]
async fn both_routes_refuse_one_request_in_the_same_words() {
    let emulator = Emulator::new();
    let (gateway_id, model) = first_published();
    let drained = "drained-model-v1";
    emulator
        .engine
        .state::<gateway_runtime::ComAmazonawsSagemakergatewayruntimeGlobalinferencegatewayruntimeState>()
        .lock()
        .put_gateway(gateway_runtime::state::Gateway::new(
            gateway_id,
            "us-east-1",
            &[(model, true), (drained, false)],
            std::time::SystemTime::now(),
        ));

    // (case, the gateway addressed, the content type declared, the body, status, shape)
    type Row<'r> = (&'r str, &'r str, &'r str, Vec<u8>, u16, &'r str);
    let rows: Vec<Row> = vec![
        (
            "a content type the gateway does not read",
            gateway_id,
            "text/plain",
            chat_body(model),
            415,
            "UnsupportedMediaType",
        ),
        (
            "a body that is not one JSON object",
            gateway_id,
            JSON,
            b"not json".to_vec(),
            400,
            "ValidationException",
        ),
        (
            "a body repeating the model member with different values",
            gateway_id,
            JSON,
            br#"{"model":"one","model":"two"}"#.to_vec(),
            400,
            "ValidationException",
        ),
        (
            "a gateway id outside the pattern the model declares",
            "NotALowercaseId",
            JSON,
            chat_body(model),
            400,
            "ValidationException",
        ),
        (
            "a gateway id the registry does not hold",
            "nosuchgateway",
            JSON,
            chat_body(model),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a model no target of the gateway serves",
            gateway_id,
            JSON,
            chat_body("no-such-model"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a model whose target has no capacity",
            gateway_id,
            JSON,
            chat_body(drained),
            503,
            "ServiceUnavailableException",
        ),
    ];

    for (case, gateway, content_type, body, status, code) in rows {
        let mut answers = Vec::new();
        for (route, path) in [
            ("InvokeGlobalInferenceGateway", invocations(gateway)),
            (
                "InvokeGlobalInferenceGatewayWithResponseStream",
                response_stream(gateway),
            ),
        ] {
            let answered = post(&emulator, &path, &[("content-type", content_type)], &body).await;
            assert_eq!(
                answered.status, status,
                "case {case:?} on {route}: expected HTTP {status}, got {} {:?}",
                answered.status,
                answered.json()
            );
            assert_eq!(
                answered.error_code(),
                code,
                "case {case:?} on {route}: expected {code}, got {:?}",
                answered.error_code()
            );
            answers.push((route, answered.message()));
        }
        assert_eq!(
            answers[0].1, answers[1].1,
            "case {case:?}: both routes word one refusal the same way, got {answers:?}"
        );
        assert!(
            emulator.log().is_empty(),
            "case {case:?}: a refused request is dispatched by neither route, got {:?}",
            emulator.log()
        );
    }
}
