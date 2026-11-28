// SPDX-License-Identifier: CC0-1.0

//! @op UpdateProfileWithAgent
//!
//! Serves one TURN of the chat-based profile update this operation documents
//! ("Updates a data transformation profile using chat-based interaction with an
//! agent. Supports multi-turn conversations for iteratively customizing profiles"):
//! it resolves the stored data transformation profile, appends the turn to the
//! conversation the request names — minting a `ConversationId` when it names none,
//! refusing a turn whose `ConversationId` no record holds and one naming a
//! conversation of another profile —
//! and answers with the `AgentOutputMessageStream` event stream the model declares,
//! as a real `vnd.amazon.eventstream` body — which OPENS with the frame a client reads
//! the output shape itself out of ([`service_common::event_stream::INITIAL_RESPONSE_EVENT`]),
//! before any frame of the union.
//!
//! ## The loop the model describes
//!
//! `AgentInputMessage.Type` is `normal` or `confirmation_response`, and
//! `EndOfOutputEvent` carries `Options` beside its `FinalResponseMessage` and
//! `ConversationId`. Those members describe one loop, and it is the loop served here:
//!
//! * a `normal` turn reads the profile's DRAFT and asks whether to record the change,
//!   offering the options that answer it ([`crate::state::AGENT_APPLY_OPTION`] and
//!   [`crate::state::AGENT_DISCARD_OPTION`]), which the conversation then remembers;
//! * a `confirmation_response` turn that names the APPLYING option is the turn that
//!   WRITES: it stamps the profile's DRAFT (version
//!   [`crate::state::DRAFT_PROFILE_VERSION`]) with the agent's `ChangeDescription`
//!   and the instant it ran, which `GetDataTransformationProfile` and
//!   `ListDataTransformationProfileVersions` read back;
//! * a `confirmation_response` turn that names the discarding option drops the
//!   proposal and writes nothing;
//! * a `confirmation_response` turn the conversation is not waiting for — nothing
//!   pending in it, or a message naming none of the offered options — is
//!   reported IN BAND, as an `ErrorEvent` carrying the modelled
//!   `AgentErrorType` value `ValidationException`: the request and the profile were
//!   both fine, so the stream had already begun.
//!
//! No language model runs in this emulator, so the agent authors no mapping content:
//! every sentence it answers with is composed from the stored profile and the stored
//! conversation ([`crate::state::agent_conversation`]), nothing is echoed back from
//! the caller's `@sensitive` message, and the DRAFT's files are left exactly as they
//! stand — content is stored through `UpdateDataTransformationProfile`, which replaces
//! the DRAFT with content the request carries.
//!
//! ## Why this file is hand-authored
//!
//! The op-stub generator declined this operation because its output member is a
//! `@streaming` event-stream union it cannot frame (`src/ops/SKIPPED_STUBS.md`), and
//! the generated `ErasedOperation` shim of such an operation declines the request too:
//! `serialize_as_event_stream<P>` needs a `Sized` protocol, which the object-safe
//! `run` cannot hand it. So there is no `impl Operation` here and no centralized
//! deserialize -> validate -> handle -> serialize sequence: the framing lives in
//! `service_common::event_stream`, the request half is read through the framework's
//! own deserializer and constraint walk, a violation is refused in this service's own
//! words ([`crate::service::refuse_invalid_input_of`], the same body every other
//! operation's refusal is rendered from), and `src/service.rs` routes a request here
//! through an `EventStreamRoute`.
//!
//! ---
//!
//! # Modeled contract for `UpdateProfileWithAgent`
//!
//! ```text
//! smithy_query(query="operation",operation="UpdateProfileWithAgent",model_path="services/service-com-amazonaws-healthlake-healthlake/smithy-model/com-amazonaws-healthlake-healthlake.json")
//! ```
//!
//! ## Input
//!
//! * `ProfileId` (ProfileIdString, len 32..32, pattern ^[a-f0-9]{32}$) **required** —
//!   the profile the turn is about.
//! * `InputMessage` (AgentInputMessage) **required** — `Body` (AgentMessageString, len
//!   1..40960, @sensitive) **required** and `Type` (AgentInputMessageType, one of:
//!   normal | confirmation_response) **required**.
//! * `ConversationId` (ConversationIdString, len 1..36) — the conversation this turn
//!   belongs to; absent on a first turn.
//!
//! ## Output
//!
//! * `AgentResponse` (AgentOutputMessageStream, @httpPayload, @streaming)
//!   **required** — the event stream; the response binds no modelled header. The union
//!   carries `TextMessageEvent`, `ProgressEvent`, `HeartbeatEvent`, `ErrorEvent` and
//!   `EndOfOutputEvent`.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403), `InternalServerException` (HTTP 500),
//!   `ResourceNotFoundException` (HTTP 404), `ThrottlingException` (HTTP 429),
//!   `ValidationException` (HTTP 400). Each goes on the wire with the status this
//!   service puts on an error's FAULT (`crate::scaffolded::wire_http_status`), because
//!   awsJson1_0 binds nothing to the transport.

