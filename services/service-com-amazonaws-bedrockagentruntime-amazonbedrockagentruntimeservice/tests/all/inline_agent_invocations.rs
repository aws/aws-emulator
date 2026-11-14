// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `InvokeInlineAgent` (`POST /agents/{sessionId}`).
//!
//! THE RULES UNDER TEST:
//!
//! * an inline agent is composed BY THE REQUEST, so there is no stored agent to resolve:
//!   what is resolved is the `foundationModel` it orchestrates with, read in the plane that
//!   owns the model catalogue — a reference naming a model this emulator publishes is
//!   SERVED, as an event stream carrying the `chunk` the modelled
//!   `InlineAgentResponseStream` declares, and one naming no such model is refused with the
//!   modelled error the inference plane classifies it as (`ValidationException` 400 for an
//!   identifier it cannot read, `ResourceNotFoundException` 404 for a resource it does not
//!   hold). No model is invoked here, so a model past its end of life still composes an
//!   agent, while one of a non-text modality — which generates no turn of a conversation at
//!   all — is refused;
//! * the `sessionId` is the CALLER's, and it addresses a conversation this plane stores:
//!   reusing it continues the same conversation — which is why the answer can report which
//!   turn of it was served — and `endSession` releases it, so a later request under the same
//!   id begins a new conversation at turn one;
//! * `streamingConfigurations.streamFinalResponse` changes the FRAMING and nothing else:
//!   the same answer is delivered in several `chunk` events instead of one, and the
//!   concatenation of those chunks is the text a single chunk carries;
//! * a leg of the request this emulator cannot compute is refused rather than ignored,
//!   while a configuration the agent would only use once a model elicited something from it
//!   — and an `enableTrace` rider, which reports alongside the answer rather than changing
//!   it — is served and reported on;
//! * the authorization decision comes FIRST: a caller whose identity-based policies do not
//!   allow `bedrock:InvokeInlineAgent` is refused with `AccessDeniedException` (403)
//!   whether or not the model it names exists, and the positive control is the same call by
//!   a permitted caller, which reaches the model reference's own refusal.
//!
//! The tables span what those rules generalise over: the FORM of the model reference (a bare
//! id, a `foundation-model` ARN, a cross-region inference-profile id, a model past its end
//! of life), the KIND of unusable reference (an id in no catalogue, a profile id whose base
//! model is in none, an ARN of a model the catalogue has no entry for, an ARN of an
//! account-scoped resource, a model of a non-text modality), how many turns a conversation
//! has taken, whether the final response is streamed, which member asks for work nothing
//! performs, and where the caller's grant comes from.

use base64::Engine as _;
use bytes::Bytes;
use serde_json::{json, Value};

use service_com_amazonaws_bedrockagentruntime_amazonbedrockagentruntimeservice::state::inline_agent;

use crate::agent_and_flow_invocations::{decode_frames, frame_names, EVENT_STREAM};
use crate::agent_memory::{allow, deny, Emulator, ACCOUNT_KEY, REGION};

/// An `instruction` long enough for the modelled `@length(min: 40)`, so a row is never
/// refused for the instruction when it is about something else.
const INSTRUCTION: &str =
    "Answer questions about the pump service schedule of the fleet, politely.";

/// The `contentType` an inline agent response stream is reported under, which the modelled
/// `x-amzn-bedrock-agent-content-type` header carries.
const RESPONSE_CONTENT_TYPE: &str = "application/json";

/// One answer to an `InvokeInlineAgent` call, read the way a client reads it: the status,
/// the body's content type, the two modelled response headers and the body itself.
struct Answer {
    status: u16,
    content_type: String,
    agent_content_type: Option<String>,
    session_header: Option<String>,
    body: Bytes,
}

impl Answer {
    /// The body as JSON, or `Value::Null` for an empty one.
    fn json(&self, case: &str) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "case {case:?}: expected a JSON body, got {:?} ({error})",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The wire error code restJson1 carries in `__type`, or `""` on a success body.
    fn error_code(&self, case: &str) -> String {
        self.json(case)
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string()
    }

