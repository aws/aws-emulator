// SPDX-License-Identifier: CC0-1.0

//! The answer one served invocation carries: the OpenAI chat-completions envelope, and
//! the text inside it.
//!
//! # NO LANGUAGE MODEL RUNS IN THIS EMULATOR
//!
//! Nothing here pretends otherwise. The completion text is the notice in
//! [`NO_MODEL_NOTICE`], which says in so many words that no model ran, followed by facts
//! about the request that was actually served — the gateway that routed it, the model it
//! resolved to, and how large the body was. It is never the request's own words answered
//! back, never a value read out of the prompt, and never text synthesised to look like
//! inference, so a client reading a completion can always tell that no inference happened.
//!
//! Everything AROUND the text is real and computed: the completion id is drawn from the
//! store's completion sequence, so two calls are two completions; the `created` stamp is
//! the instant the call was served; the token counts are measured over the bytes that
//! arrived and the text that is served, with the byte-pair tokenizer this project already
//! counts tokens with.
//!
//! # One turn, two framings
//!
//! [`ServedTurn`] is computed once and rendered either as the single
//! `chat.completion` document the buffered operation returns ([`ServedTurn::response_body`])
//! or as the `chat.completion.chunk` sequence the streaming operation frames
//! ([`ServedTurn::stream_chunks`]). A client that concatenates the chunks' `delta.content`
//! reconstructs exactly the text the buffered document carries, so the two operations
//! deliver the same answer.

use std::time::{SystemTime, UNIX_EPOCH};

/// The `object` a buffered answer of this envelope is.
const COMPLETION_OBJECT: &str = "chat.completion";

/// The `object` each event of the streaming form is.
const CHUNK_OBJECT: &str = "chat.completion.chunk";

/// The prefix the id of a chat completion carries.
const COMPLETION_ID_PREFIX: &str = "chatcmpl-";

/// The role the model's own turn is reported under.
const ASSISTANT_ROLE: &str = "assistant";

/// The `finish_reason` a turn that ended on its own reports.
const FINISH_REASON_STOP: &str = "stop";

/// The sentence every completion this emulator serves opens with.
pub const NO_MODEL_NOTICE: &str = "No language model runs in this emulator.";

/// One served turn: the completion as this emulator computed it, before it is framed.
#[derive(Debug, Clone)]
pub struct ServedTurn {
    /// The completion id, `chatcmpl-` followed by an opaque id derived from the gateway,
    /// the model and this call's position in the store's completion sequence.
    pub completion_id: String,
    /// The model the gateway resolved the request to, as the answer reports it.
    pub model: String,
    /// The `created` stamp: seconds since the epoch at the instant the call was served.
    pub created_seconds: i64,
    /// The assistant text served.
    pub text: String,
    /// How many tokens of input the request body was.
    pub prompt_tokens: u32,
    /// How many tokens of output the served text is.
    pub completion_tokens: u32,
}

impl ServedTurn {
    /// Compute the turn one invocation is answered with.
    ///
    /// `seq` is the call's position in the store's completion sequence, so the completion
    /// id a call answers with is a function of the state the call was served against and
    /// differs from every other call's. `body` is the request payload: it is COUNTED here
    /// and nowhere stored, because the gateway "reads only `model` and `stream`, and does
    /// not store the body" — what a model server would tokenize is the only thing taken
    /// from it.
    #[must_use]
    pub fn new(gateway_id: &str, model: &str, seq: u64, body: &[u8], served_at: SystemTime) -> Self {
        let text = format!(
            "{NO_MODEL_NOTICE} Gateway {gateway_id} resolved this request to model {model}, \
             which generated no text. The request body carried {} byte(s).",
            body.len()
        );
        Self {
            completion_id: format!(
                "{COMPLETION_ID_PREFIX}{}",
                ::service_common::stable_hex_id(&[gateway_id, model, &seq.to_string()])
            ),
            model: model.to_string(),
            created_seconds: created_seconds(served_at),
            prompt_tokens: count_tokens(&String::from_utf8_lossy(body)),
            completion_tokens: count_tokens(&text),
            text,
        }
    }

