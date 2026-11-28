// SPDX-License-Identifier: CC0-1.0

//! @op StartMedicalScribeListeningSession
//!
//! `POST /medical-scribe-stream/ -> 200`. Opens a Medical Scribe listening session on a
//! live (`domainId`, `subscriptionId`) pair: reads the session's parameters off the
//! request headers, applies the events the caller framed into the request body, PERSISTS
//! the session record ([`crate::state::ScribeSession`]), and answers with the session's
//! own response headers and a `MedicalScribeOutputStream` body.
//!
//! This is the one operation that creates a listening session, so what it stores is
//! exactly what [`super::get_medical_scribe_listening_session`] reports back: the
//! language, sample rate and encoding the headers negotiated, the post-stream action
//! settings, channel definitions and encounter-context flag a `configurationEvent`
//! carried, and the stream's status.
//!
//! ## What the input stream's events do
//!
//! `MedicalScribeInputStream` is a union of four members and binds none of them as
//! required (`smithy_query(query="shape", shape="MedicalScribeInputStream")`), so the
//! events are applied in ARRIVAL ORDER and a member the stream never sends simply
//! configures nothing:
//!
//! * `configurationEvent` — carries the `postStreamActionSettings` the session is set up
//!   with (required WITHIN that event), its optional `channelDefinitions` and its
//!   optional `encounterContext`. All of it is stored on the session, which is where the
//!   reader gets it from; the context itself is `@sensitive` and only the modelled
//!   `encounterContextProvided` flag is kept.
//! * `audioEvent` / `binaryAudioEvent` — one chunk of audio each. They differ only in
//!   where the chunk travels: `MedicalScribeAudioEvent$audioChunk` is an ordinary blob
//!   member, so it arrives base64-encoded inside the frame's JSON payload, while
//!   `MedicalScribeBinaryAudioEvent$audioChunk` is the event's `@eventPayload`, so the
//!   frame payload IS the raw chunk.
//! * `sessionControlEvent` — `END_OF_SESSION` is the one type the model declares, and it
//!   ends the stream: the session moves to `COMPLETED` and is stamped with its end
//!   instant. A stream that sends none stays `IN_PROGRESS`, because the model lets a
//!   session be resumed under the same `sessionId`.
//!
//! ## What the response stream carries
//!
//! `MedicalScribeOutputStream` carries a transcript event or one of two in-band errors,
//! and this emulator has NO speech recogniser. So no `transcriptEvent` is ever framed
//! and none is invented: a stream that carried audio is answered with the modelled
//! `internalFailureException` member, framed as an in-band exception with
//! [`NO_RECOGNISER`] as its message, and a stream that carried no audio is answered with
//! no event at all — there is nothing to transcribe and nothing failed. The SESSION is
//! real either way, which is the point of the operation: it is stored, it is readable
//! through `GetMedicalScribeListeningSession`, and the response headers are read back
//! out of the stored record.
//!
//! ## Why this operation is hand-authored
//!
//! Its input carries a `@streaming` event-stream member and its output is a `@streaming`
//! union. The op-stub generator declines the first and the generated `ErasedOperation`
//! shim declines the second, because `serialize_as_event_stream<P>` needs a `Sized`
//! protocol that an object-safe `run` cannot hand it. The framing, the request reader,
//! the response and the routing seam are the shared ones in
//! `service_common::event_stream`; `src/service.rs` routes a request here through
//! `Service::override_operation`, which the router consults BEFORE the generated routing
//! table. Two readers a streaming input needs are local to this file for the reason that
//! module's own documentation gives: the header-bound half of the request, and one
//! frame's payload.
//!
//! ## Input (every member an `@httpHeader` except the payload)
//!
//! * `sessionId` (ScribeSessionId, len 36..36, UUID pattern) **required**
//! * `domainId` (DomainId, len 20..25, pattern `^(hai-|dom-)[a-z0-9]+$`) **required**
//! * `subscriptionId` (SubscriptionId, len 25..25, pattern `^sub-[a-zA-Z0-9]{21}$`)
//!   **required**
//! * `languageCode` (MedicalScribeLanguageCode: en-US | multi) **required**
//! * `mediaSampleRateHertz` (integer, range 8000..48000) **required**
//! * `mediaEncoding` (MedicalScribeMediaEncoding: pcm | flac) **required**
//! * `inputStream` (MedicalScribeInputStream, `@httpPayload`, `@streaming`)
//!
//! ## Output
//!
//! `sessionId`, `domainId`, `subscriptionId`, `requestId`, `languageCode`,
//! `mediaSampleRateHertz` and `mediaEncoding` as response headers, and `responseStream`
//! (`MedicalScribeOutputStream`, `@httpPayload`, `@streaming`) as the body.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 401) — the client supplied no proper credentials.
//! * `InternalServerException` (HTTP 500) — a transient failure of the service itself.
//! * `ResourceNotFoundException` (HTTP 404) — the domain or the subscription the headers
//!   name is not held in the Region the request was signed for.
//! * `ServiceQuotaExceededException` (HTTP 402) — the request exceeds a service quota.
//! * `ThrottlingException` (HTTP 429) — the client exceeded the allowed request rate.
//! * `ValidationException` (HTTP 400) — the request breaks the model's rules.