    /// The error body's `message`.
    fn message(&self, case: &str) -> String {
        self.json(case)
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The text of every `chunk` event of the stream, in order, each decoded from the
    /// base64 `bytes` the frame's payload carries.
    fn chunks(&self, case: &str) -> Vec<String> {
        let frames = decode_frames(case, &self.body);
        assert!(
            frame_names(&frames).iter().all(|name| *name == "chunk"),
            "case {case:?}: expected only the modelled chunk event, got {:?}",
            frame_names(&frames)
        );
        frames
            .iter()
            .map(|frame| {
                assert_eq!(
                    frame.message_type, "event",
                    "case {case:?}: a chunk is a data event, got {:?}",
                    frame.message_type
                );
                let encoded = frame.payload["bytes"].as_str().unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: the chunk must carry bytes, got {:?}",
                        frame.payload
                    )
                });
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .unwrap_or_else(|error| {
                        panic!("case {case:?}: the chunk's bytes must be base64: {error}")
                    });
                String::from_utf8(decoded).unwrap_or_else(|error| {
                    panic!("case {case:?}: the chunk must be UTF-8: {error}")
                })
            })
            .collect()
    }

    /// The whole answer the stream delivered: every chunk, concatenated.
    fn text(&self, case: &str) -> String {
        self.chunks(case).concat()
    }
}

impl Emulator {
    /// One `InvokeInlineAgent` call over the modelled URI, signed with `key`.
    async fn invoke_inline_agent(&self, key: &str, session: &str, body: Value) -> Answer {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("/agents/{session}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={key}/20260101/{REGION}/bedrock/\
                     aws4_request, SignedHeaders=host, Signature=0"
                ),
            )
            .header("content-type", "application/json")
            .body(Bytes::from(
                serde_json::to_vec(&body).expect("the test body serializes"),
            ))
            .expect("the test request builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        };
        let content_type = header("content-type").unwrap_or_default();
        let agent_content_type = header("x-amzn-bedrock-agent-content-type");
        let session_header = header("x-amz-bedrock-agent-session-id");
        Answer {
            status,
            content_type,
            agent_content_type,
            session_header,
            body: response.into_body(),
        }
    }
}

/// One request body composing an inline agent against `foundation_model`.
fn request(foundation_model: &str) -> Value {
    json!({ "foundationModel": foundation_model, "instruction": INSTRUCTION })
}

/// Assert `answer` is a served invocation and give back the answer the stream delivered.
fn served(case: &str, answer: &Answer, session: &str) -> String {
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: expected 200, got {} {:?}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert_eq!(
        answer.content_type, EVENT_STREAM,
        "case {case:?}: expected an event-stream body, got {:?}",
        answer.content_type
    );
    assert_eq!(
        answer.agent_content_type.as_deref(),
        Some(RESPONSE_CONTENT_TYPE),
        "case {case:?}: expected the modelled contentType header, got {:?}",
        answer.agent_content_type
    );
    assert_eq!(
        answer.session_header.as_deref(),
        Some(session),
        "case {case:?}: expected the session the turn was served in, got {:?}",
        answer.session_header
    );
    answer.text(case)
}