#[allow(unused_imports)]
use super::types::*;

use std::sync::Arc;
use std::time::SystemTime;

use aws_smithy_runtime_api::http::{Request, Response};
use aws_smithy_schema::serde::SerializableStruct;
use aws_smithy_schema::Schema;
use service_common::event_stream::{
    deserialize_request, error_response, event_stream_only_response,
    frame_empty_initial_response, frame_event,
};
use smithy_server_runtime::{BoxFuture, OperationContext, ServerHttpProtocol, Service};

use crate::state::{
    agent_applied_message, agent_change_description, agent_declined_message,
    agent_nothing_to_confirm_message, agent_proposal_message, agent_review_progress_message,
    agent_unoffered_option_message, conversation_not_found_message,
    conversation_of_other_profile_message, profile_version_not_found_message,
    AgentChatOption, AgentConversationRefusal, AgentPendingChange, AGENT_APPLY_OPTION,
    AGENT_DISCARD_OPTION, AGENT_ERROR_VALIDATION, AGENT_MESSAGE_TYPE_CONFIRMATION_RESPONSE,
    AGENT_MESSAGE_TYPE_NORMAL, DRAFT_PROFILE_VERSION,
};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-healthlake-healthlake.json");

/// The routing key this operation is addressed by — the suffix of the `X-Amz-Target`
/// header awsJson1_0 routes on, which is the operation's own name.
///
/// Named here, beside the operation, because [`crate::service`] has no
/// `impl Operation` to read `NAME` off: a `@streaming`-output operation is not driven
/// through the centralized sequence.
pub const UPDATE_PROFILE_WITH_AGENT: &str = "UpdateProfileWithAgent";

/// The `AgentOutputMessageStream` member each frame carries in its `:event-type`
/// header: the union's own member names.
const PROGRESS_EVENT: &str = "ProgressEvent";
const TEXT_MESSAGE_EVENT: &str = "TextMessageEvent";
const ERROR_EVENT: &str = "ErrorEvent";
const END_OF_OUTPUT_EVENT: &str = "EndOfOutputEvent";

