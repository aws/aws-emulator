// SPDX-License-Identifier: CC0-1.0

//! What an INLINE AGENT invocation reads and writes: the model it orchestrates with, the
//! conversation its `sessionId` addresses, and the text its response stream carries.
//!
//! An inline agent is composed BY THE REQUEST — "Invokes an inline Amazon Bedrock agent
//! using the configurations you provide with the request"
//! (`smithy_query(query="operation",operation="InvokeInlineAgent")`) — so there is no
//! stored agent to resolve and nothing here looks one up. What IS resolved is the
//! `foundationModel` the agent orchestrates with ([`resolve_model`]), read in the plane that
//! OWNS the model catalogue, so the models an inline agent can be composed against are
//! exactly the models this emulator serves inference from and the two planes cannot
//! disagree about which exist.
//!
//! What is WRITTEN is the conversation: "The unique identifier of the session. Use the same
//! value across requests to continue the same conversation"
//! (`InvokeInlineAgentRequest$sessionId`), so unlike the knowledge-base session of
//! [`super::knowledge_base_session`] the id is the CALLER's and a request carrying a fresh
//! one opens a conversation rather than being refused. Three rules follow, and all three
//! are the model's own:
//!
//! * a request carrying an id this store holds CONTINUES that conversation, appending its
//!   turn to the turns already recorded under it;
//! * a request whose `endSession` is `true` ends it — "Specifies whether to end the session
//!   with the inline agent or not" — and the conversation's turns are released with it;
//! * a conversation idle longer than its `idleSessionTTLInSeconds` is gone: "The number of
//!   seconds for which the inline agent should maintain session information. After this time
//!   expires, the subsequent `InvokeInlineAgent` request begins a new session. … If no
//!   conversation occurs during this time, the session expires and the data provided before
//!   the timeout is deleted" ([`expire_idle_conversations`]). That expiry is owed by the
//!   CLOCK at the same instant whether or not anybody calls, so it is materialised rather
//!   than decided per request.
//!
//! NO LANGUAGE MODEL RUNS IN THIS EMULATOR, and nothing here pretends otherwise. The text
//! the response stream carries is the notice the Bedrock Runtime plane of this same emulator
//! answers every inference surface with (`EMULATED_COMPLETION_FLOOR`, the one place that
//! sentence is worded), extended with the facts of the invocation that was really served:
//! the model the agent was composed against, which turn of which conversation it is, and
//! which of the capabilities the request configured went unused because no model elicited
//! them. It is never the caller's own `inputText` handed back as if an agent had replied,
//! and never prose synthesised to look like orchestration.

use std::collections::BTreeMap;

use ::aws_smithy_types::DateTime;

use ::service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::state as runtime_state;

/// Resource guard, NOT an AWS quota: the most inline-agent conversations this store holds at
/// once. The same guard the rest of this service's store declares
/// ([`crate::state::MAX_SESSIONS`]), and far above any plausible test or demo workload; past
/// it the invocation answers its modelled `ServiceQuotaExceededException` (HTTP 400) instead
/// of allocating.
pub const MAX_INLINE_AGENT_SESSIONS: usize = 1_000;

/// Resource guard: the most turns one inline-agent conversation records. See
/// [`MAX_INLINE_AGENT_SESSIONS`].
pub const MAX_TURNS_PER_INLINE_AGENT_SESSION: usize = 100;

/// The `idleSessionTTLInSeconds` a conversation is kept for when the opening request names
/// none.
///
/// CHOSEN, not documented: the API reference publishes only the member's valid range
/// (`range 60..3600`), so this is the emulator's own idle lifetime, inside that range.
pub const DEFAULT_IDLE_SESSION_TTL_SECONDS: i32 = 600;

/// How long a chunk of a STREAMED final response is, in characters.
///
/// The model documents the interval the response is chunked at on
/// `StreamingConfigurations$applyGuardrailInterval` — "By default, the guardrail interval is
/// set to 50 characters. If a larger interval is specified, the response will be generated
/// in larger chunks … Each chunk has at least 3 characters except for the last chunk"
/// (`smithy_query(query="shape",shape="StreamingConfigurations")`) — so the default interval
/// is what sizes a chunk when the request asks for the final response to be streamed and
/// configures no interval of its own.
pub const STREAMED_CHUNK_CHARACTERS: usize = 50;