/// RULE: a request naming a model this emulator publishes is served as an event stream
/// carrying the modelled `chunk`, whose text says no model ran, names the model the agent
/// was composed against and reports the turn that was served — never the caller's own
/// `inputText` handed back.
#[tokio::test]
async fn an_inline_agent_composed_against_a_published_model_is_served() {
    /// One invocation: how the model is referenced, which model that resolves to, and what
    /// else the request carries.
    struct Row {
        case: &'static str,
        session: &'static str,
        foundation_model: String,
        /// The model id the catalogue spells, which the answer must name.
        resolved: &'static str,
        agent_name: Option<&'static str>,
        /// The `inputText` the request carried, which the answer may not echo.
        input_text: Option<&'static str>,
        /// Extra body members, merged into the request.
        extra: Value,
    }

    let rows = vec![
        Row {
            case: "a bare model id, no agentName",
            session: "inline-session-01",
            foundation_model: "amazon.nova-lite-v1:0".to_string(),
            resolved: "amazon.nova-lite-v1:0",
            agent_name: None,
            input_text: Some("what is the pump service interval"),
            extra: json!({}),
        },
        Row {
            case: "a foundation-model ARN and an agentName",
            session: "inline-session-02",
            foundation_model: format!(
                "arn:aws:bedrock:{REGION}::foundation-model/meta.llama3-8b-instruct-v1:0"
            ),
            resolved: "meta.llama3-8b-instruct-v1:0",
            agent_name: Some("FleetAssistant"),
            input_text: None,
            extra: json!({}),
        },
        Row {
            case: "a text model past its end of life, which no inference here would run",
            session: "inline-session-05",
            foundation_model: "anthropic.claude-v2".to_string(),
            resolved: "anthropic.claude-v2",
            agent_name: Some("LegacyAssistant"),
            input_text: None,
            extra: json!({}),
        },
        Row {
            case: "a cross-region inference-profile id",
            session: "inline-session-03",
            foundation_model: "us.amazon.nova-pro-v1:0".to_string(),
            resolved: "amazon.nova-pro-v1:0",
            agent_name: Some("Dispatcher"),
            input_text: Some("book the next service"),
            extra: json!({}),
        },
        Row {
            case: "an invocation configuring action groups and knowledge bases",
            session: "inline-session-04",
            foundation_model: "anthropic.claude-3-5-sonnet-20241022-v2:0".to_string(),
            resolved: "anthropic.claude-3-5-sonnet-20241022-v2:0",
            agent_name: None,
            input_text: Some("open a work order"),
            extra: json!({
                "actionGroups": [{ "actionGroupName": "WorkOrders" }],
                "knowledgeBases": [
                    { "knowledgeBaseId": "KB12345678", "description": "service manuals" },
                    { "knowledgeBaseId": "KB87654321", "description": "parts catalogue" },
                ],
                "bedrockModelConfigurations": { "performanceConfig": { "latency": "standard" } },
                "agentCollaboration": "DISABLED",
                "orchestrationType": "DEFAULT",
            }),
        },
    ];

    let emulator = Emulator::new();
    for row in rows {
        let mut body = request(&row.foundation_model);
        if let Some(name) = row.agent_name {
            body["agentName"] = json!(name);
        }
        if let Some(input) = row.input_text {
            body["inputText"] = json!(input);
        }
        if let Some(extra) = row.extra.as_object() {
            for (member, value) in extra {
                body[member] = value.clone();
            }
        }
        let answer = emulator
            .invoke_inline_agent(ACCOUNT_KEY, row.session, body)
            .await;
        let text = served(row.case, &answer, row.session);
        assert_eq!(
            answer.chunks(row.case).len(),
            1,
            "case {:?}: an answer that is not streamed is one chunk, got {:?}",
            row.case,
            answer.chunks(row.case)
        );
        assert!(
            text.contains(row.resolved),
            "case {:?}: the answer must name the model the agent was composed against, got \
             {text:?}",
            row.case
        );
        assert!(
            text.contains(&format!("turn 1 of session {}", row.session)),
            "case {:?}: the answer must report the turn it served, got {text:?}",
            row.case
        );
        if let Some(name) = row.agent_name {
            assert!(
                text.contains(name),
                "case {:?}: the answer must name the agent the request composed, got {text:?}",
                row.case
            );
        }
        if let Some(input) = row.input_text {
            assert!(
                !text.contains(input),
                "case {:?}: the answer must not be the submitted input handed back, got \
                 {text:?}",
                row.case
            );
        }
        if row.case.contains("action groups") {
            assert!(
                text.contains("1 action group(s) and 2 knowledge base(s)")
                    && text.contains("went unused"),
                "case {:?}: the answer must report the configured capabilities it did not \
                 use, got {text:?}",
                row.case
            );
        }
    }
}

