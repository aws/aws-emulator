// SPDX-License-Identifier: CC0-1.0

//! @op InvokeInlineAgent
//!
//! `POST /agents/{sessionId} -> 200`. Serves one invocation of an agent the REQUEST
//! composes — "Invokes an inline Amazon Bedrock agent using the configurations you provide
//! with the request" — as a `vnd.amazon.eventstream` body: the `chunk` event the modelled
//! `InlineAgentResponseStream` declares for the agent's answer.
//!
//! ## What is real here
//!
//! * there is NO stored agent to resolve: the agent is composed by the call, so what is
//!   resolved instead is the `foundationModel` it orchestrates with, read in the plane that
//!   owns the model catalogue (`crate::state::inline_agent`) — an identifier naming no model
//!   this emulator publishes is the modelled `ValidationException` (400) or
//!   `ResourceNotFoundException` (404), exactly as the inference plane classifies it, and
//!   never an empty stream. What the reference does NOT have to do is serve inference: no
//!   model is invoked here, so a model past its end of life still composes an inline agent,
//!   while one of a non-text modality — which generates no turn of a conversation at all —
//!   is refused;
//! * the `sessionId` label is checked against the model before anything is looked up, so a
//!   malformed one is the modelled `ValidationException` (400);
//! * the caller's permission for `bedrock:InvokeInlineAgent` on that model is decided before
//!   any store is read, so an unauthorized call is answered the same way whether or not the
//!   model exists;
//! * the CONVERSATION is this plane's own store: the caller's own `sessionId` opens one,
//!   reusing it continues the same conversation, `endSession` ends it, and one idle past its
//!   `idleSessionTTLInSeconds` is released — which is why the answer can report which turn
//!   of which conversation was served.
//!
//! NO LANGUAGE MODEL RUNS IN THIS EMULATOR, and nothing here pretends otherwise. The
//! `chunk` events carry the notice the Bedrock Runtime plane of this same emulator answers
//! every inference surface with, extended with the facts of the invocation that was really
//! served (`crate::state::inline_agent::completion_notice`): the model the agent was
//! composed against, the turn and conversation, and the capabilities the request configured
//! that went unused. The caller's own `inputText` is never handed back as if an agent had
//! replied to it, and a chunk carries no `attribution`: no knowledge base was queried, so
//! there is no source to cite.
//!
//! ## What it refuses rather than answers
//!
//! A leg this emulator cannot compute is REFUSED with the modelled `ValidationException`,
//! because a plausible-looking success is worse than an error: `guardrailConfiguration` and
//! `streamingConfigurations.applyGuardrailInterval` (no guardrail policy is evaluated, so
//! neither an intervention nor its absence can be reported), `promptOverrideConfiguration`
//! and `promptCreationConfigurations` (both change a prompt nothing reads),
//! `inlineSessionState` (its attributes replace those placeholders and its
//! `returnControlInvocationResults` answer an invocation this service returned control for
//! nowhere), `collaborators` / `collaboratorConfigurations` / a non-`DISABLED`
//! `agentCollaboration` (no collaboration is orchestrated) and `customOrchestration` /
//! `orchestrationType: CUSTOM_ORCHESTRATION` (a custom orchestration runs a function this
//! emulator invokes nothing of).
//!
//! `actionGroups` and `knowledgeBases` are SERVED: each configures a capability the agent
//! uses only once its model elicits an action or a query, and AWS answers such a request
//! with a plain `chunk` whenever none is elicited — so the configuration does not change
//! what this invocation can answer, and the notice states how many of each were configured
//! and that none was used. `enableTrace` is served for the opposite reason: it adds a
//! report ALONGSIDE the answer rather than changing it, so the request is answered and the
//! notice states that the trace carries no step, because a trace reports the orchestration
//! steps a model took and no model took any — a `trace` event is never invented for steps
//! nothing performed. `bedrockModelConfigurations` names a latency profile, which does not
//! change what an answer says, and `streamingConfigurations.streamFinalResponse` is
//! honoured as the FRAMING it is: the same notice delivered in several `chunk` events
//! instead of one.
//!
//! This operation is hand-authored rather than generated: its output is a `@streaming`
//! union, which the op-stub generator declines (`src/ops/SKIPPED_STUBS.md`) and whose
//! generated `ErasedOperation` shim would decline the request. The framing and the routing
//! seam live in `service_common::event_stream`, the shapes in
//! `super::types::invoke_inline_agent_types`, and `crate::service` routes a request here
//! through `Service::override_operation`.
//!
//! ## Input
//!
//! * `sessionId` (SessionId, len 2..100, pattern `^[0-9a-zA-Z._:-]+$`, `@httpLabel`)
//!   **required** — the conversation the turn is served in.
//! * `foundationModel` (ModelIdentifier, len 1..2048) **required** — the model the agent
//!   orchestrates with.
//! * `instruction` (Instruction, len 40.., `@sensitive`) **required**.
//! * `customerEncryptionKeyArn` (KmsKeyArn), `idleSessionTTLInSeconds` (SessionTTL, range
//!   60..3600), `agentName` (Name, `@sensitive`), `agentCollaboration`
//!   (AgentCollaboration), `orchestrationType` (OrchestrationType), `inputText` (InputText,
//!   len ..25000000, `@sensitive`), `endSession` (Boolean), `enableTrace` (Boolean).
//! * The structured members — `actionGroups`, `knowledgeBases`, `guardrailConfiguration`,
//!   `promptOverrideConfiguration`, `collaboratorConfigurations`, `collaborators`,
//!   `inlineSessionState`, `streamingConfigurations`, `promptCreationConfigurations`,
//!   `bedrockModelConfigurations` and `customOrchestration`.
//!
//! ## Output
//!
//! * `completion` (InlineAgentResponseStream, `@httpPayload`, `@streaming`) **required** —
//!   the body.
//! * `contentType` (`@httpHeader x-amzn-bedrock-agent-content-type`) **required**,
//!   `sessionId` (`@httpHeader x-amz-bedrock-agent-session-id`) **required**.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403), `BadGatewayException` (HTTP 502),
//!   `ConflictException` (HTTP 409), `DependencyFailedException` (HTTP 424),
//!   `InternalServerException` (HTTP 500), `ResourceNotFoundException` (HTTP 404),
//!   `ServiceQuotaExceededException` (HTTP 400), `ThrottlingException` (HTTP 429),
//!   `ValidationException` (HTTP 400) — input validation failed.