/// One turn of a conversation with an inline agent: what the caller submitted, the model the
/// agent was composed against for it, and when it was served.
#[derive(Debug, Clone)]
pub struct InlineAgentTurn {
    /// How long the submitted `inputText` was, in characters — a fact about the request that
    /// was served, where the text itself is the caller's and is never handed back.
    pub input_characters: usize,
    /// The model the agent orchestrated with, as the catalogue spells its id.
    pub model_id: String,
    /// When the turn was served.
    pub at: DateTime,
}

/// One conversation with an inline agent, addressed by the `sessionId` the caller chose.
#[derive(Debug, Clone)]
pub struct InlineAgentSession {
    /// The `sessionId` the opening request carried.
    pub session_id: String,
    pub created_at: DateTime,
    /// When the last turn of the conversation was served, which is the instant the idle
    /// lifetime is measured from.
    pub last_used_at: DateTime,
    /// The idle lifetime the conversation is kept for, in seconds: the opening request's own
    /// `idleSessionTTLInSeconds`, or [`DEFAULT_IDLE_SESSION_TTL_SECONDS`] when it named none.
    pub idle_ttl_seconds: i32,
    /// The customer-managed key the opening request named, when it named one.
    pub encryption_key_arn: Option<String>,
    /// The turns taken in the conversation, oldest first.
    pub turns: Vec<InlineAgentTurn>,
}

impl InlineAgentSession {
    /// A conversation opened at `at` under the TTL and key its opening request named.
    pub fn opened(
        session_id: String,
        idle_ttl_seconds: i32,
        encryption_key_arn: Option<String>,
        at: DateTime,
    ) -> Self {
        Self {
            session_id,
            created_at: at,
            last_used_at: at,
            idle_ttl_seconds,
            encryption_key_arn,
            turns: Vec::new(),
        }
    }

    /// Record one served turn, which restarts the idle lifetime.
    pub fn record(&mut self, turn: InlineAgentTurn) {
        self.last_used_at = turn.at;
        self.turns.push(turn);
    }

    /// The instant the conversation's idle lifetime ends: the last turn plus the TTL it is
    /// maintained for.
    pub fn expires_at(&self) -> DateTime {
        DateTime::from_secs(
            self.last_used_at
                .secs()
                .saturating_add(i64::from(self.idle_ttl_seconds)),
        )
    }

    /// Whether the clock has already ended this conversation's idle lifetime at `at`.
    pub fn has_expired(&self, at: DateTime) -> bool {
        at.secs() >= self.expires_at().secs()
    }
}

/// Release every conversation whose idle lifetime the clock has already ended.
///
/// Owed by the clock at the same instant whether or not anybody calls, so materialising it
/// changes nothing a caller could otherwise have observed: the next request for an expired
/// id begins a new session either way, which is what the member documents.
pub fn expire_idle_conversations(
    sessions: &mut BTreeMap<String, InlineAgentSession>,
    at: DateTime,
) {
    sessions.retain(|_, session| !session.has_expired(at));
}

/// Why an inline agent cannot be composed against the model a request names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrchestrationRefusal {
    /// The catalogue is not on this engine, so nothing can be said about the model —
    /// answered with a modelled server error, never with a client fault.
    StoreUnavailable,
    /// Not an identifier this emulator can read as naming a model it offers: a client input
    /// fault, answered with `ValidationException` (HTTP 400).
    InvalidModel(String),
    /// It names a model or a resource this emulator does not hold: answered with
    /// `ResourceNotFoundException` (HTTP 404).
    ModelNotFound(String),
}

