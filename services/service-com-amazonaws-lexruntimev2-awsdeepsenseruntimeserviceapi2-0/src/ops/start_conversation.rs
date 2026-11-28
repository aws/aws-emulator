// SPDX-License-Identifier: CC0-1.0

//! @op StartConversation
//!
//! `POST /bots/{botId}/botAliases/{botAliasId}/botLocales/{localeId}/sessions/{sessionId}/conversation -> 200`.
//! The bidirectional half of the Lex V2 runtime: "starts an HTTP/2 bidirectional
//! event stream that enables you to send audio, text, or DTMF input in real time.
//! After your application starts a conversation, users send input to Amazon Lex V2
//! as a stream of events. Amazon Lex V2 processes the incoming events and responds
//! with streaming text or audio events."
//!
//! ## What one request does
//!
//! The emulator's front door serves one BUFFERED request per call, so the events a
//! client framed into the request body are the whole of the input stream and the
//! frames written back are the whole of the response stream. The events are
//! processed in ARRIVAL ORDER, which is what makes the two streams a conversation:
//!
//! * `ConfigurationEvent` — "the initial event sent from the application to Amazon
//!   Lex V2 to configure the conversation, including session and request attributes
//!   and the response content type", so it is the event the stream opens with and
//!   what it carries is applied to the session before any input is interpreted. Its
//!   `welcomeMessages` are "a list of messages to send to the user", which this
//!   service sends as the stream's first `TextResponseEvent` and stores as the
//!   messages the session last sent.
//! * `TextInputEvent` — one conversation TURN, the same turn
//!   [`crate::state::take_turn`] takes for `RecognizeText` and `RecognizeUtterance`,
//!   so one utterance means the same thing whichever of the three carries it. The
//!   turn is reported as an `IntentResultEvent` ("the current state of the
//!   conversation between the client application and Amazon Lex V2") followed by a
//!   `TextResponseEvent` carrying whatever the bot has to say at the step the dialog
//!   reached.
//! * `DTMFInputEvent` — "a DTMF character sent from the client application", one per
//!   event. The characters of one entry ACCUMULATE and the entry is what a turn
//!   interprets: the deletion character "clears the accumulated DTMF digits and
//!   immediately ends the input" and the end character "immediately ends input"
//!   (`smithy_query(query="shape", shape="DTMFSpecification")` on the Lex Models V2
//!   model, whose documented defaults are [`DTMF_DELETION_CHARACTER`] and
//!   [`DTMF_END_CHARACTER`]; the per-slot specification that can change them is not
//!   part of the built locale this runtime reads). Such a turn ran in the `DTMF`
//!   input mode.
//! * `AudioInputEvent` — speech, which needs a recogniser this emulator does not
//!   have: the stream carries the modelled `DependencyFailedException` instead of an
//!   invented transcript, exactly as the sibling `RecognizeUtterance` refuses audio
//!   input.
//! * `PlaybackCompletionEvent` — "playback of audio is complete and … Amazon Lex V2
//!   should start processing the user's input". No audio response is ever framed
//!   here, so there is no playback to complete and the event advances nothing.
//! * `DisconnectionEvent` — "a notification from the client that it is disconnecting
//!   from Amazon Lex V2", so the conversation ends there and the rest of the body is
//!   not read. The SESSION survives it, for the idle retention a session has.
//!
//! ## Which failures travel where
//!
//! A failure of the REQUEST AS A MESSAGE — its address labels, its conversation mode,
//! or a body that cannot be read as a sequence of event frames at all (not a message
//! stream, past the frame or byte budget, or carrying no event where the payload member
//! is `@required`) — is answered with the modelled HTTP error, as every other operation
//! of this service answers one. Everything the EVENTS then say is answered on the
//! stream: the response is open from the first event onwards, so a payload the model
//! refuses, a member the conversation mode does not admit, a member the input union does
//! not declare, an address that resolves to no built bot locale, speech this emulator
//! cannot process and a session store at capacity all travel as the in-band exception
//! member the response union declares for that shape, which ends the conversation and
//! leaves the frames of the turns already taken readable.
//!
//! That the union is the channel for them is the model's own doing:
//! `StartConversation` declares only `AccessDeniedException`,
//! `InternalServerException`, `ThrottlingException` and `ValidationException` as its
//! errors, while `StartConversationResponseEventStream` declares
//! `ResourceNotFoundException`, `DependencyFailedException`, `ConflictException` and
//! `BadGatewayException` as well — so for half of what a conversation can fail with,
//! in band is the only channel there is.
//!
//! ## Why this operation is hand-authored
//!
//! Its input carries a `@streaming` event-stream member and its output is a
//! `@streaming` union. The op-stub generator declines the first (see
//! `ops/SKIPPED_STUBS.md`) and the generated `ErasedOperation` shim declines the
//! second, because `serialize_as_event_stream<P>` needs a `Sized` protocol that an
//! object-safe `run` cannot hand it. The framing, the request reader, the frame
//! decoder and the routing seam are the shared ones in
//! `service_common::event_stream`; `src/service.rs` routes a request here through
//! `Service::override_operation`, which the router consults BEFORE the generated
//! routing table.
//!
//! ## Input
//!
//! * `botId` (BotIdentifier, len 10..10, pattern `^[0-9a-zA-Z]+$`) **required**, `@httpLabel`
//! * `botAliasId` (BotAliasIdentifier) **required**, `@httpLabel`
//! * `localeId` (LocaleId, len 1..) **required**, `@httpLabel`
//! * `sessionId` (SessionId, len 2..100, pattern `^[0-9a-zA-Z._:-]+$`) **required**, `@httpLabel`
//! * `conversationMode` (ConversationMode: AUDIO | TEXT, `@httpHeader
//!   x-amz-lex-conversation-mode`) — "if the conversation mode is AUDIO you can send
//!   both audio and DTMF information. If the mode is TEXT you can only send text."
//! * `requestEventStream` (StartConversationRequestEventStream, `@httpPayload`,
//!   `@streaming`) **required**
//!
//! ## Output
//!
//! * `responseEventStream` (StartConversationResponseEventStream, `@httpPayload`,
//!   `@streaming`) — the whole of the output, so the 200 carries no modelled header.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403)
//! * `InternalServerException` (HTTP 500)
//! * `ThrottlingException` (HTTP 429)
//! * `ValidationException` (HTTP 400)