/// RULE: the `sessionId` addresses a conversation this plane stores — reusing it continues
/// the same conversation, and `endSession` releases it, so the next request under that id
/// begins a new conversation at turn one.
#[tokio::test]
async fn reusing_a_session_id_continues_the_same_conversation() {
    /// One turn of the table: whether it ends the session, and the turn number the answer
    /// must report for it.
    struct Row {
        case: &'static str,
        end_session: bool,
        expected_turn: usize,
    }

    let rows = vec![
        Row {
            case: "the first turn of a fresh conversation",
            end_session: false,
            expected_turn: 1,
        },
        Row {
            case: "a second turn under the same session id",
            end_session: false,
            expected_turn: 2,
        },
        Row {
            case: "a third turn under the same session id",
            end_session: false,
            expected_turn: 3,
        },
        Row {
            case: "the turn that ends the conversation",
            end_session: true,
            expected_turn: 4,
        },
        Row {
            case: "the first turn after the conversation was ended",
            end_session: false,
            expected_turn: 1,
        },
        Row {
            case: "a second turn of the conversation that replaced it",
            end_session: false,
            expected_turn: 2,
        },
    ];

    let emulator = Emulator::new();
    let session = "continued-session-01";
    for row in rows {
        let mut body = request("amazon.nova-lite-v1:0");
        body["inputText"] = json!("and the next one");
        if row.end_session {
            body["endSession"] = json!(true);
        }
        let answer = emulator
            .invoke_inline_agent(ACCOUNT_KEY, session, body)
            .await;
        let text = served(row.case, &answer, session);
        assert!(
            text.contains(&format!("turn {} of session {session}", row.expected_turn)),
            "case {:?}: expected turn {} of the conversation, got {text:?}",
            row.case,
            row.expected_turn
        );
        assert_eq!(
            text.contains("The session was ended by this request."),
            row.end_session,
            "case {:?}: the answer must say whether this request ended the session, got \
             {text:?}",
            row.case
        );
    }
}