#[allow(unused_imports)]
use super::types::*;
// This operation's OWN type module. `types/mod.rs` is rendered from the model by the
// scaffold generator, which skips this operation entirely (its input is a `@streaming`
// union it emits no framing for) and so declares no module for it — and rewrites that file
// on every model update. The declaration therefore lives HERE, in the one file that uses
// those shapes, where no update can drop it again.
#[path = "types/start_medical_scribe_listening_session_types.rs"]
mod session_wire;
use session_wire::*;
// This operation's exception shapes: the hand-written set, which carries the WIRE KEY the
// plane answering this call writes their detail text under
// (`crate::service::exception_wire`), named explicitly so they shadow the model-keyed
// copies `types` re-exports.
use crate::service::exception_wire::{
    AccessDeniedException, InternalServerException, ResourceNotFoundException,
    ServiceQuotaExceededException, ThrottlingException, ValidationException,
};
use crate::state::{ScribeSession, ScribeSessionStart, ScribeStreamStatus};
use aws_smithy_eventstream::frame::read_message_from;
use aws_smithy_runtime_api::http::{Request, Response};
use aws_smithy_schema::serde::SerializableStruct;
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::DeserializableStruct;
use aws_smithy_types::body::SdkBody;
use bytes::Buf;
use service_common::event_stream::{
    error_response, event_stream_response, frame_event, read_request, EventStreamOperation,
    RequestRefusal, EVENT_STREAM_CONTENT_TYPE,
};
use smithy_server_runtime::{BoxFuture, OperationContext, ServerHttpProtocol, Service};
use std::sync::Arc;
use std::time::SystemTime;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-connecthealth-connecthealth.json");

/// The URI the model binds this operation to
/// (`smithy_query(query="operation", operation="StartMedicalScribeListeningSession")`:
/// `POST /medical-scribe-stream/`). AWS publishes no WebSocket transport for it, so
/// there is no second spelling of this path to claim.
pub const MODELED_PATH: &str = "/medical-scribe-stream/";

/// The method the model binds this operation to.
pub const MODELED_METHOD: &str = "POST";

/// What this emulator answers the TRANSCRIPTION itself with.
///
/// The control plane of this operation runs for real — the request is validated against
/// the model, the domain and the subscription are resolved, the session and its
/// negotiated settings are stored, and all of it is reported back — but no speech
/// recogniser exists in this process, so there is no transcript to report for the audio
/// a stream carried. Rather than invent words, that stream carries the modelled
/// `internalFailureException` member of `MedicalScribeOutputStream`, framed as an in-band
/// exception, with this message: the model's own channel for a stream that yields no
/// result.
pub const NO_RECOGNISER: &str = "This service performs no speech recognition here, so no \
     transcript segment is produced for the streamed audio. The listening session, its \
     negotiated settings and its status are real and are reported by \
     GetMedicalScribeListeningSession.";

/// The operation's MODELLED errors; `#[operation_error]` accepts only variants the model
/// declares for it.
#[smithy_macros::operation_error("com.amazonaws.connecthealth#StartMedicalScribeListeningSession")]
#[derive(Debug)]
pub enum StartMedicalScribeListeningSessionErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// The `StartMedicalScribeListeningSession` operation. Owns the shared engine (the
/// constructor seam), like every other operation of this crate.
pub struct StartMedicalScribeListeningSession<Eng> {
    engine: Arc<Eng>,
}

impl<Eng> StartMedicalScribeListeningSession<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }
}