#[allow(unused_imports)]
use super::types::*;
use crate::state::bot_definition::SeedRefused;
use crate::state::bot_model::{built_locale, AddressRefused, BotLocaleModel};
use crate::state::{
    asks_for_speech, check_attribute_bytes, check_request_attributes, merge_attributes,
    AttributesRefused, SessionKey, SessionStoreFull, Turn, TurnRefused, MAX_UTTERANCE_CHARS,
    TEXT_CONTENT_TYPE,
};
use crate::ComAmazonawsLexruntimev2Awsdeepsenseruntimeserviceapi20State as SessionStore;
use aws_smithy_runtime_api::http::{Request, Response};
use aws_smithy_schema::serde::SerializableStruct;
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::{missing_member_message, DeserializableStruct};
use aws_smithy_types::body::SdkBody;
use service_common::event_stream::{
    decode_input_stream, error_response, event_stream_only_response, frame_empty_initial_response,
    frame_event, read_request, DecodedEvent, EventStreamOperation, InputStreamRefusal,
    RequestRefusal,
};
use smithy_server_runtime::{
    BoxFuture, OperationContext, OperationError, ServerHttpProtocol, Service,
};
use std::sync::Arc;
use std::time::SystemTime;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-lexruntimev2-awsdeepsenseruntimeserviceapi2-0.json");

/// The operation name, as the model names it and as the generated routing table
/// keys it — which is how `src/service.rs` recognises a request for it.
pub const OPERATION: &str = "StartConversation";

/// The most events ONE request's stream may carry.
///
/// A resource bound, not a modelled quota: the frame count is caller-chosen and
/// every input event of a conversation costs a turn — an interpretation of the input
/// against the bot's locale — so a stream past this is refused BEFORE the work is
/// done rather than after.
pub const MAX_CONVERSATION_EVENTS: usize = 256;

/// The most bytes of event frames ONE request's stream may carry.
///
/// The frame budget above bounds how many frames are read; this bounds how large the
/// body they are read out of may be, since a frame's own payload is caller-sized. Both
/// are applied BEFORE the body is decoded, so neither the decode nor the conversation
/// behind it is work a caller can size without limit. The number is generous for what a
/// conversation carries: the model bounds one utterance at 1024 characters, and the one
/// content type this conversation serves is text.
pub const MAX_CONVERSATION_STREAM_BYTES: usize = 1024 * 1024;

/// The DTMF character that "immediately ends input", and the one that "clears the
/// accumulated DTMF digits and immediately ends the input"
/// (`DTMFSpecification$endCharacter` / `$deletionCharacter` of the Lex Models V2
/// model, for which AWS documents the defaults `#` and `*`). Both are inside the
/// `DTMFRegex` the runtime's own `DTMFInputEvent$inputCharacter` declares
/// (`^[A-D0-9#*]{1}$`).
pub const DTMF_END_CHARACTER: &str = "#";
/// See [`DTMF_END_CHARACTER`].
pub const DTMF_DELETION_CHARACTER: &str = "*";

/// What this service answers a stream whose first event is not the configuration
/// event: the configuration is "the INITIAL event sent from the application to
/// Amazon Lex V2 to configure the conversation", so a conversation cannot start
/// without it and nothing may precede it.
pub const CONFIGURATION_EVENT_FIRST: &str =
    "the request event stream must begin with a ConfigurationEvent, the initial event that \
     configures the conversation";