/// RULE: `streamingConfigurations.streamFinalResponse` changes only the FRAMING — the
/// answer is delivered in successive chunks of the documented interval, and the
/// concatenation of those chunks is the text a single chunk carries.
#[tokio::test]
async fn streaming_the_final_response_changes_only_the_chunk_framing() {
    /// One invocation: whether it asks for the final response to be streamed, and what
    /// makes its answer longer or shorter.
    struct Row {
        case: &'static str,
        session: &'static str,
        /// `None` leaves `streamingConfigurations` out altogether.
        stream_final_response: Option<bool>,
        agent_name: Option<&'static str>,
    }

    let rows = vec![
        Row {
            case: "no streamingConfigurations at all",
            session: "framing-session-01",
            stream_final_response: None,
            agent_name: None,
        },
        Row {
            case: "streamFinalResponse explicitly false",
            session: "framing-session-02",
            stream_final_response: Some(false),
            agent_name: None,
        },
        Row {
            case: "streamFinalResponse true",
            session: "framing-session-03",
            stream_final_response: Some(true),
            agent_name: None,
        },
        Row {
            case: "streamFinalResponse true with a long agentName",
            session: "framing-session-04",
            stream_final_response: Some(true),
            agent_name: Some("FleetMaintenanceSchedulingAssistantForTheWesternRegion"),
        },
        Row {
            case: "streamFinalResponse true with a short agentName",
            session: "framing-session-05",
            stream_final_response: Some(true),
            agent_name: Some("Ops"),
        },
    ];

    let emulator = Emulator::new();
    for row in rows {
        let mut body = request("amazon.nova-lite-v1:0");
        if let Some(stream) = row.stream_final_response {
            body["streamingConfigurations"] = json!({ "streamFinalResponse": stream });
        }
        if let Some(name) = row.agent_name {
            body["agentName"] = json!(name);
        }
        let answer = emulator
            .invoke_inline_agent(ACCOUNT_KEY, row.session, body)
            .await;
        let text = served(row.case, &answer, row.session);
        let chunks = answer.chunks(row.case);
        let characters = text.chars().count();
        if row.stream_final_response == Some(true) {
            let expected = characters.div_ceil(inline_agent::STREAMED_CHUNK_CHARACTERS);
            assert_eq!(
                chunks.len(),
                expected,
                "case {:?}: expected {expected} chunks for {characters} characters, got {}",
                row.case,
                chunks.len()
            );
            for (index, chunk) in chunks.iter().enumerate() {
                let length = chunk.chars().count();
                if index + 1 < chunks.len() {
                    assert_eq!(
                        length,
                        inline_agent::STREAMED_CHUNK_CHARACTERS,
                        "case {:?}: chunk {index} must carry the documented interval, got \
                         {length}",
                        row.case
                    );
                } else {
                    assert!(
                        length > 0 && length <= inline_agent::STREAMED_CHUNK_CHARACTERS,
                        "case {:?}: the last chunk carries what is left, got {length}",
                        row.case
                    );
                }
            }
        } else {
            assert_eq!(
                chunks.len(),
                1,
                "case {:?}: an answer that is not streamed is one chunk, got {chunks:?}",
                row.case
            );
        }
        assert!(
            text.contains("amazon.nova-lite-v1:0")
                && text.contains(&format!("turn 1 of session {}", row.session)),
            "case {:?}: the whole answer must arrive however it is framed, got {text:?}",
            row.case
        );
    }

    // The two framings of the SAME invocation, delivered in two fresh conversations: the
    // streamed answer differs from the single-chunk one only in the session it names.
    let single = emulator
        .invoke_inline_agent(ACCOUNT_KEY, "framing-pair-single", request("amazon.nova-lite-v1:0"))
        .await;
    let mut streamed_body = request("amazon.nova-lite-v1:0");
    streamed_body["streamingConfigurations"] = json!({ "streamFinalResponse": true });
    let streamed = emulator
        .invoke_inline_agent(ACCOUNT_KEY, "framing-pair-streamed", streamed_body)
        .await;
    let case = "the same invocation framed both ways";
    let single_text = served(case, &single, "framing-pair-single");
    let streamed_text = served(case, &streamed, "framing-pair-streamed");
    assert_eq!(
        streamed_text.replace("framing-pair-streamed", "framing-pair-single"),
        single_text,
        "case {case:?}: streaming must change the framing and not the answer, got \
         {streamed_text:?} against {single_text:?}"
    );
    assert!(
        streamed.chunks(case).len() > 1,
        "case {case:?}: a streamed answer is delivered in more than one chunk"
    );
}

