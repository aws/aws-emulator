// SPDX-License-Identifier: CC0-1.0

//! The refusal `InvokeModel` answers a body that is NOT a readable request of the invoked
//! model family, end to end over HTTP.
//!
//! The rule under test: `InvokeModel$body` is "the prompt and inference parameters in the
//! format specified in the `contentType` ... You must provide the body in JSON format"
//! (`smithy_query(query="doc",shape="InvokeModelRequest",member="body")`) — the PROVIDER's
//! own format — so a body the family cannot read AS ITS REQUEST OBJECT (bytes whose JSON
//! does not parse, or a JSON document that parses to anything other than an object) is
//! refused 400 `ValidationException` by that provider's request validator, in the one
//! sentence that provider states the class in, naming no member and echoing nothing the
//! request carried. A provider that publishes its own native envelope answers "Malformed
//! input request, please reformat your input and try again."; a provider Amazon Bedrock
//! serves on the OpenAI-compatible chat-completions envelope answers "The JSON object in
//! the request body is malformed. Fix the syntax errors and retry your request." A body
//! that IS the family's request object but omits a member it requires is a different,
//! member-naming refusal, pinned by [`crate::bedrock_runtime_inference`].
//!
//! The rows differ along what the rule generalises over: the FORM in which the document
//! fails (each way JSON syntax can break, and each JSON type that is not an object), the
//! SIZE of the body (one byte, far longer than any prompt), the bytes being non-UTF-8, and
//! the model family whose provider decides the wording — one model per `InvokeModel`
//! request envelope, and several providers of the shared envelope, so the sentence follows
//! the family and not the model id. A body carrying NO bytes at all is a different class
//! (the required `body` member absent rather than unreadable) and keeps its own refusal.

use crate::bedrock_runtime_inference::{emulator, send};

/// The sentence a provider with its own native request envelope refuses this class in,
/// pinned literally rather than read from the constant the handler itself carries.
const MALFORMED: &str = "Malformed input request, please reformat your input and try again.";

/// The sentence a provider served on the OpenAI-compatible chat-completions envelope
/// refuses this class in.
const MALFORMED_JSON_OBJECT: &str =
    "The JSON object in the request body is malformed. Fix the syntax errors and retry your \
     request.";

/// One model per `InvokeModel` request family, so the refusal is asserted for every request
/// schema the operation dispatches on, and four providers of the shared OpenAI-compatible
/// envelope, so the wording is asserted to follow the family rather than one model id.
/// Model ids from "Amazon Bedrock base model IDs (on-demand throughput)" in the Amazon
/// Bedrock User Guide.
const MODELS: [(&str, &str, &str); 10] = [
    ("anthropic messages", "anthropic.claude-3-haiku-20240307-v1:0", MALFORMED),
    ("titan text", "amazon.titan-text-premier-v1:0", MALFORMED),
    ("nova", "amazon.nova-lite-v1:0", MALFORMED),
    ("llama", "meta.llama3-8b-instruct-v1:0", MALFORMED),
    ("mistral", "mistral.mistral-7b-instruct-v0:2", MALFORMED),
    ("cohere command", "cohere.command-text-v14", MALFORMED),
    ("openai-compatible: openai", "openai.gpt-oss-20b-1:0", MALFORMED_JSON_OBJECT),
    ("openai-compatible: deepseek", "deepseek.r1-v1:0", MALFORMED_JSON_OBJECT),
    ("openai-compatible: qwen", "qwen.qwen3-32b-v1:0", MALFORMED_JSON_OBJECT),
    ("openai-compatible: moonshotai", "moonshotai.kimi-k2.5", MALFORMED_JSON_OBJECT),
];

/// Every body form the rule covers: the ways a JSON document fails to parse, and the JSON
/// types that parse but are not the family's request object.
fn unreadable_bodies() -> Vec<(String, Vec<u8>)> {
    let mut rows: Vec<(String, Vec<u8>)> = vec![
        ("free text".to_string(), b"this is not json at all {{{".to_vec()),
        (
            "a truncated object".to_string(),
            br#"{"messages": [{"role": "user", "content": [{maxTokenCount: 20,,,}"#.to_vec(),
        ),
        ("an unquoted key".to_string(), br#"{prompt: "hi"}"#.to_vec()),
        ("a trailing comma".to_string(), br#"{"prompt": "hi",}"#.to_vec()),
        (
            "single-quoted strings".to_string(),
            b"{'prompt': 'hi'}".to_vec(),
        ),
        ("an unclosed array".to_string(), br#"{"messages": ["#.to_vec()),
        ("one stray byte".to_string(), b"{".to_vec()),
        // Bytes that are not UTF-8 at all, so the document cannot even be decoded.
        ("non-UTF-8 bytes".to_string(), vec![0xff, 0xfe, 0x00, 0x01]),
        // A document that parses, as each JSON type that is not an object.
        ("a JSON array".to_string(), br#"[{"role":"user"}]"#.to_vec()),
        ("a JSON string".to_string(), br#""hello""#.to_vec()),
        ("a JSON number".to_string(), b"42".to_vec()),
        ("a JSON boolean".to_string(), b"true".to_vec()),
        ("JSON null".to_string(), b"null".to_vec()),
    ];
    // A body far longer than any prompt, broken at its very end: size is not what decides.
    let mut overlong = br#"{"prompt": ""#.to_vec();
    overlong.extend(std::iter::repeat_n(b'x', 20_000));
    rows.push(("an overlong unterminated string".to_string(), overlong));
    rows
}

/// Every unreadable body, on every family, is refused 400 `ValidationException` with the
/// sentence that family's provider states the class in.
#[tokio::test]
async fn a_body_that_is_no_request_of_the_family_is_one_malformed_input_refusal() {
    let router = emulator();

    for (family, model_id, expected) in MODELS {
        for (form, body) in unreadable_bodies() {
            let case = format!("{family} / {form}");
            let path = format!("/model/{}/invoke", model_id.replace(':', "%3A"));
            let call = send(
                &router,
                "POST",
                &path,
                &[("content-type", "application/json")],
                body,
            )
            .await;
            assert_eq!(
                call.status, 400,
                "case {case:?}: expected HTTP 400, got {} / {}",
                call.status,
                String::from_utf8_lossy(&call.body)
            );
            assert_eq!(
                call.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {:?}",
                call.error_code()
            );
            assert_eq!(
                call.json()["message"].as_str(),
                Some(expected),
                "case {case:?}: expected message {expected:?}, got {:?}",
                call.json()["message"]
            );
        }
    }
}

/// The refusal is the DOCUMENT's, so it carries nothing the request named: neither the
/// member names a readable body would have been checked against nor the model id.
#[tokio::test]
async fn the_malformed_input_refusal_echoes_nothing_the_request_carried() {
    let router = emulator();

    for (family, model_id, expected) in MODELS {
        let path = format!("/model/{}/invoke", model_id.replace(':', "%3A"));
        let call = send(
            &router,
            "POST",
            &path,
            &[("content-type", "application/json")],
            br#"{"messages": [{"role": "user""#.to_vec(),
        )
        .await;
        let message = call.json()["message"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            message, expected,
            "case {family:?}: expected message {expected:?}, got {message:?}"
        );
        for echoed in ["messages", "role", "user", model_id] {
            assert!(
                !message.contains(echoed),
                "case {family:?}: the refusal must name nothing the request carried, but it \
                 names {echoed:?}: {message:?}"
            );
        }
    }
}