/// What this service answers a `welcomeMessages` list sent without a dialog action to
/// send it in: "if you set the welcomeMessage field, you must also set the
/// DialogAction structure's type field"
/// (`smithy_query(query="doc", shape="ConfigurationEvent", member="welcomeMessages")`)
/// — a rule ACROSS two members, which no constraint trait can express.
pub const WELCOME_MESSAGES_NEED_A_DIALOG_ACTION: &str =
    "welcomeMessages were sent without sessionState.dialogAction.type: if you set the \
     welcomeMessages field, you must also set the DialogAction structure's type field";

/// The operation's MODELLED errors; `#[operation_error]` accepts only variants the
/// model declares for it, which for this operation is four of the eight shapes its
/// response stream can carry in band.
#[smithy_macros::operation_error("com.amazonaws.lexruntimev2#StartConversation")]
#[derive(Debug)]
pub enum StartConversationErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// A request the caller has to fix, worded so the member and the rule it broke are
/// both in the message — the channel every other operation of this service refuses a
/// bad request through.
fn invalid(message: impl Into<String>) -> StartConversationErrorModeled {
    StartConversationErrorModeled::ValidationException(ValidationException {
        message: message.into(),
    })
}

/// A failure that is this emulator's own and not the caller's.
fn internal(message: impl Into<String>) -> StartConversationErrorModeled {
    StartConversationErrorModeled::InternalServerException(InternalServerException {
        message: message.into(),
    })
}

/// The refusal of a member this operation reads itself, worded the way the rest of
/// this service words the same refusal: the FRAMEWORK's own clause, except for the
/// one member this service states itself — a `botId` outside what `BotIdentifier`
/// declares, which carries this service's fixed bot-address sentence wherever a
/// request names one. The generated operations reach that sentence through the
/// service's `override_refusal` seam; this operation reads its own request, so it
/// asks for the sentence there rather than growing a second wording of it.
fn refused_member(refusal: RequestRefusal) -> StartConversationErrorModeled {
    if let RequestRefusal::Invalid(violations) = &refusal {
        if let Some(sentence) = crate::service::bot_id_refusal::sentence_for(violations) {
            return invalid(sentence);
        }
    }
    invalid(refusal.stated())
}

/// A body that is not a usable request event stream: both arms are the caller's to
/// fix — a malformed message stream is a bad request, and a stream past this
/// emulator's frame budget is a request to send fewer, larger events.
fn refused_stream(refusal: InputStreamRefusal) -> StartConversationErrorModeled {
    invalid(refusal.stated())
}

/// An attribute map the conversation may not store: the member and the rule it broke
/// are in the message, so the request can be fixed from it.
fn refused_attributes(error: AttributesRefused) -> StartConversationErrorModeled {
    invalid(error.message())
}

/// A configuration naming a dialog the addressed bot locale does not define.
fn refused_seed(error: SeedRefused) -> StartConversationErrorModeled {
    invalid(error.message())
}

/// An event kind the conversation mode does not admit: "if the conversation mode is
/// AUDIO you can send both audio and DTMF information. If the mode is TEXT you can
/// only send text" (`StartConversationRequest$conversationMode`).
fn refused_in_text_mode(event_type: &str) -> StartConversationErrorModeled {
    invalid(format!(
        "a {event_type} may not be sent on a conversation whose conversationMode is \
         {CONVERSATION_MODE_TEXT}, which carries text input only; send a \
         {MEMBER_TEXT_INPUT_EVENT}, or open the conversation in {CONVERSATION_MODE_AUDIO} mode"
    ))
}

/// Which input events a conversation admits, which is what its `conversationMode`
/// decides. A request that names no mode is not narrowed: `AUDIO` "can send both
/// audio and DTMF information", so it is the mode that admits every event the input
/// union declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// `AUDIO`, and the absent header: every member of the input union.
    Audio,
    /// `TEXT`: text input only.
    Text,
}

impl Mode {
    /// The mode `conversation_mode` names. The value has already been checked
    /// against the modelled `ConversationMode` enum by the request reader, so
    /// anything other than `TEXT` is the audio mode.
    fn of(conversation_mode: Option<&str>) -> Self {
        match conversation_mode {
            Some(CONVERSATION_MODE_TEXT) => Self::Text,
            _ => Self::Audio,
        }
    }
}

/// What ended a conversation before its stream ran out of events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ended {
    /// The client said it is disconnecting, so nothing behind that event is read.
    Disconnected,
    /// An in-band exception closed the stream; its frame is already written.
    InBand,
}

/// The `StartConversation` operation.
///
/// Owns the shared engine (the constructor seam), like every other operation of this
/// crate. The generated routing table holds one of these — whose generated
/// `ErasedOperation` shim declines an event-stream request — and `src/service.rs`
/// holds another wrapped in an `EventStreamRoute`; both reach the same sessions,
/// because the sessions live in the shared engine and not on the operation.
pub struct StartConversation<Eng> {
    engine: Arc<Eng>,
}

