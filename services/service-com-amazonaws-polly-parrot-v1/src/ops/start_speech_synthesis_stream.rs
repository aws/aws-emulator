// SPDX-License-Identifier: CC0-1.0

//! @op StartSpeechSynthesisStream
//!
//! `POST /v1/synthesisStream -> 200`. Opens a bidirectional synthesis stream:
//! reads the synthesis parameters off the request headers, consumes the
//! `TextEvent` / `CloseStreamEvent` action events the caller frames into the
//! request body, and answers with the modelled `AudioEvent` frames for the text it
//! was sent followed by the `StreamClosedEvent` that closes the stream.
//!
//! ## How the speech is produced
//!
//! Each `AudioEvent` carries speech RENDERED from the text it covers, by the same
//! synthesiser [`super::synthesize_speech`] uses and from the same parts: the text is
//! read into phonemes, the lexicons the request names and the SSML each `TextEvent`
//! carries are applied, and the result is produced by a formant synthesiser and encoded
//! in the format the request asked for. So one stream's audio for a text is the audio
//! the single-shot operation answers for that same text, and nothing is echoed back.
//!
//! Everything around the synthesis is computed from the request and from state, and
//! is the same rule the single-shot sibling applies, so the two operations cannot
//! disagree:
//!
//! * `Engine` admits only the engine the model documents for this operation —
//!   "currently, only the `generative` engine is supported"
//!   ([`STREAMING_ENGINE`]) — and the voice the request names must support it, as
//!   the voice catalog `DescribeVoices` serves records it;
//! * `OutputFormat` admits the AUDIO formats only, because "Amazon Polly does not
//!   support JSON speech marks" on this operation, and this stream carries audio
//!   chunks;
//! * `SampleRate` is accepted only when it is one of the spellings the chosen
//!   format enumerates ([`super::synthesize_speech::valid_sample_rates`]), and
//!   defaults per format and engine the same way;
//! * every name in `LexiconNames` is resolved in the lexicon store of the Region
//!   the request was signed for, and its lexemes are applied to the text that is
//!   spoken;
//! * `LanguageCode` selects which language a bilingual voice speaks and states no
//!   requirement on the voice, so a language the chosen voice does not speak is
//!   streamed with that voice rather than refused;
//! * each `TextEvent` is billed the way Amazon Polly bills characters — every
//!   character of plain text, and for `ssml` the text content only — and `ssml`
//!   text is read as a real XML document and checked against the SSML vocabulary
//!   Amazon Polly publishes, so a document that is not well-formed, or markup
//!   outside that vocabulary, is refused rather than synthesised;
//! * `StreamClosedEvent$RequestCharacters` is "the total number of characters
//!   synthesized during the streaming session" — the sum of those billed counts.
//!
//! `mp3` is the one format whose chunks carry silence rather than the rendered
//! waveform, for the encoder-licence reason [`super::synthesize_speech::legacy_audio`]
//! states; its chunks still last as long as the speech does.
//!
//! ## Where the audio events fall
//!
//! `TextEvent$FlushStreamConfiguration` decides that: it "specifies whether to
//! force the synthesis engine to immediately write buffered audio data to the
//! output stream". So the billed characters of each `TextEvent` accumulate, and the
//! accumulated audio is written as ONE `AudioEvent` at each flush point — a
//! `TextEvent` whose `FlushStreamConfiguration$Force` is set, and the end of the
//! input stream, where `CloseStreamEvent` says "all audio will be returned". A
//! stream that forces no flush therefore answers with the audio of its whole text
//! in one chunk, which is the audio `SynthesizeSpeech` answers for that same text.
//!
//! ## Why this operation is hand-authored
//!
//! Its input carries a `@streaming` event-stream member and its output is a
//! `@streaming` union. The op-stub generator declines the first
//! (`src/ops/SKIPPED_STUBS.md`) and the generated `ErasedOperation` shim declines
//! the second, because `serialize_as_event_stream<P>` needs a `Sized` protocol that
//! an object-safe `run` cannot hand it. The framing, the request reader, the
//! response and the routing seam are the shared ones in
//! `service_common::event_stream`; `src/service.rs` routes a request here through
//! `Service::override_operation`, which the router consults BEFORE the generated
//! routing table. Two things are local to this file, both because the shared module
//! has nothing for them. The ONE frame written here is the `AudioEvent`:
//! `AudioChunk` is that event's `@eventPayload`, so the frame payload is the raw
//! chunk rather than a JSON object carrying a member, which the protocol's generic
//! event serializer cannot express. And the two READERS a streaming input needs —
//! the header-bound half of the request, and one frame's payload — are driven
//! through the shared `deserialize_request` here, the same way
//! `service-com-amazonaws-transcribestreaming-transcribe` drives them in its own
//! crate; the generic home for both is that shared module, whose header already
//! books that migration together with the Transcribe Streaming copies.
//!
//! ## Input (every member an `@httpHeader` except the payload)
//!
//! * `Engine` (Engine: generative | long-form | neural | standard) **required**
//! * `OutputFormat` (OutputFormat: json | mp3 | ogg_opus | ogg_vorbis | pcm |
//!   mulaw | alaw) **required**
//! * `VoiceId` (VoiceId) **required**
//! * `LanguageCode` (LanguageCode), `LexiconNames` (list, len 0..5), `SampleRate`
//!   (string)
//! * `ActionStream` (`@httpPayload`, `@streaming` union of `TextEvent`,
//!   `CloseStreamEvent`)
//!
//! ## Output
//!
//! * `EventStream` (`@httpPayload`, `@streaming` union of `AudioEvent`,
//!   `StreamClosedEvent`, and the four errors below as in-band exceptions) — the
//!   response body, HTTP 200.
//!
//! ## Errors
//!
//! * `ValidationException` (HTTP 400) — "the input fails to satisfy the constraints
//!   specified by the service", with the `reason` naming the class:
//!   `fieldValidationFailed` for a header the rules above refuse,
//!   `invalidInboundEvent` for an action event the input union does not admit or
//!   that is not valid, and `other` for a stream past this emulator's capacity.
//! * `ServiceFailureException` (HTTP 500) — this service's own state is unreachable.
//! * `ThrottlingException` (HTTP 400) and `ServiceQuotaExceededException` (HTTP 402)
//!   are modelled and declared, and nothing here constructs either: see the note on
//!   each shape in `super::types`.

