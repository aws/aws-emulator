// SPDX-License-Identifier: CC0-1.0

//! The OpenAI-compatible chat-completions envelope: the `InvokeModel` request and response
//! shape of the providers Amazon Bedrock serves on that schema.
//!
//! Amazon Bedrock's `InvokeModel` body is the PROVIDER's own format, and the providers
//! whose own API is the OpenAI chat-completions API publish that format: a request binds
//! `messages` (or, for the completion form of the same API, a raw `prompt`) with
//! `max_tokens`, `temperature`, `top_p` and `stop`, and a response is a `chat.completion`
//! object carrying one `choices` entry whose `message` holds the assistant's turn, the
//! `finish_reason` the turn ended with and a `usage` block with the prompt, completion and
//! total token counts. The streaming form of the same envelope is a sequence of
//! `chat.completion.chunk` objects whose `choices[].delta` carries the increment and whose
//! last one states the `finish_reason`.
//!
//! Everything about the ENVELOPE lives here, so [`super`]'s per-family readers and writers
//! delegate to one place for it; the text inside it is the one generation stage every
//! surface of this service answers from ([`super::generation`]), which says plainly that no
//! model ran.

/// The `object` a non-streaming answer of this envelope is.
const COMPLETION_OBJECT: &str = "chat.completion";

/// The `object` each event of the streaming form is.
const CHUNK_OBJECT: &str = "chat.completion.chunk";

/// The prefix the id of a chat completion carries.
const COMPLETION_ID_PREFIX: &str = "chatcmpl-";

/// The role the model's own turn is reported under.
const ASSISTANT_ROLE: &str = "assistant";

/// The `finish_reason` a turn that ended on its own reports.
pub const FINISH_REASON_STOP: &str = "stop";

/// The `finish_reason` a turn that ran into the request's generation cap reports.
pub const FINISH_REASON_LENGTH: &str = "length";

/// The members a request of this envelope must bind: the prompt, in either of the two
/// spellings the API publishes for it — the chat form's `messages` and the completion
/// form's `prompt`. `messages` is first, so a body that binds neither is reported against
/// it.
pub const REQUIRED_SLOTS: &[&[&str]] = &[&["messages", "prompt"]];

/// The refusal text a body whose JSON this envelope cannot read is answered with.
///
/// Amazon Bedrock's `InvokeModel` body is the PROVIDER's own format, so the sentence a
/// body that is no JSON object at all is refused in is the provider's validator's wording
/// and not one constant for the whole catalogue: the providers served on this envelope
/// answer it in the words below, where a provider with its own native envelope answers
/// [`super::MALFORMED_BODY_MESSAGE`]. Entirely fixed text — it names no member, value,
/// offset or model id, so nothing of the request reaches it.
pub const MALFORMED_BODY_MESSAGE: &str =
    "The JSON object in the request body is malformed. Fix the syntax errors and retry your \
     request.";

/// The JSON Pointer this envelope binds the generation cap to.
pub const MAX_TOKENS_POINTER: &str = "/max_tokens";

/// The JSON Pointer this envelope binds the stop sequences to.
pub const STOP_SEQUENCES_POINTER: &str = "/stop";

/// The response body one served turn answers with, in this envelope.
///
/// `created` is not taken: the envelope's own `created` is seconds since the epoch, which
/// the caller passes so the whole response is a function of the call being served.
pub fn response_body(
    model_id: &str,
    completion_id: &str,
    text: &str,
    input_tokens: u32,
    output_tokens: u32,
    finish_reason: &str,
    created_seconds: i64,
) -> serde_json::Value {
    serde_json::json!({
        "id": format!("{COMPLETION_ID_PREFIX}{completion_id}"),
        "object": COMPLETION_OBJECT,
        "created": created_seconds,
        "model": model_id,
        "choices": [{
            "index": 0,
            "message": { "role": ASSISTANT_ROLE, "content": text },
            "finish_reason": finish_reason,
        }],
        "usage": usage(input_tokens, output_tokens),
    })
}

/// The chunks the streaming form of this envelope delivers one turn as: the opening chunk
/// that names the role, the content increment, and the terminal chunk that states the
/// finish reason and the turn's usage.
pub fn stream_chunks(
    model_id: &str,
    completion_id: &str,
    text: &str,
    input_tokens: u32,
    output_tokens: u32,
    finish_reason: &str,
    created_seconds: i64,
) -> Vec<serde_json::Value> {
    let chunk = |delta: serde_json::Value, reason: serde_json::Value| {
        serde_json::json!({
            "id": format!("{COMPLETION_ID_PREFIX}{completion_id}"),
            "object": CHUNK_OBJECT,
            "created": created_seconds,
            "model": model_id,
            "choices": [{ "index": 0, "delta": delta, "finish_reason": reason }],
        })
    };
    let mut terminal = chunk(
        serde_json::json!({}),
        serde_json::Value::String(finish_reason.to_string()),
    );
    if let Some(members) = terminal.as_object_mut() {
        members.insert("usage".to_string(), usage(input_tokens, output_tokens));
    }
    vec![
        chunk(
            serde_json::json!({ "role": ASSISTANT_ROLE, "content": "" }),
            serde_json::Value::Null,
        ),
        chunk(
            serde_json::json!({ "content": text }),
            serde_json::Value::Null,
        ),
        terminal,
    ]
}

/// The token accounting this envelope reports: the prompt's tokens, the completion's, and
/// their total.
fn usage(input_tokens: u32, output_tokens: u32) -> serde_json::Value {
    serde_json::json!({
        "prompt_tokens": input_tokens,
        "completion_tokens": output_tokens,
        "total_tokens": input_tokens.saturating_add(output_tokens),
    })
}