impl<Eng> StartConversation<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: Send + Sync + 'static> StartConversation<Eng> {
    /// The whole request-to-response step: read the address, decode the request
    /// stream, hold the conversation its events describe, and answer the frames the
    /// conversation produced.
    fn respond(
        &self,
        protocol: &dyn ServerHttpProtocol,
        req: &Request,
    ) -> Result<Response, StartConversationErrorModeled> {
        // The label- and header-bound half of the request, read and checked by the
        // MODEL before anything of the body is looked at.
        let settings: StartConversationSettings = address_of(protocol, req)?;

        let body = req.body().bytes().ok_or_else(|| {
            internal("the request event stream could not be read as bytes".to_string())
        })?;
        if body.len() > MAX_CONVERSATION_STREAM_BYTES {
            return Err(invalid(format!(
                "one request stream carries at most {MAX_CONVERSATION_STREAM_BYTES} bytes of \
                 event frames, and this one carries {}; hold the conversation over more \
                 requests",
                body.len()
            )));
        }
        let events = decode_input_stream(body, MAX_CONVERSATION_EVENTS).map_err(refused_stream)?;
        // `requestEventStream` is `@required`, so a request that frames no event at
        // all carries no payload — stated as the framework states every other absent
        // required member.
        if events.is_empty() {
            return Err(invalid(missing_member_message("requestEventStream")));
        }

        // The body opens with the frame a client reads the OUTPUT shape out of:
        // `StartConversationResponse`'s only member is the stream itself, so that
        // frame carries no member.
        let mut frames = frame_empty_initial_response(protocol).map_err(|error| {
            internal(format!("the initial response could not be framed: {error}"))
        })?;

        // What the input is interpreted against: the bot locale the alias this
        // request addresses points at, which the client built through the Lex Models
        // V2 control plane. The model gives this operation no
        // `ResourceNotFoundException`, so an address that resolves to no built locale
        // is reported IN BAND, on the stream the 200 opened.
        let model = match built_locale(
            &*self.engine,
            &settings.bot_id,
            &settings.bot_alias_id,
            &settings.locale_id,
        ) {
            Ok(model) => model,
            Err(error) => match error.refusal(&settings.bot_id, &settings.bot_alias_id) {
                AddressRefused::NotFound(message) => {
                    frames.extend(frame_exception(
                        protocol,
                        MEMBER_RESOURCE_NOT_FOUND_EXCEPTION,
                        &ResourceNotFoundException { message },
                        &SCHEMA_RESOURCE_NOT_FOUND_EXCEPTION,
                    )?);
                    return Ok(event_stream_only_response(frames));
                }
                // The store that holds the bots could not be read, which is this
                // emulator's own fault and an error the model DOES give this
                // operation.
                AddressRefused::Internal(message) => return Err(internal(message)),
            },
        };

        let state = crate::state::service_state(&*self.engine)
            .ok_or_else(|| internal("the Lex V2 runtime session store is unavailable"))?;

        let mut conversation = Conversation {
            protocol,
            state: &state,
            key: SessionKey::new(
                &settings.bot_id,
                &settings.bot_alias_id,
                &settings.locale_id,
                &settings.session_id,
            ),
            model,
            mode: Mode::of(settings.conversation_mode.as_deref()),
            dtmf_entry: String::new(),
            frames,
        };
        conversation.run(&events)?;
        Ok(event_stream_only_response(conversation.frames))
    }
}

/// One conversation being held: what it is interpreted against, what it has keyed in
/// so far, and the response stream it has written.
struct Conversation<'c> {
    /// This service's wire protocol, which every frame is written through.
    protocol: &'c dyn ServerHttpProtocol,
    /// The session store the conversation's turns are taken against.
    state: &'c ::engine::ServiceState<SessionStore>,
    /// The session the four `@httpLabel`s of the request address.
    key: SessionKey,
    /// The built bot locale the input is interpreted against.
    model: BotLocaleModel,
    /// Which input events this conversation admits.
    mode: Mode,
    /// The characters of the DTMF entry being keyed in, which is what a turn
    /// interprets once the entry ends.
    dtmf_entry: String,
    /// The response stream written so far, frame by frame.
    frames: Vec<u8>,
}