/// A request this service cannot serve, reported through the shape the model declares
/// for invalid client input — built in the one place every handler of this crate builds
/// it ([`crate::state::invalid_request`]).
fn invalid(detail: impl Into<String>) -> StartMedicalScribeListeningSessionErrorModeled {
    StartMedicalScribeListeningSessionErrorModeled::ValidationException(
        crate::state::invalid_request(detail),
    )
}

/// The refusal of a header-bound member, worded the way the rest of this service words
/// the same refusal: the FRAMEWORK's own clause, except for the one member this service
/// states itself — a `domainId` outside what `DomainId` declares, which carries this
/// service's domain-id sentence ([`crate::service::domain_id_refusal`]) wherever a request
/// names one. The generated operations reach that sentence through the service's
/// `override_refusal` seam; this operation reads its own members, so it asks for the
/// sentence here rather than growing a second wording of it.
fn refused_member(refusal: RequestRefusal) -> StartMedicalScribeListeningSessionErrorModeled {
    if let RequestRefusal::Invalid(violations) = &refusal {
        if let Some(sentence) = crate::service::domain_id_refusal::sentence_for(violations) {
            return invalid(sentence);
        }
    }
    invalid(refusal.stated())
}

/// A resource of the request's scope this Region does not hold.
fn not_found(message: String) -> StartMedicalScribeListeningSessionErrorModeled {
    StartMedicalScribeListeningSessionErrorModeled::ResourceNotFoundException(
        ResourceNotFoundException {
            message: Some(message),
            ..Default::default()
        },
    )
}

/// A bound of this service the request went past: the error the model documents as "The
/// request exceeds a service quota."
fn quota_exceeded(message: String) -> StartMedicalScribeListeningSessionErrorModeled {
    StartMedicalScribeListeningSessionErrorModeled::ServiceQuotaExceededException(
        ServiceQuotaExceededException {
            message: Some(message),
            ..Default::default()
        },
    )
}

/// A fault on this service's side, reported through the shape that classifies one.
fn internal(message: String) -> StartMedicalScribeListeningSessionErrorModeled {
    StartMedicalScribeListeningSessionErrorModeled::InternalServerException(
        InternalServerException {
            message: Some(message),
            ..Default::default()
        },
    )
}