#[allow(unused_imports)]
use super::types::*;
// This operation's own shapes are HAND-WRITTEN (the generator declines a `@streaming`
// event-stream operation), so they are reached through the module that declares them
// rather than through the glob above, which carries only what the model renders.
#[allow(unused_imports)]
use crate::service::types_local::*;
use super::synthesize_speech::{
    audio, billed_characters, default_sample_rate, lexicon, plan, render, resolve_sample_rate,
    valid_sample_rates, DEFAULT_TEXT_TYPE, SYNTHESIS_TIME_BUDGET,
};
use aws_smithy_eventstream::frame::{read_message_from, write_message_to};
use aws_smithy_runtime_api::http::{Request, Response};
use aws_smithy_types::body::SdkBody;
use aws_smithy_types::event_stream::{Header, HeaderValue, Message};
use bytes::Buf;
use service_common::event_stream::{
    deserialize_request, error_response, event_stream_only_response, frame_event, read_request,
    RequestRefusal,
};
use smithy_server_runtime::{BoxFuture, OperationContext, ServerHttpProtocol, Service};
use std::sync::Arc;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-polly-parrot-v1.json");

/// The URI the model binds this operation to
/// (smithy_query(query="operation", operation="StartSpeechSynthesisStream"):
/// `POST /v1/synthesisStream`). `src/service.rs` routes that method and path here.
pub const MODELED_PATH: &str = "/v1/synthesisStream";

/// The one engine this operation accepts.
///
/// `StartSpeechSynthesisStreamInput$Engine`: "Currently, only the
/// `generative` engine is supported. If you specify a voice that the selected
/// engine doesn't support, Amazon Polly returns an error."
pub const STREAMING_ENGINE: &str = "generative";

/// The most action events one stream may carry.
///
/// A caller chooses both how many frames a body holds and how small each one is, so
/// decoding them is work a caller sizes: a body the transport accepts still holds
/// hundreds of thousands of tiny frames, and each one becomes a decoded event and a
/// flush point. The count is refused before any of that work is done.
pub const MAX_ACTION_EVENTS: usize = 1024;

/// The most characters one stream synthesizes here.
///
/// CHOSEN, not an Amazon Polly quota: a stream is open-ended and the real service
/// publishes no total for it, while this emulator buffers the whole answer before
/// sending it, and every billed character becomes audio bytes
/// ([`super::synthesize_speech::CHARACTERS_PER_SECOND`]). The bound is the total
/// `SynthesizeSpeech` documents for ONE request, so a stream's buffered answer is
/// the same order of size as that operation's and the two cost the same memory.
pub const MAX_STREAM_CHARACTERS: usize = super::synthesize_speech::MAX_TOTAL_CHARACTERS;

/// The most characters ONE `TextEvent` carries.
///
/// The same bound one `SynthesizeSpeech` request's `Text` has, applied per event and to
/// the RAW text: the billed count excludes SSML markup, so this is what keeps a single
/// event from carrying unbounded markup. It is checked before the event is planned.
pub const MAX_EVENT_CHARACTERS: usize = super::synthesize_speech::MAX_TOTAL_CHARACTERS;

/// How many action events are handled between yields to the runtime.
///
/// Decoding frames and encoding audio is synchronous work inside an async future,
/// and a future with no await point cannot be cancelled or timed out part-way;
/// yielding every so often gives a cancellation somewhere to land.
const EVENTS_PER_YIELD: usize = 64;

/// Event-stream prelude headers.
const FRAME_HEADER_MESSAGE_TYPE: &str = ":message-type";
const FRAME_HEADER_EVENT_TYPE: &str = ":event-type";
const FRAME_HEADER_CONTENT_TYPE: &str = ":content-type";
/// The `:message-type` of a union member; a client sends only these.
const FRAME_MESSAGE_TYPE_EVENT: &str = "event";
/// The `:content-type` an `@eventPayload` blob frame declares.
const FRAME_CONTENT_TYPE_BINARY: &str = "application/octet-stream";