/// RULE: a `foundationModel` naming no model this emulator publishes is refused with the
/// modelled error the inference plane classifies the reference as, and the refusal records
/// nothing — the next served call under the same session id is still turn one.
#[tokio::test]
async fn a_foundation_model_this_emulator_does_not_publish_is_refused() {
    /// One unusable reference and the refusal it earns.
    struct Row {
        case: &'static str,
        session: &'static str,
        foundation_model: String,
        expected: (&'static str, u16),
        /// A phrase the refusal must state, so the caller can fix the request from it.
        message_contains: &'static str,
    }

    let rows = vec![
        Row {
            case: "a bare model id in no catalogue",
            session: "unusable-session-01",
            foundation_model: "acme.nonexistent-model-v9:0".to_string(),
            expected: ("ValidationException", 400),
            message_contains: "foundationModel",
        },
        Row {
            case: "a cross-region profile id whose base model is in no catalogue",
            session: "unusable-session-02",
            foundation_model: "us.acme.nonexistent-model-v9:0".to_string(),
            expected: ("ValidationException", 400),
            message_contains: "foundationModel",
        },
        Row {
            case: "a foundation-model ARN naming a model in no catalogue",
            session: "unusable-session-05",
            foundation_model: format!(
                "arn:aws:bedrock:{REGION}::foundation-model/acme.nonexistent-model-v9:0"
            ),
            expected: ("ResourceNotFoundException", 404),
            message_contains: "Could not resolve the model",
        },
        Row {
            case: "an ARN of a custom model this emulator holds none of",
            session: "unusable-session-03",
            foundation_model: format!(
                "arn:aws:bedrock:{REGION}:000000000000:custom-model/amazon.nova-lite-v1/abcdefghijkl"
            ),
            expected: ("ResourceNotFoundException", 404),
            message_contains: "Could not resolve",
        },
        Row {
            case: "an image model, which generates no turn of a conversation",
            session: "unusable-session-04",
            foundation_model: "amazon.nova-canvas-v1:0".to_string(),
            expected: ("ValidationException", 400),
            message_contains: "generates image, not text",
        },
        Row {
            case: "an embedding model, which generates no turn either",
            session: "unusable-session-06",
            foundation_model: "amazon.titan-embed-text-v2:0".to_string(),
            expected: ("ValidationException", 400),
            message_contains: "generates embedding, not text",
        },
    ];

    let emulator = Emulator::new();
    for row in rows {
        let answer = emulator
            .invoke_inline_agent(ACCOUNT_KEY, row.session, request(&row.foundation_model))
            .await;
        assert_eq!(
            (answer.error_code(row.case).as_str(), answer.status),
            row.expected,
            "case {:?}: expected {:?}, got {} {:?}",
            row.case,
            row.expected,
            answer.status,
            answer.json(row.case)
        );
        assert_ne!(
            answer.content_type, EVENT_STREAM,
            "case {:?}: a refused request must not answer an event stream",
            row.case
        );
        assert!(
            answer.message(row.case).contains(row.message_contains),
            "case {:?}: expected the refusal to state {:?}, got {:?}",
            row.case,
            row.message_contains,
            answer.message(row.case)
        );
        // Nothing was recorded for the refused call: the conversation under that id is
        // opened by the first call that is really served.
        let served_answer = emulator
            .invoke_inline_agent(ACCOUNT_KEY, row.session, request("amazon.nova-lite-v1:0"))
            .await;
        let text = served(row.case, &served_answer, row.session);
        assert!(
            text.contains(&format!("turn 1 of session {}", row.session)),
            "case {:?}: a refused call records no turn, got {text:?}",
            row.case
        );
    }
}

/// RULE: a leg of the request this emulator cannot compute is refused with the modelled
/// `ValidationException` (400) naming the member, while a configuration the agent would use
/// only once a model elicited something from it is served.
#[tokio::test]
async fn a_leg_this_emulator_cannot_compute_is_refused_rather_than_ignored() {
    /// One request body and whether it is served or refused.
    struct Row {
        case: &'static str,
        session: &'static str,
        /// Members merged into the request beyond the two required ones.
        extra: Value,
        /// The member a refusal must name, or `None` for a row that is served.
        refused_member: Option<&'static str>,
    }

    let rows = vec![
        Row {
            case: "a guardrail the invocation asks to be governed by",
            session: "leg-session-01",
            extra: json!({
                "guardrailConfiguration": {
                    "guardrailIdentifier": "abcdefghij",
                    "guardrailVersion": "DRAFT",
                },
            }),
            refused_member: Some("guardrailConfiguration"),
        },
        Row {
            case: "a guardrail interval applied while the response is generated",
            session: "leg-session-02",
            extra: json!({
                "streamingConfigurations": {
                    "streamFinalResponse": true,
                    "applyGuardrailInterval": 20,
                },
            }),
            refused_member: Some("streamingConfigurations.applyGuardrailInterval"),
        },
        Row {
            case: "an overridden prompt template",
            session: "leg-session-03",
            extra: json!({ "promptOverrideConfiguration": { "promptConfigurations": [] } }),
            refused_member: Some("promptOverrideConfiguration"),
        },
        Row {
            case: "a selection of the earlier turns that populate the prompt",
            session: "leg-session-04",
            extra: json!({
                "promptCreationConfigurations": { "previousConversationTurnsToInclude": 2 },
            }),
            refused_member: Some("promptCreationConfigurations"),
        },
        Row {
            case: "a session state carrying return-control results",
            session: "leg-session-05",
            extra: json!({
                "inlineSessionState": {
                    "invocationId": "11111111-2222-3333-4444-555555555555",
                    "returnControlInvocationResults": [],
                },
            }),
            refused_member: Some("inlineSessionState"),
        },
        Row {
            case: "a session state carrying only attributes",
            session: "leg-session-06",
            extra: json!({ "inlineSessionState": { "sessionAttributes": { "fleet": "west" } } }),
            refused_member: Some("inlineSessionState"),
        },
        Row {
            case: "a request asking for the orchestration trace",
            session: "leg-session-07",
            extra: json!({ "enableTrace": true }),
            refused_member: None,
        },
        Row {
            case: "inline collaborators to hand work to",
            session: "leg-session-08",
            extra: json!({
                "collaborators": [{
                    "foundationModel": "amazon.nova-lite-v1:0",
                    "instruction": INSTRUCTION,
                }],
            }),
            refused_member: Some("collaborators"),
        },
        Row {
            case: "the settings of a collaborator agent",
            session: "leg-session-09",
            extra: json!({
                "collaboratorConfigurations": [{
                    "collaboratorName": "Scheduler",
                    "collaboratorInstruction": "Schedule the service visit.",
                }],
            }),
            refused_member: Some("collaboratorConfigurations"),
        },
        Row {
            case: "a supervisor collaboration",
            session: "leg-session-10",
            extra: json!({ "agentCollaboration": "SUPERVISOR" }),
            refused_member: Some("agentCollaboration"),
        },
        Row {
            case: "a routing supervisor collaboration",
            session: "leg-session-11",
            extra: json!({ "agentCollaboration": "SUPERVISOR_ROUTER" }),
            refused_member: Some("agentCollaboration"),
        },
        Row {
            case: "an orchestration driven by the caller's own function",
            session: "leg-session-12",
            extra: json!({ "orchestrationType": "CUSTOM_ORCHESTRATION" }),
            refused_member: Some("orchestrationType"),
        },
        Row {
            case: "a custom orchestration executor",
            session: "leg-session-13",
            extra: json!({
                "customOrchestration": {
                    "executor": {
                        "lambda": "arn:aws:lambda:us-west-2:000000000000:function:orchestrate",
                    },
                },
            }),
            refused_member: Some("customOrchestration"),
        },
        Row {
            case: "collaboration explicitly disabled, which is what this service serves",
            session: "leg-session-14",
            extra: json!({ "agentCollaboration": "DISABLED" }),
            refused_member: None,
        },
        Row {
            case: "the default orchestration, named explicitly",
            session: "leg-session-15",
            extra: json!({ "orchestrationType": "DEFAULT" }),
            refused_member: None,
        },
        Row {
            case: "a latency profile for the model the agent calls",
            session: "leg-session-16",
            extra: json!({
                "bedrockModelConfigurations": { "performanceConfig": { "latency": "optimized" } },
            }),
            refused_member: None,
        },
        Row {
            case: "an action group the agent may use and a key to encrypt the session with",
            session: "leg-session-17",
            extra: json!({
                "actionGroups": [{ "actionGroupName": "WorkOrders" }],
                "customerEncryptionKeyArn":
                    "arn:aws:kms:us-west-2:000000000000:key/11111111-2222-3333-4444-555555555555",
                "idleSessionTTLInSeconds": 900,
            }),
            refused_member: None,
        },
    ];

    let emulator = Emulator::new();
    for row in rows {
        let mut body = request("amazon.nova-lite-v1:0");
        if let Some(extra) = row.extra.as_object() {
            for (member, value) in extra {
                body[member] = value.clone();
            }
        }
        let answer = emulator
            .invoke_inline_agent(ACCOUNT_KEY, row.session, body)
            .await;
        match row.refused_member {
            Some(member) => {
                assert_eq!(
                    (answer.error_code(row.case).as_str(), answer.status),
                    ("ValidationException", 400),
                    "case {:?}: expected the modelled validation refusal, got {} {:?}",
                    row.case,
                    answer.status,
                    answer.json(row.case)
                );
                assert!(
                    answer.message(row.case).contains(member),
                    "case {:?}: the refusal must name {member:?}, got {:?}",
                    row.case,
                    answer.message(row.case)
                );
                assert_ne!(
                    answer.content_type, EVENT_STREAM,
                    "case {:?}: a refused request must not answer an event stream",
                    row.case
                );
            }
            None => {
                let text = served(row.case, &answer, row.session);
                assert!(
                    text.contains("amazon.nova-lite-v1:0"),
                    "case {:?}: the answer must name the model the agent was composed \
                     against, got {text:?}",
                    row.case
                );
                // A trace is a report ALONGSIDE the answer, so a request that enables one
                // is answered and told that it carries no step.
                assert_eq!(
                    text.contains("The trace this request enabled reports no step"),
                    row.extra.get("enableTrace") == Some(&json!(true)),
                    "case {:?}: the answer must say whether an enabled trace reports \
                     anything, got {text:?}",
                    row.case
                );
                assert!(
                    !text.contains("trace") || row.extra.get("enableTrace") == Some(&json!(true)),
                    "case {:?}: an answer must not mention a trace nobody asked for, got \
                     {text:?}",
                    row.case
                );
            }
        }
    }
}

/// RULE: the authorization decision comes first — a caller whose identity-based policies do
/// not allow `bedrock:InvokeInlineAgent` is refused with `AccessDeniedException` (403)
/// whether or not the model it names exists. The positive control is the same call by a
/// permitted caller, which reaches the model reference's own refusal.
#[tokio::test]
async fn a_caller_without_the_action_is_refused_before_the_model_is_resolved() {
    /// Where one caller's grant comes from, and what the call answers for it.
    struct Row {
        case: &'static str,
        policy: Option<String>,
        expected: (&'static str, u16),
    }

    let rows = vec![
        Row {
            case: "a principal with no policy at all",
            policy: None,
            expected: ("AccessDeniedException", 403),
        },
        Row {
            case: "a principal granted an unrelated action",
            policy: Some(allow(&["bedrock:GetAgentMemory"])),
            expected: ("AccessDeniedException", 403),
        },
        Row {
            case: "a principal granted the action and denied it outright",
            policy: Some(deny(&["bedrock:InvokeInlineAgent"])),
            expected: ("AccessDeniedException", 403),
        },
        Row {
            case: "a principal granted exactly the inline-agent action",
            policy: Some(allow(&["bedrock:InvokeInlineAgent"])),
            // Allowed through to the model reference, which names no model this emulator
            // serves.
            expected: ("ValidationException", 400),
        },
        Row {
            case: "a principal granted every action of the service",
            policy: Some(allow(&["bedrock:*"])),
            expected: ("ValidationException", 400),
        },
    ];

    let emulator = Emulator::new();
    for (index, row) in rows.into_iter().enumerate() {
        let key = emulator
            .assumed_role_key(&format!("inline-agent-caller-{index}"), row.policy.as_deref())
            .await;
        let answer = emulator
            .invoke_inline_agent(
                &key,
                &format!("authz-session-{index}"),
                request("acme.nonexistent-model-v9:0"),
            )
            .await;
        assert_eq!(
            (answer.error_code(row.case).as_str(), answer.status),
            row.expected,
            "case {:?}: expected {:?}, got {} {:?}",
            row.case,
            row.expected,
            answer.status,
            answer.json(row.case)
        );
    }
}