impl<Eng: Send + Sync + 'static> StartMedicalScribeListeningSession<Eng> {
    fn respond(
        &self,
        protocol: &dyn ServerHttpProtocol,
        req: &Request,
        ctx: &OperationContext<'_>,
    ) -> Result<Response, StartMedicalScribeListeningSessionErrorModeled> {
        // The header-bound half of the request, read and checked by the MODEL.
        let settings: StartMedicalScribeListeningSessionSettings =
            settings_from_headers(protocol, req)?;

        // Everything the input stream's events say about the session, read before any
        // state is touched: a request that breaks the model's rules writes nothing.
        let now = SystemTime::now();
        let mut session = ScribeSession {
            session_id: settings.session_id.clone(),
            domain_id: settings.domain_id.clone(),
            subscription_id: settings.subscription_id.clone(),
            language_code: settings.language_code.clone(),
            media_sample_rate_hertz: settings.media_sample_rate_hertz,
            media_encoding: settings.media_encoding.clone(),
            channel_definitions: Vec::new(),
            post_stream_action_settings: None,
            post_stream_action_result: None,
            encounter_context_provided: false,
            medical_coding_context_provided: false,
            stream_status: ScribeStreamStatus::InProgress,
            stream_creation_time: now,
            stream_end_time: None,
        };
        let streamed = apply_input_stream(protocol, req, &mut session, now)?;

        let region = crate::state::region_of(ctx);
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(internal(
                crate::state::store_unreachable("StartMedicalScribeListeningSession")
                    .message
                    .unwrap_or_default(),
            ));
        };

        let stored = {
            let mut store = state.lock();
            // The one preamble every handler shares: everything the CLOCK already made
            // due, which here means a domain whose DELETING span has elapsed is purged
            // before this session's scope is resolved against it.
            store.materialise_due(now);

            // A session is reachable only through the (`domainId`, `subscriptionId`) pair
            // it belongs to, so it can only be STARTED on a pair this Region holds live —
            // the same scoped chain, and the same two sentences, the reader resolves.
            if store.live_domain(&region, &settings.domain_id).is_none() {
                return Err(not_found(
                    crate::service::error_detail::domain_not_found_message(&settings.domain_id),
                ));
            }
            if store
                .live_subscription(&region, &settings.domain_id, &settings.subscription_id)
                .is_none()
            {
                return Err(not_found(
                    crate::service::error_detail::subscription_not_found_message(
                        &settings.domain_id,
                        &settings.subscription_id,
                    ),
                ));
            }
            let request_id = store.mint_request_id();
            let Some(subscription) =
                store.live_subscription_mut(&region, &settings.domain_id, &settings.subscription_id)
            else {
                return Err(not_found(
                    crate::service::error_detail::subscription_not_found_message(
                        &settings.domain_id,
                        &settings.subscription_id,
                    ),
                ));
            };
            let session_id = session.session_id.clone();
            match subscription.start_scribe_session(session) {
                ScribeSessionStart::AtCapacity => {
                    return Err(quota_exceeded(format!(
                        "subscription {} already holds the maximum of {} Medical Scribe \
                         listening sessions",
                        settings.subscription_id,
                        crate::state::MAX_SCRIBE_SESSIONS_PER_SUBSCRIPTION
                    )))
                }
                ScribeSessionStart::Started | ScribeSessionStart::Resumed => {}
            }
            // Read the session back OUT of the store: the response reports what was
            // persisted, never what the request headers said.
            let Some(stored) = store.live_scribe_session(
                &region,
                &settings.domain_id,
                &settings.subscription_id,
                &session_id,
            ) else {
                return Err(internal(
                    "the Medical Scribe listening session was not persisted".to_string(),
                ));
            };
            StartMedicalScribeListeningSessionHeaders {
                session_id: Some(stored.session_id.clone()),
                domain_id: Some(stored.domain_id.clone()),
                subscription_id: Some(stored.subscription_id.clone()),
                request_id: Some(request_id),
                language_code: Some(stored.language_code.clone()),
                media_sample_rate_hertz: Some(stored.media_sample_rate_hertz),
                media_encoding: Some(stored.media_encoding.clone()),
            }
        };

        // No recogniser ran, so the response stream carries no transcript. A stream that
        // sent audio is told so in band; one that sent none has nothing to be told.
        let frames = if streamed.audio_bytes > 0 {
            frame_event(
                protocol,
                MEMBER_INTERNAL_FAILURE_EXCEPTION,
                true,
                &InternalServerException {
                    message: Some(NO_RECOGNISER.to_string()),
                    ..Default::default()
                },
                &SCHEMA_INTERNAL_SERVER_EXCEPTION,
            )
            .map_err(|error| {
                internal(format!("the response stream could not be framed: {error}"))
            })?
        } else {
            Vec::new()
        };

        event_stream_response(
            protocol,
            "StartMedicalScribeListeningSession",
            &stored,
            &StartMedicalScribeListeningSessionHeaders::SCHEMA,
            frames,
        )
        .map_err(|error| internal(format!("the response could not be serialized: {error}")))
    }
}

/// What one input stream turned out to carry, beside what it configured on the session.
#[derive(Debug, Default)]
struct StreamedAudio {
    /// How many bytes of audio chunk the stream carried, over every audio event of
    /// either spelling. It decides whether there was anything to transcribe.
    audio_bytes: u64,
}

