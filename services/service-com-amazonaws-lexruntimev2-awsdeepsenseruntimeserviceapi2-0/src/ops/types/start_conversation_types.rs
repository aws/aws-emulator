// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `start_conversation` operation: the
//! label-bound half of its input, the event structures its `@streaming` request
//! union carries, and the events it frames into the response stream.
//!
//! Each structure/union is declared exactly once across this service's `types/`
//! modules, and `types/mod.rs` re-exports them all — the shapes the session
//! operations already declare (`Message`, `SessionState`, `Interpretation`,
//! `RecognizedBotMember`, every exception) are reused from `shared_types`,
//! `intent_types` and `recognize_text_types` rather than re-declared here.
//!
//! ## Why a `Settings` struct exists
//!
//! `StartConversation` binds four `@httpLabel`s and one `@httpHeader`, and its
//! remaining input member is the `@streaming` event-stream `@httpPayload`, whose
//! framing the generated codec does not speak. Declaring the request shape with
//! that one member in `unsupported = [..]` gives a struct the framework's own
//! HTTP-binding deserializer fills from the request's path and headers and
//! `constraints::validate` checks against the model's `@length`, `@pattern` and
//! enum-membership traits — so the addressing half of the request is validated by
//! the MODEL, not by hand — while the payload half is decoded from real
//! `vnd.amazon.eventstream` frames in the operation itself.
//!
//! No event structure of the response stream is declared beyond the two this
//! service frames: a transcript event is "a transcript of voice audio"
//! (`smithy_query(query="doc", shape="TranscriptEvent")`), which needs a speech
//! recogniser this emulator does not have, and a heartbeat "indicates that the
//! stream is still open", which a buffered request never is by the time the
//! response is written. The in-band exception members it does frame are already
//! declared in `shared_types`.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-lexruntimev2-awsdeepsenseruntimeserviceapi2-0.json");

/// The members of `StartConversationRequestEventStream`, as the `:event-type`
/// header of a request frame spells them
/// (`smithy_query(query="shape", shape="StartConversationRequestEventStream")`).
pub const MEMBER_CONFIGURATION_EVENT: &str = "ConfigurationEvent";
/// See [`MEMBER_CONFIGURATION_EVENT`].
pub const MEMBER_AUDIO_INPUT_EVENT: &str = "AudioInputEvent";
/// See [`MEMBER_CONFIGURATION_EVENT`].
pub const MEMBER_DTMF_INPUT_EVENT: &str = "DTMFInputEvent";
/// See [`MEMBER_CONFIGURATION_EVENT`].
pub const MEMBER_TEXT_INPUT_EVENT: &str = "TextInputEvent";
/// See [`MEMBER_CONFIGURATION_EVENT`].
pub const MEMBER_PLAYBACK_COMPLETION_EVENT: &str = "PlaybackCompletionEvent";
/// See [`MEMBER_CONFIGURATION_EVENT`].
pub const MEMBER_DISCONNECTION_EVENT: &str = "DisconnectionEvent";

/// The members of `StartConversationResponseEventStream` this service frames into
/// a response stream
/// (`smithy_query(query="shape", shape="StartConversationResponseEventStream")`).
pub const MEMBER_INTENT_RESULT_EVENT: &str = "IntentResultEvent";
/// See [`MEMBER_INTENT_RESULT_EVENT`].
pub const MEMBER_TEXT_RESPONSE_EVENT: &str = "TextResponseEvent";
/// See [`MEMBER_INTENT_RESULT_EVENT`]: the in-band exception members, which are
/// the ONLY channel the model gives these shapes on this operation — they are not
/// among the errors `StartConversation` declares.
pub const MEMBER_RESOURCE_NOT_FOUND_EXCEPTION: &str = "ResourceNotFoundException";
/// See [`MEMBER_RESOURCE_NOT_FOUND_EXCEPTION`].
pub const MEMBER_DEPENDENCY_FAILED_EXCEPTION: &str = "DependencyFailedException";
/// See [`MEMBER_RESOURCE_NOT_FOUND_EXCEPTION`].
pub const MEMBER_THROTTLING_EXCEPTION: &str = "ThrottlingException";

/// The values `ConversationMode` accepts on the wire
/// (`smithy_query(query="shape", shape="ConversationMode")`).
pub const CONVERSATION_MODE_AUDIO: &str = "AUDIO";
/// See [`CONVERSATION_MODE_AUDIO`].
pub const CONVERSATION_MODE_TEXT: &str = "TEXT";