/// The violation path of the `InputMessage.Type` member, as a violation path spells a
/// member name (lowerCamel, nested members joined by `.`).
const MESSAGE_TYPE_PATH: &str = "inputMessage.type";

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation.
#[smithy_macros::operation_error("com.amazonaws.healthlake#UpdateProfileWithAgent")]
#[derive(Debug)]
pub enum UpdateProfileWithAgentErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel.
///
/// Wrapped in [`crate::scaffolded::Scaffolded`] for the reason every other operation
/// of this service wraps its own: the wrapper is what maps a modelled error's
/// `@httpError` onto the status this service actually puts on the wire
/// ([`crate::scaffolded::wire_http_status`]), so a refusal from here is answered with
/// the same status as the same error from any sibling operation.
pub type UpdateProfileWithAgentError =
    crate::scaffolded::Scaffolded<UpdateProfileWithAgentErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<UpdateProfileWithAgentErrorModeled> for UpdateProfileWithAgentError {
    fn from(modeled: UpdateProfileWithAgentErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateProfileWithAgent` operation.
///
/// Owns the shared engine (the constructor seam). The generated routing table holds
/// one of these — where its generated shim declines the request — and `service.rs`
/// holds another wrapped in an `EventStreamRoute`; both reach the same state, because
/// the state lives in the shared engine, not on the operation.
pub struct UpdateProfileWithAgent<Eng> {
    engine: Arc<Eng>,
}

impl<Eng> UpdateProfileWithAgent<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }
}

/// What one turn did, which decides the frames it answers with.
#[derive(Debug, PartialEq, Eq)]
enum Turn {
    /// The agent asked whether to record the change, offering `options`.
    Asked { options: Vec<String> },
    /// The caller confirmed the change, so this turn writes the DRAFT.
    Confirmed,
    /// The proposal was dropped at the caller's request; nothing is written.
    Dropped,
    /// The turn could not be acted on, and says why IN BAND. The options the
    /// conversation is still offering, if it is offering any, go out with it so the
    /// caller can answer.
    Refused {
        message: String,
        options: Vec<String>,
    },
}

impl<Eng: Send + Sync + 'static> UpdateProfileWithAgent<Eng> {
    /// Serve one turn: resolve the profile, record the turn against its conversation,
    /// apply whatever the turn decided, and frame the answer.
    fn respond(
        &self,
        protocol: &dyn ServerHttpProtocol,
        req: &Request,
    ) -> Result<Response, UpdateProfileWithAgentError> {
        let input: UpdateProfileWithAgentInput =
            deserialize_request(protocol, req, &UpdateProfileWithAgentInput::SCHEMA)
                .map_err(validation)?;

        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(internal_server(
                "the HealthLake data transformation profile registry is unavailable",
            ));
        };

        let now = SystemTime::now();
        let mut store = state.lock();
        // Every conversation whose retention the clock has already run out is gone
        // before this turn resolves the one it names.
        store.sweep_agent_conversations(now);

        // The profile is resolved FIRST, so a turn about a profile no record holds
        // mints no conversation and records no turn. This call resolves the DRAFT —
        // it is the version the change is recorded against — so the refusal names it
        // beside the profile, exactly as `UpdateDataTransformationProfile`'s does.
        if !store.profiles.contains_key(&input.profile_id) {
            return Err(not_found(profile_version_not_found_message(
                &input.profile_id,
                DRAFT_PROFILE_VERSION,
            )));
        }

        // The conversation side of the turn, decided while the conversation record is
        // still borrowed: it needs the pending proposal and nothing of the profile, so
        // the record is read and rewritten in ONE place rather than looked up again.
        //
        // A turn that NAMES a conversation continues the one held under that id for
        // this profile. An id no record holds — never served, swept by its retention,
        // or held for another profile — is no conversation of this profile, so the turn
        // is refused rather than a fresh conversation being opened behind the id the
        // caller sent.
        let (conversation_id, turn_number, turn) = {
            let conversation = match store.agent_conversation_turn(
                input.conversation_id.as_deref(),
                &input.profile_id,
                now,
            ) {
                Ok(conversation) => conversation,
                // An id no record holds names no conversation at all; one held for
                // another profile names a conversation this turn may not continue.
                // Two situations, two answers, each naming the id the request
                // carried.
                Err(refusal) => {
                    let named = input.conversation_id.as_deref().unwrap_or_default();
                    return Err(match refusal {
                        AgentConversationRefusal::NotFound => {
                            not_found(conversation_not_found_message(named))
                        }
                        AgentConversationRefusal::OtherProfile => {
                            validation(conversation_of_other_profile_message(&input.profile_id))
                        }
                    });
                }
            };
            let conversation_id = conversation.id.clone();
            let turn_number = conversation.turns;
            let turn = match input.input_message.r#type.as_str() {
                AGENT_MESSAGE_TYPE_CONFIRMATION_RESPONSE => {
                    // A confirmation answers a question the agent asked in THIS
                    // conversation; anything else is a turn the conversation is not
                    // waiting for. Read as owned values, so the record can be rewritten
                    // below.
                    let answered = conversation.pending.as_ref().map(|change| {
                        (
                            change.option_labels(),
                            change
                                .option_answered(&input.input_message.body)
                                .map(|option| option.applies),
                        )
                    });
                    match answered {
                        Some((_, Some(applies))) => {
                            // The question has been answered either way, so the
                            // conversation stops waiting on it.
                            conversation.pending = None;
                            if applies {
                                Turn::Confirmed
                            } else {
                                Turn::Dropped
                            }
                        }
                        Some((labels, None)) => Turn::Refused {
                            message: agent_unoffered_option_message(&labels),
                            options: labels,
                        },
                        None => Turn::Refused {
                            message: agent_nothing_to_confirm_message(
                                &input.profile_id,
                                &conversation_id,
                            ),
                            options: Vec::new(),
                        },
                    }
                }
                AGENT_MESSAGE_TYPE_NORMAL => {
                    let options = vec![
                        AgentChatOption {
                            label: AGENT_APPLY_OPTION.to_string(),
                            applies: true,
                        },
                        AgentChatOption {
                            label: AGENT_DISCARD_OPTION.to_string(),
                            applies: false,
                        },
                    ];
                    let labels: Vec<String> =
                        options.iter().map(|option| option.label.clone()).collect();
                    conversation.pending = Some(AgentPendingChange { options });
                    Turn::Asked { options: labels }
                }
                // Unreachable: the modelled `AgentInputMessageType` admits exactly the
                // two values above, and a value outside the enum is refused before this
                // handler runs. Stated rather than folded into the `normal` arm, so a
                // value this handler does not understand is never acted on as if it
                // were one it does — and answered with this service's own enum clause.
                _ => {
                    return Err(validation(crate::service::enum_violation_message(
                        MESSAGE_TYPE_PATH,
                        &[
                            AGENT_MESSAGE_TYPE_NORMAL,
                            AGENT_MESSAGE_TYPE_CONFIRMATION_RESPONSE,
                        ],
                    )))
                }
            };
            (conversation_id, turn_number, turn)
        };

        // What the agent reports it read, from the profile as it stands BEFORE this
        // turn writes anything.
        let progress = {
            let Some(profile) = store.profiles.get(&input.profile_id) else {
                return Err(internal_server(
                    "the profile this turn resolved left the store mid-request",
                ));
            };
            agent_review_progress_message(profile, turn_number)
        };

        if turn == Turn::Confirmed {
            let Some(profile) = store.profiles.get_mut(&input.profile_id) else {
                return Err(internal_server(
                    "the profile this turn resolved left the store mid-request",
                ));
            };
            // The write this operation exists for: the DRAFT carries the agent's
            // account of the change and the instant it was recorded, which the
            // modelled readers report.
            profile.draft.change_description = Some(agent_change_description(&conversation_id));
            profile.draft.last_updated_at = crate::state::recorded_instant(now);
        }

        // The answer, read off the profile as it stands AFTER the turn's write.
        let Some(profile) = store.profiles.get(&input.profile_id) else {
            return Err(internal_server(
                "the profile this turn resolved left the store mid-request",
            ));
        };
        let (final_message, options, in_band_error) = match &turn {
            Turn::Asked { options } => (agent_proposal_message(profile), options.clone(), false),
            Turn::Confirmed => (agent_applied_message(profile), Vec::new(), false),
            Turn::Dropped => (agent_declined_message(profile), Vec::new(), false),
            Turn::Refused { message, options } => (message.clone(), options.clone(), true),
        };
        drop(store);

        // The body OPENS with the frame a client reads the output shape out of, before
        // any frame of the union: `UpdateProfileWithAgentResponse`'s only member is the
        // stream itself, so that frame carries no member and its payload is the empty
        // document. A client's decoder reads it first and treats a body that begins with
        // anything else as unparseable, so without it none of the events behind it is
        // delivered.
        let mut frames = frame_empty_initial_response(protocol).map_err(|error| {
            internal_server(&format!(
                "the initial response event could not be framed: {error}"
            ))
        })?;
        self.frame(
            protocol,
            &mut frames,
            PROGRESS_EVENT,
            &ProgressEvent { message: progress },
            &ProgressEvent::SCHEMA,
        )?;
        if in_band_error {
            self.frame(
                protocol,
                &mut frames,
                ERROR_EVENT,
                &ErrorEvent {
                    error: AGENT_ERROR_VALIDATION.to_string(),
                    message: final_message.clone(),
                },
                &ErrorEvent::SCHEMA,
            )?;
        } else {
            // One text event, carrying the whole answer: the turn is computed in one
            // step, so nothing here splits it into fabricated increments.
            self.frame(
                protocol,
                &mut frames,
                TEXT_MESSAGE_EVENT,
                &TextMessageEvent {
                    message: final_message.clone(),
                },
                &TextMessageEvent::SCHEMA,
            )?;
        }
        self.frame(
            protocol,
            &mut frames,
            END_OF_OUTPUT_EVENT,
            &EndOfOutputEvent {
                final_response_message: final_message,
                // `Options` is what the NEXT turn may answer with, so it is absent
                // once there is nothing to answer.
                options: (!options.is_empty()).then_some(options),
                conversation_id: Some(conversation_id),
            },
            &EndOfOutputEvent::SCHEMA,
        )?;

        // `UpdateProfileWithAgentResponse` binds no header: the stream IS the response.
        Ok(event_stream_only_response(frames))
    }

    /// Append one `AgentOutputMessageStream` event to the stream, framed by this
    /// service's own protocol.
    fn frame(
        &self,
        protocol: &dyn ServerHttpProtocol,
        frames: &mut Vec<u8>,
        event_type: &str,
        event: &dyn SerializableStruct,
        schema: &Schema,
    ) -> Result<(), UpdateProfileWithAgentError> {
        // `is_exception` is false for every member of this union: none of them is an
        // `@error` shape, so each travels as a data event — `ErrorEvent` included,
        // which is a modelled event carrying an `AgentErrorType`.
        let frame = frame_event(protocol, event_type, false, event, schema).map_err(|error| {
            internal_server(&format!(
                "the {event_type} event could not be framed: {error}"
            ))
        })?;
        frames.extend_from_slice(&frame);
        Ok(())
    }
}

impl<Eng: Send + Sync + 'static> service_common::event_stream::EventStreamOperation
    for UpdateProfileWithAgent<Eng>
{
    fn serve<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, Response> {
        Box::pin(async move {
            let protocol = svc.definition().protocol();
            // A value outside a rule the model declares is refused from the MODELLED
            // `ValidationException` shape, through the same body the gate in
            // `service.rs` renders for every other operation of this service — so a
            // refusal here reads exactly like one from a non-streaming sibling.
            if let Some(refused) = crate::service::refuse_invalid_input_of::<
                UpdateProfileWithAgentInput,
            >(
                protocol,
                req,
                &UpdateProfileWithAgentInput::SCHEMA,
                UPDATE_PROFILE_WITH_AGENT,
            ) {
                return refused;
            }
            let response = match self.respond(protocol, req) {
                Ok(response) => response,
                Err(error) => error_response(protocol, &error),
            };
            // One access record per request, so a refused turn is diagnosable. No
            // request payload and no credential reaches it.
            tracing::debug!(
                operation = UPDATE_PROFILE_WITH_AGENT,
                service = ctx.identity().signing_name(),
                method = ctx.parts().method.as_str(),
                status = response.status().as_u16(),
                "served an event-stream request"
            );
            response
        })
    }
}

/// This operation's 400 refusal, as the modelled `ValidationException` it declares.
///
/// Reached only for a request the protocol's codec could not READ at all: a value
/// outside a declared constraint is refused before [`UpdateProfileWithAgent::respond`]
/// runs.
fn validation(message: impl Into<String>) -> UpdateProfileWithAgentError {
    UpdateProfileWithAgentErrorModeled::ValidationException(ValidationException {
        message: Some(message.into()),
    })
    .into()
}

/// This operation's 404, stating the profile and the version the turn resolved.
fn not_found(message: String) -> UpdateProfileWithAgentError {
    UpdateProfileWithAgentErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
        message: Some(message),
    })
    .into()
}

/// This operation's 500, for a store the handler could not reach at all.
fn internal_server(message: &str) -> UpdateProfileWithAgentError {
    UpdateProfileWithAgentErrorModeled::InternalServerException(InternalServerException {
        message: Some(message.to_string()),
    })
    .into()
}