/// Apply every event of the request's `inputStream` to `session`, in arrival order.
///
/// The frames are decoded with the shared `vnd.amazon.eventstream` reader and each
/// payload is checked against the MODEL's own rules for the event it names, so a
/// configuration this service stores is one the model admits. Bounded at
/// [`crate::state::MAX_SCRIBE_STREAM_EVENTS`] frames, because the frame count is work a
/// caller sizes.
fn apply_input_stream(
    protocol: &dyn ServerHttpProtocol,
    req: &Request,
    session: &mut ScribeSession,
    now: SystemTime,
) -> Result<StreamedAudio, StartMedicalScribeListeningSessionErrorModeled> {
    let mut streamed = StreamedAudio::default();
    // `inputStream` is not `@required`, so a request that opens the stream and sends no
    // event is a request the model admits: it starts a session that nothing has
    // configured yet.
    let Some(body) = req.body().bytes() else {
        return Ok(streamed);
    };
    let mut buffer = bytes::Bytes::copy_from_slice(body);
    let mut frames: usize = 0;
    while buffer.has_remaining() {
        if frames >= crate::state::MAX_SCRIBE_STREAM_EVENTS {
            return Err(quota_exceeded(format!(
                "the input stream carries more than the {} events this service accepts in \
                 one request; send the audio in fewer, larger events",
                crate::state::MAX_SCRIBE_STREAM_EVENTS
            )));
        }
        frames += 1;
        let before = buffer.remaining();
        let message = read_message_from(&mut buffer).map_err(|error| {
            invalid(format!(
                "the request body is not a valid {EVENT_STREAM_CONTENT_TYPE} message \
                 stream: {error}"
            ))
        })?;
        if buffer.remaining() >= before {
            return Err(invalid(
                "the request body carries a zero-length event-stream frame",
            ));
        }
        let header = |name: &str| {
            message
                .headers()
                .iter()
                .find(|header| header.name().as_str() == name)
                .and_then(|header| match header.value() {
                    aws_smithy_types::event_stream::HeaderValue::String(value) => {
                        Some(value.as_str().to_string())
                    }
                    _ => None,
                })
        };
        // `:message-type` is `event` for a union member and `exception` for an in-band
        // error; a client sends only events.
        let message_type = header(":message-type").unwrap_or_default();
        if message_type != "event" {
            return Err(invalid(format!(
                "an event-stream frame with :message-type '{message_type}' may not be sent \
                 on an input stream; only 'event' frames may"
            )));
        }
        let Some(event_type) = header(":event-type") else {
            return Err(invalid(
                "an event-stream frame carries no :event-type header naming the event",
            ));
        };
        match event_type.as_str() {
            MEMBER_CONFIGURATION_EVENT => {
                let configuration: MedicalScribeConfigurationEvent = event_payload(
                    protocol,
                    message.payload(),
                    &MedicalScribeConfigurationEvent::SCHEMA,
                )?;
                apply_configuration(session, configuration);
            }
            MEMBER_AUDIO_EVENT => {
                // `MedicalScribeAudioEvent$audioChunk` is an ordinary blob member, so the
                // chunk is base64-encoded inside the frame's JSON payload.
                let audio: MedicalScribeAudioEvent =
                    event_payload(protocol, message.payload(), &MedicalScribeAudioEvent::SCHEMA)?;
                streamed.audio_bytes = streamed
                    .audio_bytes
                    .saturating_add(audio.audio_chunk.len() as u64);
            }
            MEMBER_BINARY_AUDIO_EVENT => {
                // `MedicalScribeBinaryAudioEvent$audioChunk` is the event's
                // `@eventPayload`, so the frame payload IS the chunk.
                streamed.audio_bytes = streamed
                    .audio_bytes
                    .saturating_add(message.payload().len() as u64);
            }
            MEMBER_SESSION_CONTROL_EVENT => {
                let control: MedicalScribeSessionControlEvent = event_payload(
                    protocol,
                    message.payload(),
                    &MedicalScribeSessionControlEvent::SCHEMA,
                )?;
                // `END_OF_SESSION` is the one type the enum declares, and the payload is
                // checked against that enum before it is acted on.
                if control.r#type.as_deref() == Some(END_OF_SESSION) {
                    session.stream_status = ScribeStreamStatus::Completed;
                    session.stream_end_time = Some(now);
                }
            }
            other => {
                return Err(invalid(format!(
                    "'{}' is not a member of MedicalScribeInputStream; send \
                     {MEMBER_CONFIGURATION_EVENT}, {MEMBER_AUDIO_EVENT}, \
                     {MEMBER_BINARY_AUDIO_EVENT} or {MEMBER_SESSION_CONTROL_EVENT} frames",
                    crate::service::error_detail::echoed(other)
                )))
            }
        }
    }
    Ok(streamed)
}