/// The values `InputMode` accepts on the wire — "whether the input mode to the
/// operation was text, speech, or from a touch-tone keypad"
/// (`smithy_query(query="shape", shape="InputMode")`: Text | Speech | DTMF).
pub const INPUT_MODE_TEXT: &str = "Text";
/// See [`INPUT_MODE_TEXT`].
pub const INPUT_MODE_DTMF: &str = "DTMF";

/// The label- and header-bound half of `StartConversationRequest`;
/// `requestEventStream` is the event-stream payload and is decoded frame by frame
/// instead.
#[smithy_macros::smithy(
    shape = "com.amazonaws.lexruntimev2#StartConversationRequest",
    unsupported = ["requestEventStream"]
)]
#[derive(Debug, Clone, Default)]
pub struct StartConversationSettings {
    pub bot_id: String,
    pub bot_alias_id: String,
    pub locale_id: String,
    pub session_id: String,
    pub conversation_mode: Option<String>,
}

/// "The initial event sent from the application to Amazon Lex V2 to configure the
/// conversation, including session and request attributes and the response
/// content type."
#[smithy_macros::smithy(shape = "com.amazonaws.lexruntimev2#ConfigurationEvent")]
#[derive(Debug, Clone, Default)]
pub struct ConfigurationEvent {
    pub request_attributes: Option<std::collections::HashMap<String, String>>,
    pub response_content_type: String,
    pub session_state: Option<SessionState>,
    pub welcome_messages: Option<Vec<Message>>,
    pub disable_playback: bool,
    pub event_id: Option<String>,
    pub client_timestamp_millis: i64,
}

/// One chunk of audio from the client. `audioChunk` is an ordinary blob member,
/// so it arrives base64-encoded inside the frame's JSON payload.
#[smithy_macros::smithy(shape = "com.amazonaws.lexruntimev2#AudioInputEvent")]
#[derive(Debug, Clone, Default)]
pub struct AudioInputEvent {
    pub audio_chunk: Option<Vec<u8>>,
    pub content_type: String,
    pub event_id: Option<String>,
    pub client_timestamp_millis: i64,
}

/// "A DTMF character sent from the client application", one per event.
#[smithy_macros::smithy(shape = "com.amazonaws.lexruntimev2#DTMFInputEvent")]
#[derive(Debug, Clone, Default)]
pub struct DTMFInputEvent {
    pub input_character: String,
    pub event_id: Option<String>,
    pub client_timestamp_millis: i64,
}

/// "The event sent from your client application to Amazon Lex V2 with text input
/// from the user."
#[smithy_macros::smithy(shape = "com.amazonaws.lexruntimev2#TextInputEvent")]
#[derive(Debug, Clone, Default)]
pub struct TextInputEvent {
    pub text: String,
    pub event_id: Option<String>,
    pub client_timestamp_millis: i64,
}

/// "Event sent from the client application to Amazon Lex V2 to indicate that
/// playback of audio is complete."
#[smithy_macros::smithy(shape = "com.amazonaws.lexruntimev2#PlaybackCompletionEvent")]
#[derive(Debug, Clone, Default)]
pub struct PlaybackCompletionEvent {
    pub event_id: Option<String>,
    pub client_timestamp_millis: i64,
}

/// "A notification from the client that it is disconnecting from Amazon Lex V2."
#[smithy_macros::smithy(shape = "com.amazonaws.lexruntimev2#DisconnectionEvent")]
#[derive(Debug, Clone, Default)]
pub struct DisconnectionEvent {
    pub event_id: Option<String>,
    pub client_timestamp_millis: i64,
}

/// "Contains the current state of the conversation between the client application
/// and Amazon Lex V2" — the event one turn of the conversation reports itself in.
#[smithy_macros::smithy(shape = "com.amazonaws.lexruntimev2#IntentResultEvent")]
#[derive(Debug, Clone, Default)]
pub struct IntentResultEvent {
    pub input_mode: Option<String>,
    pub interpretations: Option<Vec<Interpretation>>,
    pub session_state: Option<SessionState>,
    pub request_attributes: Option<std::collections::HashMap<String, String>>,
    pub session_id: Option<String>,
    pub event_id: Option<String>,
    pub recognized_bot_member: Option<RecognizedBotMember>,
}

/// "The event sent from Amazon Lex V2 to your application with text to present to
/// the user."
#[smithy_macros::smithy(shape = "com.amazonaws.lexruntimev2#TextResponseEvent")]
#[derive(Debug, Clone, Default)]
pub struct TextResponseEvent {
    pub messages: Option<Vec<Message>>,
    pub event_id: Option<String>,
}