impl Conversation<'_> {
    /// Apply every event of the request stream, in arrival order, until one of them
    /// ends the conversation or the stream runs out.
    ///
    /// A failure of an EVENT does not fail the request: the response stream is open by
    /// the time the first event is applied, so the failure is framed as the in-band
    /// exception member the response union declares for its shape and the conversation
    /// ends there — which is the channel the model gives it, and which keeps the frames
    /// of the turns already taken (and the session they advanced) reportable. Only a
    /// failure of the FRAMING itself is left to the caller, since a stream that cannot
    /// be written cannot carry its own refusal either.
    fn run(&mut self, events: &[DecodedEvent]) -> Result<(), StartConversationErrorModeled> {
        for (position, event) in events.iter().enumerate() {
            match self.apply_event(position, event) {
                Ok(None) => {}
                Ok(Some(_ended)) => break,
                Err(failure) => {
                    self.frame_in_band(&failure)?;
                    break;
                }
            }
        }
        Ok(())
    }

    /// Apply ONE event, appending whatever the conversation answers it with to the
    /// response stream. `Some(..)` when that event ended the conversation.
    fn apply_event(
        &mut self,
        position: usize,
        event: &DecodedEvent,
    ) -> Result<Option<Ended>, StartConversationErrorModeled> {
        match event.event_type.as_str() {
            MEMBER_CONFIGURATION_EVENT => {
                // "The INITIAL event sent from the application": a configuration
                // anywhere else in the stream would configure a conversation that has
                // already started.
                if position != 0 {
                    return Err(invalid(CONFIGURATION_EVENT_FIRST));
                }
                let configuration: ConfigurationEvent =
                    self.payload(&event.payload, &ConfigurationEvent::SCHEMA)?;
                self.configure(configuration)
            }
            // The configuration is what a conversation starts with, so no other
            // member may stand at the head of the stream.
            _ if position == 0 => Err(invalid(CONFIGURATION_EVENT_FIRST)),
            MEMBER_TEXT_INPUT_EVENT => {
                let input: TextInputEvent = self.payload(&event.payload, &TextInputEvent::SCHEMA)?;
                // One turn of the conversation, the same turn the two `Recognize*`
                // operations take. The utterance is bounded by the model at
                // `len 1..1024`, which the reader above has already applied.
                self.take_turn(&input.text, INPUT_MODE_TEXT)
            }
            MEMBER_DTMF_INPUT_EVENT if self.mode == Mode::Text => {
                Err(refused_in_text_mode(MEMBER_DTMF_INPUT_EVENT))
            }
            MEMBER_DTMF_INPUT_EVENT => {
                let input: DTMFInputEvent = self.payload(&event.payload, &DTMFInputEvent::SCHEMA)?;
                self.key_in_dtmf(input)
            }
            MEMBER_AUDIO_INPUT_EVENT if self.mode == Mode::Text => {
                Err(refused_in_text_mode(MEMBER_AUDIO_INPUT_EVENT))
            }
            MEMBER_AUDIO_INPUT_EVENT => {
                // Read before it is refused, so a chunk that breaks the model's own
                // rules for the event is still the caller's bad request rather than a
                // dependency failure.
                let input: AudioInputEvent =
                    self.payload(&event.payload, &AudioInputEvent::SCHEMA)?;
                // Amazon Lex V2 transcribes the audio of a conversation with its own
                // speech recognition, which is not available here: nothing recognized
                // the chunk, so no transcript, intent or interpretation of it can be
                // reported, and an invented one would be a worse answer than the
                // modelled dependency failure. The same refusal the sibling
                // `RecognizeUtterance` gives audio input.
                self.dependency_failed(format!(
                    "an {MEMBER_AUDIO_INPUT_EVENT} carries {} speech, which this emulator \
                     cannot transcribe; send the input as a {MEMBER_TEXT_INPUT_EVENT}",
                    input.content_type
                ))
            }
            // "Playback of audio is complete and … Amazon Lex V2 should start
            // processing the user's input": no audio response is framed here, so there
            // is no playback to complete and no input waiting on one.
            MEMBER_PLAYBACK_COMPLETION_EVENT => {
                let _: PlaybackCompletionEvent =
                    self.payload(&event.payload, &PlaybackCompletionEvent::SCHEMA)?;
                Ok(None)
            }
            // "A notification from the client that it is disconnecting": the
            // conversation ends here, and the session it held survives for the idle
            // retention every session of this service has.
            MEMBER_DISCONNECTION_EVENT => {
                let _: DisconnectionEvent =
                    self.payload(&event.payload, &DisconnectionEvent::SCHEMA)?;
                Ok(Some(Ended::Disconnected))
            }
            other => Err(invalid(format!(
                "'{other}' is not a member of StartConversationRequestEventStream; send \
                 {MEMBER_CONFIGURATION_EVENT}, {MEMBER_TEXT_INPUT_EVENT}, \
                 {MEMBER_AUDIO_INPUT_EVENT}, {MEMBER_DTMF_INPUT_EVENT}, \
                 {MEMBER_PLAYBACK_COMPLETION_EVENT} or {MEMBER_DISCONNECTION_EVENT} frames"
            ))),
        }
    }

    /// Apply the configuration event to the session the request addresses, and send
    /// the welcome messages the conversation opens with.
    ///
    /// Everything the event configures is STORED, because that is what a following
    /// `GetSession` reports and what the first turn is interpreted against: the
    /// dialog state it seeds is merged exactly as a `sessionState` sent beside an
    /// utterance is, its request attributes accumulate over the conversation, and its
    /// welcome messages become "the messages last sent to the user".
    fn configure(
        &mut self,
        configuration: ConfigurationEvent,
    ) -> Result<Option<Ended>, StartConversationErrorModeled> {
        // "If the value begins with audio/, Amazon Lex V2 returns speech in the
        // response", which it synthesizes with Amazon Polly — a dependency this
        // emulator does not have, and an empty or invented audio stream would be a
        // worse answer than the modelled dependency failure. The same refusal the
        // siblings give an audio `responseContentType`.
        if asks_for_speech(&configuration.response_content_type) {
            return self.dependency_failed(format!(
                "responseContentType {} requests synthesized speech, which this emulator \
                 cannot produce; ask for {TEXT_CONTENT_TYPE}",
                configuration.response_content_type
            ));
        }

        let welcome = configuration.welcome_messages.unwrap_or_default();
        let supplied = configuration.session_state;
        // `disablePlayback` "determines whether Amazon Lex V2 should send audio
        // responses to the client application", and this conversation sends none at
        // all — the one content type it serves is text — so neither value of it
        // changes what the response stream carries. `eventId` and
        // `clientTimestampMillis` are the client's own bookkeeping for the event it
        // sent, which nothing of the conversation is keyed by.
        // "If you set the welcomeMessage field, you must also set the DialogAction
        // structure's type field": a welcome message is sent in a dialog state, so
        // there has to be one to send it in.
        if !welcome.is_empty()
            && supplied
                .as_ref()
                .and_then(|state| state.dialog_action.as_ref())
                .is_none()
        {
            return Err(invalid(WELCOME_MESSAGES_NEED_A_DIALOG_ACTION));
        }
        // A seeded dialog has to be one the addressed bot could be in: checked BEFORE
        // anything is written, so a refused configuration starts no session.
        if let Some(supplied) = supplied.as_ref() {
            self.model
                .check_session_state(supplied)
                .map_err(refused_seed)?;
        }

        let now = SystemTime::now();
        // The whole of the write runs under one hold of the store's guard, so the
        // conversation's own store is locked for a map lookup and a merge and no
        // longer.
        let throttled = {
            let mut store = self.state.lock();
            // Both attribute maps are bounded, and `requestAttributes` may not name
            // the namespace Amazon Lex V2 reserves for itself. They accumulate over
            // the conversation, so each is checked against the merge with what the
            // session already holds.
            check_attribute_bytes(
                "sessionState.sessionAttributes",
                store
                    .session(&self.key, now)
                    .and_then(|session| session.session_state.session_attributes.as_ref()),
                supplied
                    .as_ref()
                    .and_then(|supplied| supplied.session_attributes.as_ref()),
            )
            .map_err(refused_attributes)?;
            check_request_attributes(
                "requestAttributes",
                store
                    .session(&self.key, now)
                    .map(|session| &session.request_attributes),
                configuration.request_attributes.as_ref(),
            )
            .map_err(refused_attributes)?;

            // `session_mut` is the ONE bounded insertion path: it expires idle
            // sessions and refuses to grow the store past its cap.
            match store.session_mut(self.key.clone(), now) {
                Err(full) => Some(full),
                Ok(session) => {
                    if let Some(supplied) = supplied {
                        session.merge_session_state(supplied, now);
                    }
                    if let Some(attributes) = configuration.request_attributes {
                        merge_attributes(&mut session.request_attributes, attributes);
                    }
                    if !welcome.is_empty() {
                        // "A list of messages to send to the user", which is
                        // therefore what the session last sent — the member
                        // `GetSession` reports.
                        session.messages = welcome.clone();
                    }
                    session.updated_at = now;
                    None
                }
            }
        };
        if let Some(full) = throttled {
            return self.throttled(full);
        }

        if welcome.is_empty() {
            return Ok(None);
        }
        self.frame_text_response(welcome)?;
        Ok(None)
    }
    /// Key one DTMF character into the entry being typed, and interpret the entry
    /// once it ends.
    fn key_in_dtmf(
        &mut self,
        input: DTMFInputEvent,
    ) -> Result<Option<Ended>, StartConversationErrorModeled> {
        match input.input_character.as_str() {
            // "Clears the accumulated DTMF digits and immediately ends the input":
            // there is nothing left to interpret, so no turn is taken.
            DTMF_DELETION_CHARACTER => {
                self.dtmf_entry.clear();
                Ok(None)
            }
            // "Immediately ends input": the digits keyed in so far are the input one
            // turn interprets. An end character with nothing before it ends an entry
            // that carries nothing, so there is nothing to interpret.
            DTMF_END_CHARACTER => {
                let entry = std::mem::take(&mut self.dtmf_entry);
                if entry.is_empty() {
                    return Ok(None);
                }
                self.take_turn(&entry, INPUT_MODE_DTMF)
            }
            character => {
                // One entry is bounded like any other utterance of this service:
                // `DTMFSpecification$maxLength` is "the maximum number of DTMF digits
                // allowed in an utterance", whose own `@range` tops out at the same
                // 1024 characters the model bounds a text utterance by.
                if self.dtmf_entry.chars().count() >= MAX_UTTERANCE_CHARS {
                    return Err(invalid(format!(
                        "one DTMF entry carries at most {MAX_UTTERANCE_CHARS} characters, the \
                         most Amazon Lex V2 interprets in one turn; send \
                         {DTMF_END_CHARACTER} to interpret what was keyed in"
                    )));
                }
                self.dtmf_entry.push_str(character);
                Ok(None)
            }
        }
    }

    /// Take ONE turn of the conversation on `input`, and frame what the turn reports:
    /// the state of the conversation after it, then whatever the bot has to say at
    /// the step the dialog reached.
    fn take_turn(
        &mut self,
        input: &str,
        input_mode: &str,
    ) -> Result<Option<Ended>, StartConversationErrorModeled> {
        // The dialog state and request attributes of this conversation were applied
        // to the session by its configuration event, so a turn supplies neither: it
        // is interpreted against the session as it stands, which is what holding a
        // conversation over one stream means.
        let turn = match crate::state::take_turn(
            self.state,
            self.key.clone(),
            SystemTime::now(),
            input,
            &self.model,
            None,
            None,
        ) {
            Ok(turn) => turn,
            // A refusal of the request rate rather than a fault of the request, on a
            // conversation that has already started: reported in band.
            Err(TurnRefused::StoreFull(full)) => return self.throttled(full),
            Err(TurnRefused::Attributes(attributes)) => {
                return Err(refused_attributes(attributes))
            }
        };
        self.frame_intent_result(input_mode, &turn)?;
        // "The event sent from Amazon Lex V2 to your application WITH TEXT to present
        // to the user": a step the bot authors no message for has no text to present,
        // so there is no event to present it in.
        if !turn.messages.is_empty() {
            self.frame_text_response(turn.messages)?;
        }
        Ok(None)
    }

    /// Frame the `IntentResultEvent` one turn reports itself in: every member of it is
    /// read off what the turn left on the session, which is what a following
    /// `GetSession` reports too.
    fn frame_intent_result(
        &mut self,
        input_mode: &str,
        turn: &Turn,
    ) -> Result<(), StartConversationErrorModeled> {
        let event = IntentResultEvent {
            // "Indicates whether the input mode to the operation was text, speech, or
            // from a touch-tone keypad": the mode this turn's own input arrived in.
            input_mode: Some(input_mode.to_string()),
            interpretations: Some(turn.interpretations.clone()),
            session_state: Some(turn.session_state.clone()),
            // The attributes as the conversation has accumulated them: the map is
            // reported on every turn, empty for a conversation nothing has sent an
            // attribute on, so a client can read it without first testing whether the
            // member is there.
            request_attributes: Some(turn.request_attributes.clone()),
            session_id: Some(turn.session_id.clone()),
            // The id of an event is one AWS mints per event of a stream, and nothing
            // of this conversation is addressed by one: no event of the request stream
            // is answered out of order, and no audio playback is interrupted.
            event_id: None,
            // A bot member is recognized only for a bot in a network of bots, which
            // this runtime holds no definition of.
            recognized_bot_member: None,
        };
        self.frame(MEMBER_INTENT_RESULT_EVENT, &event, &IntentResultEvent::SCHEMA)
    }

    /// Frame the `TextResponseEvent` carrying the messages to present to the user.
    fn frame_text_response(
        &mut self,
        messages: Vec<Message>,
    ) -> Result<(), StartConversationErrorModeled> {
        let event = TextResponseEvent {
            messages: Some(messages),
            event_id: None,
        };
        self.frame(MEMBER_TEXT_RESPONSE_EVENT, &event, &TextResponseEvent::SCHEMA)
    }

    /// End the conversation with the in-band `DependencyFailedException`: Amazon Lex
    /// V2 reaches other services for speech — its own speech recognition to
    /// transcribe it and Amazon Polly to synthesize it — and neither is available
    /// here.
    fn dependency_failed(
        &mut self,
        message: String,
    ) -> Result<Option<Ended>, StartConversationErrorModeled> {
        let frame = frame_exception(
            self.protocol,
            MEMBER_DEPENDENCY_FAILED_EXCEPTION,
            &DependencyFailedException { message },
            &SCHEMA_DEPENDENCY_FAILED_EXCEPTION,
        )?;
        self.frames.extend(frame);
        Ok(Some(Ended::InBand))
    }

    /// End the conversation with the in-band `ThrottlingException`: the session store
    /// is at capacity, so this conversation cannot have a session.
    fn throttled(
        &mut self,
        full: SessionStoreFull,
    ) -> Result<Option<Ended>, StartConversationErrorModeled> {
        let frame = frame_exception(
            self.protocol,
            MEMBER_THROTTLING_EXCEPTION,
            &ThrottlingException {
                message: full.message(),
            },
            &SCHEMA_THROTTLING_EXCEPTION,
        )?;
        self.frames.extend(frame);
        Ok(Some(Ended::InBand))
    }

    /// Frame a modelled failure of the conversation as the in-band exception member
    /// the response union declares for its shape.
    ///
    /// Both the member name and the payload are the FAILURE's own — its shape name and
    /// the members the model gives that shape — so a failure travels as the member of
    /// `StartConversationResponseEventStream` that carries its shape, with no second
    /// table mapping one to the other.
    fn frame_in_band(
        &mut self,
        failure: &StartConversationErrorModeled,
    ) -> Result<(), StartConversationErrorModeled> {
        let member = failure.shape_name();
        let frame = frame_event(
            self.protocol,
            member,
            true,
            failure.as_serializable(),
            failure.variant_schema(),
        )
        .map_err(|error| {
            internal(format!(
                "the in-band {member} frame could not be encoded: {error}"
            ))
        })?;
        self.frames.extend(frame);
        Ok(())
    }

    /// Frame one event of the response stream, through the service's own protocol.
    fn frame(
        &mut self,
        event_type: &str,
        event: &dyn SerializableStruct,
        schema: &Schema,
    ) -> Result<(), StartConversationErrorModeled> {
        let frame = frame_event(self.protocol, event_type, false, event, schema).map_err(|error| {
            internal(format!("the {event_type} event could not be framed: {error}"))
        })?;
        self.frames.extend(frame);
        Ok(())
    }

    /// Deserialize one frame's payload into the modelled event structure `T`, then
    /// check it against the model's constraints.
    fn payload<T>(
        &self,
        payload: &[u8],
        schema: &Schema,
    ) -> Result<T, StartConversationErrorModeled>
    where
        T: DeserializableStruct + SerializableStruct,
    {
        event_payload(self.protocol, payload, schema)
    }
}