/// Persist everything a `configurationEvent` carries onto the session, because
/// `GetMedicalScribeListeningSession` reports all of it.
///
/// The post-stream settings are stored in the `*Response` shape the reader answers with,
/// which is the same member set the event carries minus the custom template's
/// instructions — the model's response shape declares no member for those, and they are
/// `@sensitive`, so they are not kept. The encounter context itself is `@sensitive` too,
/// and so is the structured medical-coding context inside it: only the modelled
/// `encounterContextProvided` / `medicalCodingContextProvided` flags are stored, which is
/// all the model reports about either.
fn apply_configuration(session: &mut ScribeSession, configuration: MedicalScribeConfigurationEvent) {
    let settings = configuration.post_stream_action_settings;
    session.post_stream_action_settings = Some(MedicalScribePostStreamActionSettingsResponse {
        output_s3_uri: settings.output_s3_uri,
        clinical_note_generation_settings: ClinicalNoteGenerationSettingsResponse {
            note_template_settings: Some(
                match settings.clinical_note_generation_settings.note_template_settings {
                    NoteTemplateSettings::managedTemplate(template) => {
                        NoteTemplateSettingsResponse::managedTemplate(ManagedTemplateResponse {
                            template_type: Some(template.template_type),
                        })
                    }
                    NoteTemplateSettings::customTemplate(template) => {
                        NoteTemplateSettingsResponse::customTemplate(CustomTemplateResponse {
                            template_type: Some(template.template_type),
                        })
                    }
                },
            ),
        },
        // Whether the session generates medical codes after the stream: a `@required`
        // member of the event's own settings shape, so the flag the reader answers with is
        // the one the configuration carried rather than a default of this service.
        medical_code_generation_settings: settings.medical_code_generation_settings.map(
            |generation| MedicalCodeGenerationSettingsResponse {
                enabled: Some(generation.enabled),
            },
        ),
    });
    session.channel_definitions = configuration.channel_definitions.unwrap_or_default();
    session.encounter_context_provided = configuration.encounter_context.is_some();
    // The structured coding context travels INSIDE that encounter context, and the model
    // reports the same thing about it: whether the configuration carried one.
    session.medical_coding_context_provided = configuration
        .encounter_context
        .is_some_and(|context| context.medical_coding_context.is_some());
}

/// Read the `@httpHeader`-bound half of the request into the modelled settings shape and
/// check it against the model.
///
/// Driven through the shared request reader over a copy of the request whose body is
/// EMPTY: every member of the settings shape is header-bound (its event-stream payload
/// member is excluded from it), so an empty body is what keeps the JSON codec from being
/// pointed at event-stream frames, and the model's own enums, patterns and ranges are
/// what admit or refuse the request. The refusal is worded by [`refused_member`], so a
/// header-bound member of this operation reads exactly like a body or label member of
/// another operation the request boundary refuses.
fn settings_from_headers(
    protocol: &dyn ServerHttpProtocol,
    req: &Request,
) -> Result<
    StartMedicalScribeListeningSessionSettings,
    StartMedicalScribeListeningSessionErrorModeled,
> {
    let mut headers_only = Request::new(SdkBody::empty());
    headers_only
        .set_uri(req.uri())
        .map_err(|error| invalid(format!("the request URI could not be read: {error}")))?;
    headers_only
        .set_method(req.method())
        .map_err(|error| invalid(format!("the request method could not be read: {error}")))?;
    for (name, value) in req.headers().iter() {
        headers_only
            .headers_mut()
            .append(name.to_string(), value.to_string());
    }
    read_request(
        protocol,
        &headers_only,
        &StartMedicalScribeListeningSessionSettings::SCHEMA,
    )
    .map_err(refused_member)
}

/// Deserialize one frame's payload into the modelled event structure `T`, then check it
/// against the model's constraints.
///
/// Driven through the service's own protocol: the payload is handed to the protocol as a
/// body, and `T`'s schema binds no HTTP member, so the protocol's body codec is what
/// parses it and the model's traits are what refuse it.
fn event_payload<T>(
    protocol: &dyn ServerHttpProtocol,
    payload: &[u8],
    schema: &Schema,
) -> Result<T, StartMedicalScribeListeningSessionErrorModeled>
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

impl<Eng: Send + Sync + 'static> EventStreamOperation for StartMedicalScribeListeningSession<Eng> {
    fn serve<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, Response> {
        Box::pin(async move {
            let protocol = svc.definition().protocol();
            let response = match self.respond(protocol, req, ctx) {
                Ok(response) => response,
                // A failure is rendered by the framework, in the modelled `message`
                // member, the same way every other operation of this service reports one.
                Err(error) => error_response(protocol, &error),
            };
            // One access record per request, so a refused stream is diagnosable. No
            // request payload and no credential reaches it.
            tracing::debug!(
                operation = "StartMedicalScribeListeningSession",
                service = ctx.identity().signing_name(),
                method = ctx.parts().method.as_str(),
                status = response.status().as_u16(),
                "served an event-stream request"
            );
            response
        })
    }
}