    /// This turn as the one `chat.completion` document the buffered operation returns.
    #[must_use]
    pub fn response_body(&self) -> Vec<u8> {
        document(&serde_json::json!({
            "id": self.completion_id,
            "object": COMPLETION_OBJECT,
            "created": self.created_seconds,
            "model": self.model,
            "choices": [{
                "index": 0,
                "message": { "role": ASSISTANT_ROLE, "content": self.text },
                "finish_reason": FINISH_REASON_STOP,
            }],
            "usage": self.usage(),
        }))
    }

    /// This turn as the `chat.completion.chunk` documents the streaming operation frames,
    /// in order: the chunk that names the role, one chunk per increment of the text, and
    /// the terminal chunk that states the finish reason and the turn's usage.
    ///
    /// Concatenating every chunk's `delta.content` yields exactly [`ServedTurn::text`], so
    /// the stream delivers the same answer the buffered document does.
    #[must_use]
    pub fn stream_chunks(&self) -> Vec<Vec<u8>> {
        let chunk = |delta: serde_json::Value, finish: serde_json::Value| {
            serde_json::json!({
                "id": self.completion_id,
                "object": CHUNK_OBJECT,
                "created": self.created_seconds,
                "model": self.model,
                "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }],
            })
        };
        let mut documents = vec![chunk(
            serde_json::json!({ "role": ASSISTANT_ROLE, "content": "" }),
            serde_json::Value::Null,
        )];
        for piece in increments(&self.text) {
            documents.push(chunk(
                serde_json::json!({ "content": piece }),
                serde_json::Value::Null,
            ));
        }
        let mut terminal = chunk(
            serde_json::json!({}),
            serde_json::Value::String(FINISH_REASON_STOP.to_string()),
        );
        if let Some(members) = terminal.as_object_mut() {
            members.insert("usage".to_string(), self.usage());
        }
        documents.push(terminal);
        documents.iter().map(document).collect()
    }

    /// The token accounting this envelope reports: the prompt's tokens, the completion's,
    /// and their total.
    fn usage(&self) -> serde_json::Value {
        serde_json::json!({
            "prompt_tokens": self.prompt_tokens,
            "completion_tokens": self.completion_tokens,
            "total_tokens": self.prompt_tokens.saturating_add(self.completion_tokens),
        })
    }
}

/// The increments a streamed turn delivers its text in: one per sentence, the sentence's
/// own trailing space included, so the pieces concatenate back to the whole text.
fn increments(text: &str) -> Vec<&str> {
    let pieces: Vec<&str> = text.split_inclusive(". ").collect();
    if pieces.is_empty() {
        vec![text]
    } else {
        pieces
    }
}

/// One envelope document as the bytes of its payload.
fn document(value: &serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap_or_default()
}

/// `served_at` as the envelope's `created`: whole seconds since the epoch.
fn created_seconds(served_at: SystemTime) -> i64 {
    served_at
        .duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// How many tokens `text` is.
///
/// Measured with the byte-pair tokenizer crate's own `cl100k_base` vocabulary: no AWS
/// model publishes its vocabulary, so the count is at the granularity a model's tokenizer
/// counts at rather than a character heuristic, and it is deterministic in the text and in
/// nothing else. The text is caller content and nothing in it is a control marker, so it
/// is encoded as ORDINARY text.
fn count_tokens(text: &str) -> u32 {
    u32::try_from(tokenizer().encode_ordinary(text).len()).unwrap_or(u32::MAX)
}

/// The one tokenizer every count of this service is measured with. Nothing is read from
/// the host or the network to build it: the vocabulary ships with the crate.
fn tokenizer() -> &'static ::tiktoken_rs::CoreBPE {
    static TOKENIZER: ::std::sync::OnceLock<::tiktoken_rs::CoreBPE> = ::std::sync::OnceLock::new();
    TOKENIZER
        .get_or_init(|| ::tiktoken_rs::cl100k_base().expect("the shipped vocabulary is readable"))
}