/// Frame one of the response stream's in-band EXCEPTION members, which travels as
/// `:message-type: exception` rather than as an event.
fn frame_exception(
    protocol: &dyn ServerHttpProtocol,
    event_type: &str,
    event: &dyn SerializableStruct,
    schema: &Schema,
) -> Result<Vec<u8>, StartConversationErrorModeled> {
    frame_event(protocol, event_type, true, event, schema)
        .map_err(|error| internal(format!("the {event_type} frame could not be encoded: {error}")))
}

/// Read the label- and header-bound half of the request into the modelled settings
/// shape and check it against the model.
///
/// Driven through the shared request reader over a copy of the request whose body is
/// EMPTY: every member of the settings shape is bound to a path label or a header
/// (its event-stream payload member is excluded from it), so an empty body is what
/// keeps the JSON codec from being pointed at event-stream frames, and the model's
/// own lengths, patterns and enum are what admit or refuse the request.
fn address_of(
    protocol: &dyn ServerHttpProtocol,
    req: &Request,
) -> Result<StartConversationSettings, StartConversationErrorModeled> {
    let mut address_only = Request::new(SdkBody::empty());
    address_only
        .set_uri(req.uri())
        .map_err(|error| invalid(format!("the request URI could not be read: {error}")))?;
    address_only
        .set_method(req.method())
        .map_err(|error| invalid(format!("the request method could not be read: {error}")))?;
    for (name, value) in req.headers().iter() {
        address_only
            .headers_mut()
            .append(name.to_string(), value.to_string());
    }
    read_request(protocol, &address_only, &StartConversationSettings::SCHEMA).map_err(refused_member)
}