#[allow(unused_imports)]
use super::types::*;

use aws_smithy_runtime_api::http::{Request, Response};
use aws_smithy_schema_server::ProtocolError;
use service_common::event_stream::{
    error_response, event_stream_response, frame_event, read_request, EventStreamOperation,
    RequestRefusal,
};
use smithy_server_runtime::{BoxFuture, OperationContext, Refusal, ServerHttpProtocol, Service};

use crate::state::inline_agent as inline;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice.json");

/// The operation's MODELED errors; `#[operation_error]` accepts only variants the model
/// declares for it. No `Scaffolded` wrapper: this operation is implemented, so the 501
/// channel has nothing to carry.
#[smithy_macros::operation_error("com.amazonaws.bedrockagentruntime#InvokeInlineAgent")]
#[derive(Debug)]
pub enum InvokeInlineAgentError {
    AccessDeniedException(AccessDeniedException),
    BadGatewayException(BadGatewayException),
    ConflictException(ConflictException),
    DependencyFailedException(DependencyFailedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's modelled 400 `ValidationException`, naming the member that was wrong
/// and what it requires — the caller has to fix the request from this text alone.
fn validation(message: impl Into<String>) -> InvokeInlineAgentError {
    InvokeInlineAgentError::ValidationException(ValidationException {
        message: Some(message.into()),
    })
}

/// This operation's modelled 404 `ResourceNotFoundException`.
fn not_found(message: String) -> InvokeInlineAgentError {
    InvokeInlineAgentError::ResourceNotFoundException(ResourceNotFoundException {
        message: Some(message),
    })
}

/// This operation's modelled 500 `InternalServerException`: a store this emulator should
/// carry is not on the engine, which is never reported as a client fault.
fn internal(message: impl Into<String>) -> InvokeInlineAgentError {
    InvokeInlineAgentError::InternalServerException(InternalServerException {
        message: Some(message.into()),
        reason: None,
    })
}

/// This operation's modelled 400 `ServiceQuotaExceededException`, answered when a resource
/// guard of this emulator's store is reached.
fn quota(message: String) -> InvokeInlineAgentError {
    InvokeInlineAgentError::ServiceQuotaExceededException(ServiceQuotaExceededException {
        message: Some(message),
    })
}

/// The `InlineAgentResponseStream` member one part of the agent's answer travels under,
/// which the frame carries as its `:event-type`
/// (`smithy_query(query="shape",shape="InlineAgentResponseStream")`).
const CHUNK_EVENT: &str = "chunk";

/// The operation name, which names the IAM action this call is authorized against
/// (`bedrock:InvokeInlineAgent`). Declared here because this operation has no generated
/// `impl Operation` to carry a `NAME`.
const OPERATION_NAME: &str = "InvokeInlineAgent";

/// The member path the model reference travels in, so a refusal about it names the member
/// the caller actually carried.
const FOUNDATION_MODEL_MEMBER: &str = "foundationModel";

/// The `AgentCollaboration` value that asks for NO collaboration, which is the only one
/// this service serves (`smithy_query(query="shape",shape="AgentCollaboration")` ->
/// DISABLED, SUPERVISOR, SUPERVISOR_ROUTER).
const COLLABORATION_DISABLED: &str = "DISABLED";

/// The `OrchestrationType` that asks for an orchestration driven by the caller's own
/// function (`smithy_query(query="shape",shape="OrchestrationType")` -> DEFAULT,
/// CUSTOM_ORCHESTRATION).
const CUSTOM_ORCHESTRATION: &str = "CUSTOM_ORCHESTRATION";

/// The `InvokeInlineAgent` operation.
///
/// Owns the shared engine (the constructor seam). Held by `crate::service` behind an
/// `EventStreamRoute`, because the generated routing table cannot serve a
/// `@streaming`-output operation.
pub struct InvokeInlineAgent<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> InvokeInlineAgent<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> InvokeInlineAgent<Eng> {
    /// The whole request once it has been read: authorize it, resolve the model the agent
    /// orchestrates with, refuse what cannot be computed, record the turn and frame the
    /// answer.
    fn respond(
        &self,
        protocol: &dyn ServerHttpProtocol,
        input: &InvokeInlineAgentInput,
        ctx: &OperationContext<'_>,
    ) -> Result<Response, InvokeInlineAgentError> {
        // Authorization is decided against the model the agent orchestrates with — the one
        // resource this call names, and the resource an identity-based policy for
        // `bedrock:InvokeInlineAgent` can scope to — and before any store is read, so an
        // unauthorized call is answered the same way whether or not that model exists.
        let resource = crate::state::target_model::model_resource_arn(ctx, &input.foundation_model);
        if let Err(denied) = crate::state::authorize(&*self.engine, ctx, OPERATION_NAME, &resource)
        {
            return Err(InvokeInlineAgentError::AccessDeniedException(denied));
        }

        let model_id = inline::resolve_model(
            &*self.engine,
            FOUNDATION_MODEL_MEMBER,
            &input.foundation_model,
        )
        .map_err(|refusal| match refusal {
            inline::OrchestrationRefusal::StoreUnavailable => {
                internal("the model catalogue is unavailable")
            }
            inline::OrchestrationRefusal::InvalidModel(message) => validation(message),
            inline::OrchestrationRefusal::ModelNotFound(message) => not_found(message),
        })?;

        // Only once the model has RESOLVED is a leg this service cannot compute refused: a
        // reference naming no model is refused whatever else the request carries, so
        // resolution comes first and this is what a caller whose model exists is told.
        unservable_leg(input)?;

        let served = self.record_turn(input, &model_id)?;
        let notice = inline::completion_notice(&served);
        // `streamFinalResponse` is about FRAMING: with it the one answer is delivered in
        // several chunks, whose concatenation is the same text a single chunk carries.
        let parts = if streams_final_response(input) {
            inline::streamed_chunks(&notice)
        } else {
            vec![notice]
        };
        let mut frames = Vec::new();
        for part in parts {
            let frame = frame_event(
                protocol,
                CHUNK_EVENT,
                false,
                &InlineAgentPayloadPart {
                    bytes: Some(part.into_bytes()),
                    // No knowledge base was queried on this path, so there is no retrieved
                    // source for the chunk to be attributed to.
                    attribution: None,
                },
                &InlineAgentPayloadPart::SCHEMA,
            )
            .map_err(|error| {
                internal(format!(
                    "the {CHUNK_EVENT} event could not be framed: {error}"
                ))
            })?;
            frames.extend_from_slice(&frame);
        }

        event_stream_response(
            protocol,
            OPERATION_NAME,
            &InvokeInlineAgentOutput {
                content_type: crate::state::agent_invocation::RESPONSE_CONTENT_TYPE.to_string(),
                session_id: served.session_id.clone(),
            },
            &InvokeInlineAgentOutput::SCHEMA,
            frames,
        )
        .map_err(|error| internal(format!("the response could not be serialized: {error}")))
    }

    /// Record the served turn in the conversation the request's `sessionId` addresses,
    /// opening one when this store holds none under that id, and answer the facts of the
    /// invocation the response reports.
    ///
    /// The expiry the clock already owes is materialised FIRST, so an id whose conversation
    /// has idled out begins a new one — which is what the member documents — and the lookup,
    /// the write and the ending all happen under ONE hold of the store, so the turn the
    /// answer reports is the turn this store really holds.
    fn record_turn(
        &self,
        input: &InvokeInlineAgentInput,
        model_id: &str,
    ) -> Result<inline::ServedInvocation, InvokeInlineAgentError> {
        let store = crate::state::service_state(&*self.engine)
            .ok_or_else(|| internal("the inline agent session store is unavailable"))?;
        let at = crate::state::now();
        let mut guard = store.lock();
        inline::expire_idle_conversations(&mut guard.inline_agent_sessions, at);
        let sessions = &mut guard.inline_agent_sessions;
        // A request whose id this store holds none of OPENS a conversation, so what is
        // bounded there is the store; a request that continues one is bounded by the turns
        // that conversation already holds.
        if !sessions.contains_key(&input.session_id)
            && sessions.len() >= inline::MAX_INLINE_AGENT_SESSIONS
        {
            return Err(quota(
                crate::state::knowledge_base_session::session_quota_message(
                    inline::MAX_INLINE_AGENT_SESSIONS,
                    "inline agent sessions",
                ),
            ));
        }
        let session = sessions
            .entry(input.session_id.clone())
            .or_insert_with(|| {
                inline::InlineAgentSession::opened(
                    input.session_id.clone(),
                    input
                        .idle_session_ttl_in_seconds
                        .unwrap_or(inline::DEFAULT_IDLE_SESSION_TTL_SECONDS),
                    input.customer_encryption_key_arn.clone(),
                    at,
                )
            });
        if session.turns.len() >= inline::MAX_TURNS_PER_INLINE_AGENT_SESSION {
            return Err(quota(
                crate::state::knowledge_base_session::session_quota_message(
                    inline::MAX_TURNS_PER_INLINE_AGENT_SESSION,
                    "turns in one inline agent session",
                ),
            ));
        }
        // An inline agent is composed afresh by every request, so the conversation is
        // maintained under the configuration of its latest turn: a turn that names an idle
        // lifetime or a key is the one the conversation is kept under from then on.
        if let Some(ttl) = input.idle_session_ttl_in_seconds {
            session.idle_ttl_seconds = ttl;
        }
        if input.customer_encryption_key_arn.is_some() {
            session.encryption_key_arn = input.customer_encryption_key_arn.clone();
        }
        session.record(inline::InlineAgentTurn {
            input_characters: input
                .input_text
                .as_deref()
                .unwrap_or_default()
                .chars()
                .count(),
            model_id: model_id.to_string(),
            at,
        });
        let turn = session.turns.len();
        let session_ended = input.end_session == Some(true);
        if session_ended {
            // Ending the session releases the conversation, so a later request under the
            // same id begins a new one.
            sessions.remove(&input.session_id);
        }
        Ok(inline::ServedInvocation {
            session_id: input.session_id.clone(),
            model_id: model_id.to_string(),
            agent_name: input.agent_name.clone(),
            turn,
            action_groups: input.action_groups.as_ref().map_or(0, Vec::len),
            knowledge_bases: input.knowledge_bases.as_ref().map_or(0, Vec::len),
            session_ended,
            trace_requested: input.enable_trace == Some(true),
        })
    }
}

/// Whether the request asked for the final response to be STREAMED, which is the one thing
/// `streamingConfigurations` decides here: "Specifies whether to enable streaming for the
/// final response. This is set to `false` by default"
/// (`smithy_query(query="shape",shape="StreamingConfigurations")`).
fn streams_final_response(input: &InvokeInlineAgentInput) -> bool {
    input
        .streaming_configurations
        .as_ref()
        .is_some_and(|streaming| streaming.stream_final_response)
}

/// The first leg of the request this service cannot compute, as the sentence the modelled
/// `ValidationException` states. See the module docs for why each one is refused.
fn unservable_leg(input: &InvokeInlineAgentInput) -> Result<(), InvokeInlineAgentError> {
    if input.guardrail_configuration.is_some() {
        return Err(validation(
            "guardrailConfiguration: it names the guardrail the invocation is governed by, \
             and this service evaluates no guardrail policy, so neither an intervention nor \
             its absence can be reported; omit guardrailConfiguration to take the answer \
             ungoverned",
        ));
    }
    if input
        .streaming_configurations
        .as_ref()
        .is_some_and(|streaming| streaming.apply_guardrail_interval.is_some())
    {
        return Err(validation(
            "streamingConfigurations.applyGuardrailInterval: it is how often a guardrail is \
             applied while the response is generated, and this service evaluates no \
             guardrail policy, so neither an intervention nor its absence can be reported; \
             omit it to take the response ungoverned",
        ));
    }
    if input.prompt_override_configuration.is_some() {
        return Err(validation(
            "promptOverrideConfiguration: it replaces the prompt templates of the agent \
             sequence, and no prompt is read in this emulator, so an override cannot change \
             what this invocation answers; omit it",
        ));
    }
    if input.prompt_creation_configurations.is_some() {
        return Err(validation(
            "promptCreationConfigurations: it selects which of the session's earlier turns \
             populate the agent prompt, and no prompt is read in this emulator, so the \
             selection cannot change what this invocation answers; omit it",
        ));
    }
    if input.inline_session_state.is_some() {
        return Err(validation(
            "inlineSessionState: its attributes replace the placeholders of a prompt nothing \
             reads here, its returnControlInvocationResults answer an action-group \
             invocation whose invocationId must match one an earlier InvokeInlineAgent \
             response returned and this service returns control for no action, and its files \
             are read by a code interpreter nothing runs; omit inlineSessionState to invoke \
             the inline agent with the request's own inputText",
        ));
    }
    if input.collaborators.is_some() || input.collaborator_configurations.is_some() {
        return Err(validation(
            "collaborators and collaboratorConfigurations: each names another agent this \
             invocation would hand work to, and nothing here orchestrates a collaboration, \
             so no collaborator is invoked and its answer cannot be part of this one; omit \
             both to invoke the inline agent alone",
        ));
    }
    if input
        .agent_collaboration
        .as_deref()
        .is_some_and(|collaboration| collaboration != COLLABORATION_DISABLED)
    {
        return Err(validation(format!(
            "agentCollaboration: it is how a supervisor agent coordinates the answers of its \
             collaborators, and nothing here orchestrates a collaboration; \
             {COLLABORATION_DISABLED} is the only value this service serves"
        )));
    }
    if input.custom_orchestration.is_some()
        || input
            .orchestration_type
            .as_deref()
            .is_some_and(|orchestration| orchestration == CUSTOM_ORCHESTRATION)
    {
        return Err(validation(format!(
            "customOrchestration and orchestrationType: {CUSTOM_ORCHESTRATION} drives the \
             agent from the function customOrchestration names, and this emulator invokes \
             no such function, so the orchestration it defines never runs; omit both to \
             invoke the inline agent with the default orchestration"
        )));
    }
    Ok(())
}

/// The literal first segment of this operation's modelled URI, `POST /agents/{sessionId}`.
const AGENTS_SEGMENT: &str = "agents";

/// Whether `req` addresses this operation's modelled route: `POST` on the one literal
/// segment plus the single `sessionId` label.
///
/// The seam that routes it ([`crate::service::EmulatedService::override_operation`]) asks
/// this, so the path this operation recognises is stated once, beside the operation. WHICH
/// Bedrock service claims such a request at all is `service_common::bedrock`'s: the
/// build-time plane binds `POST /agents/{agentId}/` on the same two segments, and the claim
/// rule splits the two.
pub fn addresses(req: &Request) -> bool {
    if req.method() != "POST" {
        return false;
    }
    let path = service_common::uri_path(req.uri());
    let mut segments = path.trim_start_matches('/').split('/');
    let (Some(AGENTS_SEGMENT), Some(label), None) =
        (segments.next(), segments.next(), segments.next())
    else {
        return false;
    };
    !label.is_empty()
}

/// Serving this operation is hand-authored because the generated `ErasedOperation` shim of
/// a `@streaming`-output operation declines the request (it cannot drive
/// `serialize_as_event_stream<P>` through an object-safe `run`).
///
/// The request half is the model's own: the framework's deserializer and constraint walk
/// over THIS operation's input, refused in this service's own wording
/// (`Service::override_refusal`), so a malformed URI label or body member is refused exactly
/// as one from a non-streaming operation of the same service.
impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> EventStreamOperation
    for InvokeInlineAgent<Eng>
{
    fn serve<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, Response> {
        Box::pin(async move {
            let definition = svc.definition();
            let protocol = definition.protocol();
            let input = match read_request::<InvokeInlineAgentInput>(
                protocol,
                req,
                &InvokeInlineAgentInput::SCHEMA,
            ) {
                Ok(input) => input,
                Err(RequestRefusal::Invalid(violations)) => {
                    let chosen = svc.override_refusal(&Refusal::Invalid(violations.clone()));
                    return definition.refusal_response(
                        ProtocolError::Invalid(violations),
                        chosen,
                        req,
                    );
                }
                // A body the protocol's codec could not parse has no violation form, so
                // the reader's own sentence is what the modelled 400 carries.
                Err(RequestRefusal::Unreadable(message)) => {
                    return error_response(protocol, &validation(message))
                }
            };
            match self.respond(protocol, &input, ctx) {
                Ok(response) => response,
                Err(error) => error_response(protocol, &error),
            }
        })
    }
}