/// The model `foundation_model` names, as the Bedrock Runtime data plane's catalogue holds
/// it.
///
/// `foundationModel` is "The model identifier (ID) of the model to use for orchestration",
/// and every spelling of such a reference — the bare model id, a cross-region
/// inference-profile id, a `foundation-model` ARN and an ARN of an account-scoped inference
/// resource — is read by the plane that OWNS the catalogue (`recognise_model`), which also
/// decides which failures are a 400 and which a 404.
///
/// What this operation needs of the reference is that it NAMES a model this emulator
/// publishes, because that is what makes the answer's statement of which model the agent was
/// bound to true. It does not need the model to serve INFERENCE: no model is invoked here,
/// and "a model id stays a recognised identifier after its model reaches end of life", so a
/// model past its end of life composes an inline agent while an identifier naming no model
/// at all does not. A model of a non-text modality is refused: it generates no turn of a
/// conversation at all, whatever ran it.
///
/// `member` is the member path the reference travelled in, so a refusal names the member the
/// request actually carried.
///
/// READ-ONLY: the catalogue is published, not stored per account, so nothing is written.
pub fn resolve_model<Eng: Send + Sync + 'static>(
    engine: &Eng,
    member: &str,
    foundation_model: &str,
) -> Result<String, OrchestrationRefusal> {
    let Some(runtime) = runtime_state::service_state(engine) else {
        return Err(OrchestrationRefusal::StoreUnavailable);
    };
    let guard = runtime.lock();
    match guard.recognise_model(foundation_model) {
        Ok(entry) if entry.modality != runtime_state::ModelModality::Text => {
            Err(OrchestrationRefusal::InvalidModel(format!(
                "{member}: {} generates {}, not text, so it orchestrates no inline agent turn",
                entry.model_id,
                entry.modality.label()
            )))
        }
        Ok(entry) => Ok(entry.model_id.clone()),
        Err(runtime_state::ModelRejection::Invalid(detail)) => {
            Err(OrchestrationRefusal::InvalidModel(format!("{member}: {detail}")))
        }
        Err(runtime_state::ModelRejection::NotFound(detail)) => {
            Err(OrchestrationRefusal::ModelNotFound(detail))
        }
    }
}

/// The facts of one served invocation the response stream reports.
#[derive(Debug, Clone)]
pub struct ServedInvocation {
    /// The conversation the turn was served in.
    pub session_id: String,
    /// The model the agent orchestrated with, as the catalogue spells its id.
    pub model_id: String,
    /// The `agentName` the request composed the agent under, when it named one.
    pub agent_name: Option<String>,
    /// Which turn of the conversation this is, counting from one.
    pub turn: usize,
    /// How many action groups the request configured, none of which was invoked.
    pub action_groups: usize,
    /// How many knowledge bases the request configured, none of which was queried.
    pub knowledge_bases: usize,
    /// Whether this request ended the conversation.
    pub session_ended: bool,
    /// Whether the request asked for the orchestration TRACE, which carries no step here.
    pub trace_requested: bool,
}

/// The text the `chunk` events of a served invocation carry: that no model ran, the model
/// the agent was composed against, which turn of which conversation was served, which
/// configured capabilities went unused, whether a requested trace reports anything, and
/// whether the conversation was ended.
///
/// The caller's own `inputText` is NOT part of it: a prompt handed back would read as an
/// agent that had replied to it.
pub fn completion_notice(served: &ServedInvocation) -> String {
    let agent = match served.agent_name.as_deref() {
        Some(name) => format!("named {name}"),
        None => "this request composed".to_string(),
    };
    let mut notice = format!(
        "{} The inline agent {agent} is bound to {} and generated no response: this was \
         turn {} of session {}.",
        runtime_state::EMULATED_COMPLETION_FLOOR, served.model_id, served.turn, served.session_id
    );
    if served.action_groups > 0 || served.knowledge_bases > 0 {
        notice.push_str(&format!(
            " The {} action group(s) and {} knowledge base(s) this request configured went \
             unused: an action is carried out, and a knowledge base queried, only once a \
             model elicits one.",
            served.action_groups, served.knowledge_bases
        ));
    }
    if served.trace_requested {
        notice.push_str(
            " The trace this request enabled reports no step: a trace reports the \
             orchestration steps a model took, and no model took any.",
        );
    }
    if served.session_ended {
        notice.push_str(" The session was ended by this request.");
    }
    notice
}

/// `notice` split into the chunks a STREAMED final response is delivered in: successive
/// runs of [`STREAMED_CHUNK_CHARACTERS`] characters, the last one carrying what is left, so
/// the chunks concatenate back to exactly `notice`.
///
/// Split on CHARACTERS rather than bytes, because the interval the model documents is a
/// character count.
pub fn streamed_chunks(notice: &str) -> Vec<String> {
    let characters: Vec<char> = notice.chars().collect();
    if characters.is_empty() {
        return vec![String::new()];
    }
    characters
        .chunks(STREAMED_CHUNK_CHARACTERS)
        .map(|chunk| chunk.iter().collect())
        .collect()
}