/// Deserialize one frame's payload into the modelled event structure `T`, then check
/// it against the model's constraints.
///
/// Driven through the service's own protocol: the payload is handed to the protocol
/// as a body, and `T`'s schema binds no HTTP member, so the protocol's body codec is
/// what parses it and the model's traits are what refuse it.
fn event_payload<T>(
    protocol: &dyn ServerHttpProtocol,
    payload: &[u8],
    schema: &Schema,
) -> Result<T, StartConversationErrorModeled>
where
    T: DeserializableStruct + SerializableStruct,
{
    let body = Request::new(SdkBody::from(payload.to_vec()));
    read_request(protocol, &body, schema).map_err(|refusal| match refusal {
        RequestRefusal::Invalid(_) => invalid(refusal.stated()),
        RequestRefusal::Unreadable(message) => {
            invalid(format!("an event payload could not be read: {message}"))
        }
    })
}

impl<Eng: Send + Sync + 'static> EventStreamOperation for StartConversation<Eng> {
    fn serve<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, Response> {
        Box::pin(async move {
            let protocol = svc.definition().protocol();
            let response = match self.respond(protocol, req) {
                Ok(response) => response,
                // A refusal of the request is rendered by the framework, in the
                // modelled `message` member, the same way every other operation of
                // this service reports one.
                Err(error) => error_response(protocol, &error),
            };
            // One access record per request, so a refused conversation is
            // diagnosable. No event payload and no credential reaches it.
            tracing::debug!(
                operation = OPERATION,
                service = ctx.identity().signing_name(),
                method = ctx.parts().method.as_str(),
                status = response.status().as_u16(),
                "served a conversation event stream"
            );
            response
        })
    }
}