/// The members of `StartSpeechSynthesisStreamActionStream` (the input union).
const MEMBER_TEXT_EVENT: &str = "TextEvent";
const MEMBER_CLOSE_STREAM_EVENT: &str = "CloseStreamEvent";
/// The two members of `StartSpeechSynthesisStreamEventStream` this stream carries.
const MEMBER_AUDIO_EVENT: &str = "AudioEvent";
const MEMBER_STREAM_CLOSED_EVENT: &str = "StreamClosedEvent";

/// The `ValidationExceptionReason` values this operation answers with.
const REASON_FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";
const REASON_INVALID_INBOUND_EVENT: &str = "invalidInboundEvent";
const REASON_OTHER: &str = "other";

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which the `smithy_model!` line above is what emits into this
// module.
//
// The operation's MODELED errors: the macro accepts exactly the variants the model
// declares for the operation, so all four are here even though only two are ever
// constructed (`super::types` records why for the other two).
#[smithy_macros::operation_error("com.amazonaws.polly#StartSpeechSynthesisStream")]
#[derive(Debug)]
pub enum StartSpeechSynthesisStreamErrorModeled {
    // The two errors the model added for CUSTOM voices. Declared because the operation
    // lists them, and constructed by nothing here: `VoiceId` targets the model's
    // closed `VoiceId` enum, so a `vc-` id — the id `CreateVoice` mints — is refused by
    // the request boundary before this handler runs, and no stream of this emulator can
    // name a custom voice that is missing (`ResourceNotFoundException`) or not yet
    // active (`ConflictException`).
    ConflictException(ConflictException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceFailureException(ServiceFailureException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel. The wrapper is what lets the one format this
/// emulator cannot encode answer the scaffold's 501 rather than a success carrying
/// a stream no decoder accepts.
pub type StartSpeechSynthesisStreamError =
    crate::scaffolded::Scaffolded<StartSpeechSynthesisStreamErrorModeled>;

/// Lets a modelled failure lift into the error channel with `.into()`.
impl From<StartSpeechSynthesisStreamErrorModeled> for StartSpeechSynthesisStreamError {
    fn from(modeled: StartSpeechSynthesisStreamErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// Refuse one header-bound member, the way the service's own request-validation
/// envelope words a refusal: the member's path, the value that failed and the
/// constraint it failed.
///
/// `name` is the member's path in the envelope's own lowerCamel convention, and
/// `constraint` states what the member must satisfy; both also travel in the
/// modelled `fields` list, which is where a client reads WHICH member to fix. The
/// sentence itself comes from the one place this service words a single-member
/// refusal ([`crate::service::validation::one_member_refusal`]).
fn field_refusal(name: &str, value: &str, constraint: &str) -> StartSpeechSynthesisStreamError {
    StartSpeechSynthesisStreamErrorModeled::ValidationException(ValidationException {
        message: crate::service::validation::one_member_refusal(name, value, constraint),
        reason: REASON_FIELD_VALIDATION_FAILED.to_string(),
        fields: Some(vec![ValidationExceptionField {
            name: name.to_string(),
            message: constraint.to_string(),
        }]),
    })
    .into()
}

/// Refuse an action event the input stream may not carry: one the union does not
/// admit, one that is not a readable event, or one that arrives after the stream
/// was closed.
fn inbound_event_refusal(message: impl Into<String>) -> StartSpeechSynthesisStreamError {
    StartSpeechSynthesisStreamErrorModeled::ValidationException(ValidationException {
        message: message.into(),
        reason: REASON_INVALID_INBOUND_EVENT.to_string(),
        fields: None,
    })
    .into()
}

/// Refuse a stream past this emulator's own capacity — neither a field of the
/// request nor a malformed event, so the reason is the model's catch-all.
fn capacity_refusal(message: impl Into<String>) -> StartSpeechSynthesisStreamError {
    StartSpeechSynthesisStreamErrorModeled::ValidationException(ValidationException {
        message: message.into(),
        reason: REASON_OTHER.to_string(),
        fields: None,
    })
    .into()
}

/// This service's own state could not be reached.
fn service_failure() -> StartSpeechSynthesisStreamError {
    StartSpeechSynthesisStreamErrorModeled::ServiceFailureException(
        crate::service::errors::service_failure(),
    )
    .into()
}

/// The `OutputFormat` values this operation accepts: every value of the model's
/// enum except the speech-mark one, which this operation does not support.
fn audio_output_formats() -> Vec<&'static str> {
    OUTPUT_FORMAT_VALUES
        .iter()
        .copied()
        .filter(|format| *format != SPEECH_MARK_OUTPUT_FORMAT)
        .collect()
}

/// The `OutputFormat` that carries speech marks rather than audio.
const SPEECH_MARK_OUTPUT_FORMAT: &str = "json";

/// One decoded member of `StartSpeechSynthesisStreamActionStream`.
#[derive(Debug, Clone)]
enum ActionEvent {
    /// Text to synthesize, as the caller framed it.
    Text(TextEvent),
    /// The end of the input stream.
    CloseStream,
}

/// The `StartSpeechSynthesisStream` operation.
///
/// Owns the shared engine (the constructor seam). The generated routing table holds
/// one of these — where its `ErasedOperation` shim declines a `@streaming`-output
/// operation — and `service.rs` holds another wrapped in an `EventStreamRoute`;
/// both reach the same lexicons and the same voice catalog, because that state
/// lives in the shared engine rather than on the operation.
pub struct StartSpeechSynthesisStream<Eng> {
    engine: Arc<Eng>,
}

impl<Eng> StartSpeechSynthesisStream<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: Send + Sync + 'static> StartSpeechSynthesisStream<Eng> {
    /// The whole request: settle the synthesis parameters, consume the action
    /// events, and frame the audio and the closing event they produce.
    async fn respond(
        &self,
        protocol: &dyn ServerHttpProtocol,
        req: &Request,
        ctx: &OperationContext<'_>,
    ) -> Result<Response, StartSpeechSynthesisStreamError> {
        let settings = settings_from_headers(protocol, req).map_err(|message| {
            // The header half is read and checked by the MODEL (its enums, its list
            // length), so this sentence is the framework's own envelope, naming the
            // member that failed as that boundary names it; it carries no `fields`
            // list because the boundary reports the member in the sentence rather
            // than as a member path this code could lift out of it.
            StartSpeechSynthesisStreamErrorModeled::ValidationException(ValidationException {
                message,
                reason: REASON_FIELD_VALIDATION_FAILED.to_string(),
                fields: None,
            })
        })?;

        let output_format = settings.output_format.as_str();
        if settings.engine != STREAMING_ENGINE {
            return Err(field_refusal(
                "engine",
                &settings.engine,
                &format!("Member must satisfy enum value set [{STREAMING_ENGINE}]"),
            ));
        }
        if output_format == SPEECH_MARK_OUTPUT_FORMAT {
            // "Currently, Amazon Polly does not support JSON speech marks"
            // (`StartSpeechSynthesisStreamInput$OutputFormat`), and this stream
            // carries audio chunks, so the accepted set is the audio formats.
            return Err(field_refusal(
                "outputFormat",
                output_format,
                &format!(
                    "Member must satisfy enum value set [{}]",
                    audio_output_formats().join(", ")
                ),
            ));
        }
        let sample_rate = match settings.sample_rate.as_deref() {
            Some(requested) => resolve_sample_rate(output_format, requested).ok_or_else(|| {
                field_refusal(
                    "sampleRate",
                    requested,
                    &format!(
                        "Member must satisfy enum value set [{}]",
                        valid_sample_rates(output_format).join(", ")
                    ),
                )
            })?,
            None => default_sample_rate(output_format, &settings.engine),
        };

        // The two state reads: the lexicons this request names must be stored in the
        // Region it was signed for, and the voice it names decides which engine it may
        // ask for — read off the catalog `DescribeVoices` serves, so the two
        // operations cannot disagree about a voice.
        let state = crate::state::service_state(&*self.engine).ok_or_else(service_failure)?;
        let mut lexicons = lexicon::Lexicons::default();
        if let Some(names) = settings.lexicon_names.as_ref() {
            let region = crate::state::request_region(ctx.parts());
            let guard = state.lock();
            let mut missing: Option<String> = None;
            for name in names {
                match guard.lexicon(&region, name) {
                    Some(stored) => lexicons.add(&stored.content),
                    None => {
                        missing = Some(name.clone());
                        break;
                    }
                }
            }
            drop(guard);
            if let Some(missing) = missing {
                return Err(field_refusal(
                    "lexiconNames",
                    &missing,
                    "Member must name a lexicon stored in this Region",
                ));
            }
        }
        // The voice catalog is the set of voices the service OFFERS, and the model's
        // `VoiceId` enum — which the boundary has already narrowed this value to — is
        // wider than it, so the lookup resolves a voice the catalog does not list as
        // well (`crate::state::UNLISTED_VOICES`). It still FAILS CLOSED: a value the
        // enum declares and no table knows would be this emulator's own fault, and is
        // reported as one rather than serving a stream whose voice nothing checked.
        // `SynthesizeSpeech` reads the voice the same way, so the two cannot disagree
        // about one.
        let voice = {
            let guard = state.lock();
            guard.voice(&settings.voice_id).copied()
        };
        let voice = voice.ok_or_else(service_failure)?;
        if !voice.supports_engine(STREAMING_ENGINE) {
            return Err(field_refusal(
                "voiceId",
                &settings.voice_id,
                &format!("Member must be a voice that supports the {STREAMING_ENGINE} engine"),
            ));
        }
        // "Specify this parameter only when using a bilingual voice. If a bilingual
        // voice is used and no language code is specified, Amazon Polly uses the
        // default language of the bilingual voice"
        // (`StartSpeechSynthesisStreamInput$LanguageCode`): the member SELECTS a
        // bilingual voice's language and states no requirement on the voice, so a
        // language the chosen voice does not speak is not a refusal here either — the
        // same reading `SynthesizeSpeech` takes of its own copy of the member.
        let events = decode_action_stream(protocol, req).await?;
        // The voice the request named decides the timbre of the speech, read off the
        // same catalog row its engine was checked against.
        let timbre = render::Timbre::for_gender(Some(voice.gender));
        frame_stream(
            protocol,
            output_format,
            sample_rate,
            timbre,
            &lexicons,
            &events,
        )
        .await
    }
}

/// Consume the action events in arrival order and frame what they produce: one
/// `AudioEvent` per flush point, then the `StreamClosedEvent` carrying the
/// characters the session synthesized.
async fn frame_stream(
    protocol: &dyn ServerHttpProtocol,
    output_format: &str,
    sample_rate: u32,
    timbre: render::Timbre,
    lexicons: &lexicon::Lexicons,
    events: &[ActionEvent],
) -> Result<Response, StartSpeechSynthesisStreamError> {
    let mut frames: Vec<u8> = Vec::new();
    // The text buffered since the last flush, each piece with the `TextType` it came
    // in, and the characters billed over the whole session.
    let mut buffered: Vec<(String, String)> = Vec::new();
    let mut synthesized = 0usize;
    // How many samples the session has produced, which is what the audio bound is on.
    let mut rendered = 0usize;
    for (index, event) in events.iter().enumerate() {
        if index % EVENTS_PER_YIELD == EVENTS_PER_YIELD - 1 {
            tokio::task::yield_now().await;
        }
        let text_event = match event {
            ActionEvent::Text(text_event) => text_event,
            // "After sending this event, the input stream will be closed and all
            // audio will be returned" — so nothing after it is consumed, and the
            // flush below is what returns that audio.
            ActionEvent::CloseStream => break,
        };
        let text_type = text_event.text_type.as_deref().unwrap_or(DEFAULT_TEXT_TYPE);
        // One event's text is bounded the way one `SynthesizeSpeech` request's is, BEFORE
        // anything is planned from it: the billed count below excludes SSML markup, so
        // the raw length is what bounds the markup a single event can carry.
        if text_event.text.chars().count() > MAX_EVENT_CHARACTERS {
            return Err(capacity_refusal(format!(
                "a TextEvent carries more than the {MAX_EVENT_CHARACTERS} characters this \
                 emulator synthesizes in one event"
            )));
        }
        // `ssml` text is read as a real XML document against the published SSML
        // vocabulary, and what is billed is its text content; plain text bills every
        // character. The same rule, and the same reader, `SynthesizeSpeech` applies.
        let billed = billed_characters(&text_event.text, text_type)
            .map_err(|_| inbound_event_refusal(crate::service::errors::INVALID_SSML.to_string()))?;
        synthesized = synthesized.saturating_add(billed);
        if synthesized > MAX_STREAM_CHARACTERS {
            // One of the two bounds on the work a session can ask for. It bounds the
            // TEXT; because SSML markup is not billed, how long that text takes to speak
            // is bounded separately, by the sample budget handed to each flush below.
            return Err(capacity_refusal(format!(
                "the input stream carries more than the {MAX_STREAM_CHARACTERS} characters this \
                 emulator synthesizes in one stream"
            )));
        }
        // A `TextEvent` carrying no text buffers nothing: there is no audio to return
        // for text that was never sent.
        if !text_event.text.is_empty() {
            buffered.push((text_event.text.clone(), text_type.to_string()));
        }
        let forced = text_event
            .flush_stream_configuration
            .as_ref()
            .is_some_and(|flush| flush.force);
        if forced {
            rendered = rendered.saturating_add(
                flush(
                    &mut frames,
                    output_format,
                    sample_rate,
                    timbre,
                    lexicons,
                    &mut buffered,
                    max_samples(sample_rate).saturating_sub(rendered),
                )
                .await?,
            );
        }
    }
    // The input stream ends with the request body, so whatever is still buffered is
    // written out before the stream closes.
    flush(
        &mut frames,
        output_format,
        sample_rate,
        timbre,
        lexicons,
        &mut buffered,
        max_samples(sample_rate).saturating_sub(rendered),
    )
    .await?;

    let closed = StreamClosedEvent {
        request_characters: i32::try_from(synthesized).unwrap_or(i32::MAX),
    };
    frames.extend_from_slice(
        &frame_event(
            protocol,
            MEMBER_STREAM_CLOSED_EVENT,
            false,
            &closed,
            &StreamClosedEvent::SCHEMA,
        )
        .map_err(|_| service_failure())?,
    );
    // `StartSpeechSynthesisStreamOutput` binds no header: the stream IS the response.
    Ok(event_stream_only_response(frames))
}

/// Write the audio of the buffered text as one `AudioEvent`, and empty the buffer. A
/// flush with nothing buffered writes no frame: there is no audio to return for text
/// that was never sent. Answers how many samples it produced.
///
/// The text buffered between two flush points is spoken as ONE utterance, so a stream
/// that forces no flush answers the audio `SynthesizeSpeech` answers for its whole text.
/// A run of pieces that arrived with different `TextType`s cannot be one document, so
/// each run of one type is spoken on its own and the audio is joined.
///
/// `budget` is how many samples the session may still produce. It is consulted BEFORE
/// each run is planned, so no buffer here grows past what the session is allowed; a run
/// that would pass it is the capacity refusal rather than an allocation. Planning,
/// rendering AND encoding are synchronous CPU-bound work with no await of their own, so
/// all three go to the BLOCKING pool in ONE task and the worker serving this request is
/// released while it runs: `spawn_blocking(..).await` is a real await point, so the health
/// endpoint, a cancellation and a graceful shutdown keep working, and the wait is bounded
/// by [`SYNTHESIS_TIME_BUDGET`].
#[allow(clippy::too_many_arguments)]
async fn flush(
    frames: &mut Vec<u8>,
    output_format: &str,
    sample_rate: u32,
    timbre: render::Timbre,
    lexicons: &lexicon::Lexicons,
    buffered: &mut Vec<(String, String)>,
    budget: usize,
) -> Result<usize, StartSpeechSynthesisStreamError> {
    let pieces = std::mem::take(buffered);
    if pieces.is_empty() {
        return Ok(0);
    }
    // What one run of a single `TextType` is: its text, joined.
    let mut runs: Vec<(String, String)> = Vec::new();
    let mut index = 0usize;
    while index < pieces.len() {
        let text_type = pieces[index].1.clone();
        let mut joined = String::new();
        while index < pieces.len() && pieces[index].1 == text_type {
            joined.push_str(&pieces[index].0);
            index += 1;
        }
        runs.push((joined, text_type));
    }

    let owned_lexicons = lexicons.clone();
    let owned_format = output_format.to_string();
    let produced = match ::tokio::time::timeout(
        SYNTHESIS_TIME_BUDGET,
        ::tokio::task::spawn_blocking(move || {
            chunk_of_runs(&runs, &owned_format, sample_rate, timbre, &owned_lexicons, budget)
        }),
    )
    .await
    {
        Ok(Ok(Ok(produced))) => produced,
        Ok(Ok(Err(ChunkFailure::Capacity))) => {
            return Err(capacity_refusal(format!(
                "the input stream asks for more than the {MAX_STREAM_SECONDS} seconds of audio \
                 this emulator synthesizes in one stream"
            )))
        }
        // The document was read when the event arrived, so a failure here is the same one
        // that reader reports.
        Ok(Ok(Err(ChunkFailure::InvalidSsml))) => {
            return Err(inbound_event_refusal(
                crate::service::errors::INVALID_SSML.to_string(),
            ))
        }
        // The encoder refused a rate this format documents, the synthesis did not finish
        // inside the budget, or it did not run at all: nothing was synthesized, so this is
        // a server fault rather than a chunk that stands for no speech.
        Ok(Ok(Err(ChunkFailure::Encoding))) | Ok(Err(_)) | Err(_) => {
            return Err(service_failure())
        }
    };
    let Some((chunk, samples)) = produced else {
        return Ok(0);
    };
    // `AudioEvent$AudioChunk` is the event's `@eventPayload`, so the frame payload
    // IS the chunk — which is why this one frame is written here rather than through
    // the protocol's generic event serializer, whose JSON object would carry the
    // chunk as a member instead.
    let message = Message::new(chunk)
        .add_header(Header::new(
            FRAME_HEADER_MESSAGE_TYPE,
            HeaderValue::String(FRAME_MESSAGE_TYPE_EVENT.into()),
        ))
        .add_header(Header::new(
            FRAME_HEADER_EVENT_TYPE,
            HeaderValue::String(MEMBER_AUDIO_EVENT.into()),
        ))
        .add_header(Header::new(
            FRAME_HEADER_CONTENT_TYPE,
            HeaderValue::String(FRAME_CONTENT_TYPE_BINARY.into()),
        ));
    write_message_to(&message, frames).map_err(|_| service_failure())?;
    Ok(samples)
}

/// Why one flush produced no chunk.
enum ChunkFailure {
    /// The session cannot carry the next run's audio.
    Capacity,
    /// A document could not be read as XML at all.
    InvalidSsml,
    /// The encoder refused the stream.
    Encoding,
}

/// The one `AudioEvent` chunk of `runs`, and how many samples it carries, or `None` when
/// they say nothing.
///
/// Synchronous and CPU-bound from end to end — planning, rendering and encoding — so the
/// caller runs it on the blocking pool. `budget` is checked before each run is planned, so
/// the sample buffer is bounded before it is filled and `render::max_samples` bounds each
/// run's contribution to it.
fn chunk_of_runs(
    runs: &[(String, String)],
    output_format: &str,
    sample_rate: u32,
    timbre: render::Timbre,
    lexicons: &lexicon::Lexicons,
    budget: usize,
) -> Result<Option<(Vec<u8>, usize)>, ChunkFailure> {
    let mut samples: Vec<f32> = Vec::new();
    for (joined, text_type) in runs {
        // The next run cannot be planned, let alone rendered, unless the session can
        // still carry a whole utterance of it: `render::max_samples` is the most one
        // utterance produces, so this is what makes the bound dominate every allocation
        // below rather than follow it.
        if samples
            .len()
            .saturating_add(render::max_samples(sample_rate))
            > budget
        {
            return Err(ChunkFailure::Capacity);
        }
        let utterance =
            plan::plan(joined, text_type, lexicons).map_err(|_| ChunkFailure::InvalidSsml)?;
        samples.extend(render::render(&utterance, sample_rate, timbre));
    }
    if samples.is_empty() {
        return Ok(None);
    }
    let chunk =
        encoded_chunk(output_format, sample_rate, &samples).ok_or(ChunkFailure::Encoding)?;
    Ok(Some((chunk, samples.len())))
}

/// The most seconds of audio one stream produces.
///
/// CHOSEN, and it is what bounds the SIZE of the answer: the character bound above limits
/// the text, but SSML markup is not billed and stretches how long that text takes to
/// speak (a slow `<prosody rate>` multiplies every segment and each `<break>` adds up to
/// ten seconds), so the audio a session carries is bounded here as well. It is several
/// times the bound one utterance has ([`plan::MAX_UTTERANCE_MS`]), so a stream still
/// carries the speech of its whole text while a session that asks for hours of pauses is
/// refused before the samples are produced — and small enough that the samples one flush
/// holds stay in the tens of megabytes at the highest rate any format documents.
pub const MAX_STREAM_SECONDS: u64 = 600;

/// The sample count [`MAX_STREAM_SECONDS`] is at `sample_rate`.
fn max_samples(sample_rate: u32) -> usize {
    usize::try_from(MAX_STREAM_SECONDS.saturating_mul(u64::from(sample_rate))).unwrap_or(usize::MAX)
}

/// The audio of the rendered samples, in the encoding the request settled.
///
/// Every encoder is the one `SynthesizeSpeech` answers that format with, so one
/// request's audio and one stream's audio for the same text are the same bytes. `None`
/// when the encoder refuses the stream, which the caller answers with the modelled server
/// error: the rate was settled against the format's own documented value set before the
/// stream was accepted, and every one of those values is a rate these encoders carry, so
/// that is this emulator failing to encode a rate it admitted — never a second refusal of
/// a member the request was already told was acceptable.
fn encoded_chunk(output_format: &str, sample_rate: u32, samples: &[f32]) -> Option<Vec<u8>> {
    match output_format {
        "pcm" => Some(audio::pcm(samples)),
        "mulaw" => Some(audio::mulaw(samples)),
        "alaw" => Some(audio::alaw(samples)),
        "ogg_vorbis" => audio::ogg_vorbis(samples, sample_rate),
        // The two formats whose samples are not written: see
        // [`super::synthesize_speech::legacy_audio`]. Their chunks still last as long as
        // the speech they stand for.
        "ogg_opus" => audio::ogg_opus_silence_for(samples, sample_rate),
        "mp3" => audio::mp3_silence_for(samples, sample_rate),
        // `respond` has already narrowed `OutputFormat` to the formats this operation
        // serves, so reaching here would be this emulator failing to encode a format it
        // admitted.
        _ => None,
    }
}

/// Read the `@httpHeader`-bound half of the request into the settings shape and
/// check it against the model.
///
/// Driven through the shared request reader over a copy of the request whose body is
/// EMPTY: every member of the settings shape is header-bound (its event-stream
/// payload member is excluded from it), so an empty body is what keeps the body
/// codec from being pointed at event-stream frames, and the model's own enums and
/// bounds are what admit or refuse the request. `Err` carries the refusal sentence
/// the framework's own boundary would have worded, built from the VIOLATIONS the shared
/// reader hands back ([`service_common::event_stream::read_request`]) through this
/// service's own statement of them ([`crate::service::validation::stated_envelope`]) — so
/// a header-bound member and a body member of another operation state one enum's accepted
/// values alike.
fn settings_from_headers(
    protocol: &dyn ServerHttpProtocol,
    req: &Request,
) -> Result<StartSpeechSynthesisStreamSettings, String> {
    let mut headers_only = Request::new(SdkBody::empty());
    headers_only
        .set_uri(req.uri())
        .map_err(|error| format!("the request URI could not be read: {error}"))?;
    headers_only
        .set_method(req.method())
        .map_err(|error| format!("the request method could not be read: {error}"))?;
    for (name, value) in req.headers().iter() {
        headers_only
            .headers_mut()
            .append(name.to_string(), value.to_string());
    }
    read_request(
        protocol,
        &headers_only,
        &StartSpeechSynthesisStreamSettings::SCHEMA,
    )
    .map_err(|refusal| match refusal {
        // The violations are stated the way this service states them, in the framework's
        // own envelope — the one path the refusal seam takes for every other operation.
        RequestRefusal::Invalid(violations) => {
            crate::service::validation::stated_envelope(&violations)
        }
        // A request the codec could not read at all has no violation to state.
        RequestRefusal::Unreadable(message) => message,
    })
}

/// Read ONE decoded frame's payload into the modelled event structure and check it
/// against the model.
///
/// The payload is handed to the shared request reader as a body, and an event shape
/// binds no HTTP member, so the protocol's body codec is what parses it. An event
/// whose structure carries an `@eventPayload` member is not read this way — there
/// the frame payload IS that member's raw value.
fn event_payload<T>(
    protocol: &dyn ServerHttpProtocol,
    payload: &[u8],
    schema: &aws_smithy_schema::Schema,
) -> Result<T, String>
where
    T: aws_smithy_schema_server::DeserializableStruct + aws_smithy_schema::serde::SerializableStruct,
{
    let body = Request::new(SdkBody::from(payload.to_vec()));
    deserialize_request(protocol, &body, schema)
}

/// Split the request body into the action events it frames, in arrival order.
///
/// Each frame names its union member in the `:event-type` header the AWS SDKs set;
/// a `TextEvent`'s payload is the event structure in the protocol's own body
/// format, which is what reads it back into the modelled shape. An EMPTY body is a
/// stream that carried no action event, which is not a refusal: `ActionStream` is
/// not a required member.
async fn decode_action_stream(
    protocol: &dyn ServerHttpProtocol,
    req: &Request,
) -> Result<Vec<ActionEvent>, StartSpeechSynthesisStreamError> {
    let body = req
        .body()
        .bytes()
        .ok_or_else(|| inbound_event_refusal("the request body is not available as bytes"))?;
    let mut buffer = bytes::Bytes::copy_from_slice(body);
    let mut events: Vec<ActionEvent> = Vec::new();
    let mut closed = false;
    while buffer.has_remaining() {
        if events.len() % EVENTS_PER_YIELD == EVENTS_PER_YIELD - 1 {
            tokio::task::yield_now().await;
        }
        if events.len() >= MAX_ACTION_EVENTS {
            return Err(capacity_refusal(format!(
                "the input stream carries more than the {MAX_ACTION_EVENTS} events this emulator \
                 accepts in one request"
            )));
        }
        let before = buffer.remaining();
        let message = read_message_from(&mut buffer).map_err(|e| {
            inbound_event_refusal(format!(
                "the request body is not a valid {} message stream: {e}",
                service_common::event_stream::EVENT_STREAM_CONTENT_TYPE
            ))
        })?;
        if buffer.remaining() >= before {
            return Err(inbound_event_refusal(
                "the request body carries a zero-length event-stream frame",
            ));
        }
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
        let message_type = header(FRAME_HEADER_MESSAGE_TYPE).unwrap_or_default();
        if message_type != FRAME_MESSAGE_TYPE_EVENT {
            return Err(inbound_event_refusal(format!(
                "an event-stream frame with {FRAME_HEADER_MESSAGE_TYPE} '{message_type}' may not \
                 be sent on an input stream; only '{FRAME_MESSAGE_TYPE_EVENT}' frames may"
            )));
        }
        let event_type = header(FRAME_HEADER_EVENT_TYPE).ok_or_else(|| {
            inbound_event_refusal(format!(
                "an event-stream frame carries no {FRAME_HEADER_EVENT_TYPE} header naming the \
                 event it is"
            ))
        })?;
        if closed {
            return Err(inbound_event_refusal(format!(
                "a {event_type} follows the {MEMBER_CLOSE_STREAM_EVENT} that closed the input \
                 stream; no event may be sent on a closed input stream"
            )));
        }
        match event_type.as_str() {
            MEMBER_TEXT_EVENT => {
                let text_event: TextEvent =
                    event_payload(protocol, message.payload(), &TextEvent::SCHEMA)
                        .map_err(inbound_event_refusal)?;
                events.push(ActionEvent::Text(text_event));
            }
            MEMBER_CLOSE_STREAM_EVENT => {
                closed = true;
                events.push(ActionEvent::CloseStream);
            }
            other => {
                return Err(inbound_event_refusal(format!(
                    "'{other}' is not a member of StartSpeechSynthesisStreamActionStream; send \
                     {MEMBER_TEXT_EVENT} or {MEMBER_CLOSE_STREAM_EVENT} events"
                )))
            }
        }
    }
    Ok(events)
}

impl<Eng: Send + Sync + 'static> service_common::event_stream::EventStreamOperation
    for StartSpeechSynthesisStream<Eng>
{
    fn serve<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, Response> {
        Box::pin(async move {
            // This service overrides its wire protocol (`crate::service::wire`), so
            // the stream is framed and its failures are rendered through the SAME
            // codec every other operation of it answers in.
            let protocol = svc
                .override_protocol()
                .unwrap_or_else(|| svc.definition().protocol());
            let response = match self.respond(protocol, req, ctx).await {
                Ok(response) => response,
                Err(error) => error_response(protocol, &error),
            };
            // One access record per request, so a refused stream is diagnosable. No
            // request payload and no credential reaches it.
            tracing::debug!(
                operation = "StartSpeechSynthesisStream",
                service = ctx.identity().signing_name(),
                method = ctx.parts().method.as_str(),
                status = response.status().as_u16(),
                "served an event-stream request"
            );
            response
        })
    }
}
