// SPDX-License-Identifier: CC0-1.0

//! The CONVERSATION `UpdateProfileWithAgent` carries a chat-based profile update
//! over, and the words the agent answers one turn with.
//!
//! `UpdateProfileWithAgent` "updates a data transformation profile using chat-based
//! interaction with an agent" and "supports multi-turn conversations for iteratively
//! customizing profiles", and its input says how a turn is addressed: a
//! `ConversationId` (absent on the first turn, so the service mints one and reports
//! it back — and a turn that names one continues the conversation held under that id
//! for that profile, so an id no record holds resolves nothing), and an
//! `InputMessage` whose `Type` is either `normal` or
//! `confirmation_response`. Its `EndOfOutputEvent` says what a turn hands back: a
//! final response, the `Options` the next turn may answer with, and the conversation
//! the turn belonged to. Those four members together describe one loop — the agent
//! proposes a change and offers the options that answer it, and the turn that answers
//! one of them is the turn that WRITES — which is the loop this module holds the state
//! for. The conversation is the only thing a turn needs that is not in the profile
//! record, so it lives here beside it, in the same store every other handler reads.
//!
//! # What a confirmed turn writes, and what it does not
//!
//! No language model runs in this emulator, so the agent authors no mapping content:
//! a confirmed turn stamps the DRAFT's `ChangeDescription` with its own account of the
//! change and the instant it ran — both read back by
//! `GetDataTransformationProfile` and `ListDataTransformationProfileVersions` — and
//! leaves the DRAFT's files exactly as they stand. A caller that has mapping content
//! to store sends it to `UpdateDataTransformationProfile`, which replaces the DRAFT
//! with content the request carries. Nothing here invents mapping files, matches on
//! the caller's text, or echoes it back: the message `Body` is `@sensitive`, so a
//! turn retains its SIZE and its `Type` and never its text, and every sentence the
//! agent answers with is composed from the stored profile and the stored conversation.

use std::time::{Duration, SystemTime};

use super::{
    ComAmazonawsHealthlakeHealthlakeState, DataTransformationProfile, DRAFT_PROFILE_VERSION,
};

/// How long a conversation stays resumable after its last turn.
///
/// AWS documents no lifetime for a data transformation agent conversation, so this
/// span is CHOSEN: long enough that a caller reading the agent's question, deciding,
/// and answering it still lands in the same conversation, and short enough that
/// abandoned chats cannot accumulate. Like every other due instant in this service it
/// is stored and swept by the next call, never slept on.
pub const AGENT_CONVERSATION_RETENTION: Duration = Duration::from_secs(900);

/// The most conversations this emulator holds at once.
///
/// A ceiling, not a claim about any AWS quota: it exists so a loop of first turns
/// cannot grow the emulator's heap without bound. Past it the STALEST conversation is
/// evicted rather than the new turn being refused — a conversation is an ephemeral
/// chat rather than a resource a caller can address again, so the same treatment the
/// idempotency tokens get is the right one.
pub const MAX_AGENT_CONVERSATIONS: usize = 100;

/// The `AgentInputMessageType` value of a turn that asks the agent for something, as
/// the modelled enum spells it on the wire
/// (`smithy_query(query="shape",shape="AgentInputMessageType")`: `normal`,
/// `confirmation_response`). Membership is the model's rule, enforced by the boundary
/// validator; these constants are only what the handler branches on.
pub const AGENT_MESSAGE_TYPE_NORMAL: &str = "normal";

/// The `AgentInputMessageType` value of a turn that ANSWERS the options the agent
/// offered, from the same modelled enum.
pub const AGENT_MESSAGE_TYPE_CONFIRMATION_RESPONSE: &str = "confirmation_response";

/// The option a caller answers to APPLY the change the agent proposed.
///
/// An option is the agent's own wording, so it is stated once, here, and the stored
/// conversation carries the options it offered — a confirmation is matched against
/// THOSE rather than against this constant, so a conversation opened before the
/// wording changed is still answerable.
pub const AGENT_APPLY_OPTION: &str = "Yes, record this change on the DRAFT";

/// The option a caller answers to leave the DRAFT alone.
pub const AGENT_DISCARD_OPTION: &str = "No, leave the DRAFT unchanged";

/// One option the agent offered, and whether answering it applies the change.
#[derive(Debug, Clone)]
pub struct AgentChatOption {
    /// The text the agent offered and the answer has to name.
    pub label: String,
    /// Whether this option is the one that writes.
    pub applies: bool,
}

/// The change the agent proposed and is waiting for an answer to.
#[derive(Debug, Clone)]
pub struct AgentPendingChange {
    /// The options offered with the proposal, in the order they were offered.
    pub options: Vec<AgentChatOption>,
}

impl AgentPendingChange {
    /// The option labels, as the `Options` member of an `EndOfOutputEvent` carries
    /// them.
    pub fn option_labels(&self) -> Vec<String> {
        self.options
            .iter()
            .map(|option| option.label.clone())
            .collect()
    }

    /// The option `answer` names, or `None` when it names none of them.
    ///
    /// Matched on the label the agent offered, ignoring surrounding whitespace and
    /// ASCII case, because that is how a caller re-sends a label it was handed.
    pub fn option_answered(&self, answer: &str) -> Option<&AgentChatOption> {
        let answer = answer.trim();
        self.options
            .iter()
            .find(|option| option.label.eq_ignore_ascii_case(answer))
    }
}

/// One multi-turn conversation between a caller and the data transformation agent.
#[derive(Debug, Clone)]
pub struct AgentConversation {
    /// The `ConversationId` every turn of this conversation reports back.
    pub id: String,
    /// The profile this conversation is about: the one its FIRST turn named.
    ///
    /// A conversation "iteratively customiz[es]" one profile, so it belongs to that
    /// profile — which is why a turn naming it about another profile resolves no
    /// conversation at all.
    pub profile_id: String,
    /// How many turns this conversation has served. Counted rather than kept as a
    /// transcript: the caller's message is `@sensitive` and nothing here needs its
    /// text, so the turn count is what the agent reads back off the conversation.
    pub turns: u64,
    /// The change the agent is waiting for an answer to, or `None` when it is waiting
    /// for nothing.
    pub pending: Option<AgentPendingChange>,
    /// When the last turn of this conversation was served — what
    /// [`AGENT_CONVERSATION_RETENTION`] runs from.
    pub last_turn_at: SystemTime,
}

impl AgentConversation {
    /// A conversation about `profile_id` with no turns served yet, as of `at`.
    pub fn new(id: String, profile_id: String, at: SystemTime) -> Self {
        Self {
            id,
            profile_id,
            turns: 0,
            pending: None,
            last_turn_at: at,
        }
    }
}

/// Why a turn naming a `ConversationId` could not continue that conversation.
///
/// Two different answers, because they are two different situations: an id NO record
/// holds names nothing at all, while an id the service does hold names a conversation
/// about ANOTHER profile — a conversation that exists and is simply not this turn's
/// to continue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentConversationRefusal {
    /// No conversation is held under the id the request carried: it was never served,
    /// or its retention has run out ([`AGENT_CONVERSATION_RETENTION`]).
    NotFound,
    /// A conversation IS held under that id, about another profile.
    OtherProfile,
}

impl ComAmazonawsHealthlakeHealthlakeState {
    /// Drop every conversation whose retention the clock has ALREADY run out,
    /// measured at `now`.
    ///
    /// The sweep a turn runs before it resolves the conversation it names, so an
    /// expired chat is gone whether or not anybody looked at it — the same way this
    /// service materialises every other due instant. Bounded by
    /// [`MAX_AGENT_CONVERSATIONS`].
    pub fn sweep_agent_conversations(&mut self, now: SystemTime) {
        self.agent_conversations.retain(|_, conversation| {
            conversation
                .last_turn_at
                .checked_add(AGENT_CONVERSATION_RETENTION)
                .is_none_or(|expires_at| expires_at > now)
        });
    }

    /// The conversation this turn belongs to, counted as served, or why the request's
    /// `ConversationId` is not a conversation of `profile_id`.
    ///
    /// A turn that names NO `ConversationId` is the first turn of a new conversation,
    /// so one is minted for the profile the turn is about ("supports multi-turn
    /// conversations", with `EndOfOutputEvent$ConversationId` reporting the id back).
    /// A turn that NAMES one continues the conversation held under that id — and only
    /// that conversation: an id no record holds (never served, or swept by
    /// [`Self::sweep_agent_conversations`]) resolves nothing, and an id held for
    /// another profile is a conversation this turn may not continue, so neither opens
    /// a conversation behind the id the caller sent. Minting past
    /// [`MAX_AGENT_CONVERSATIONS`] evicts the stalest conversation first.
    pub fn agent_conversation_turn(
        &mut self,
        requested: Option<&str>,
        profile_id: &str,
        at: SystemTime,
    ) -> Result<&mut AgentConversation, AgentConversationRefusal> {
        let id = match requested {
            Some(requested) => {
                let Some(held) = self.agent_conversations.get(requested) else {
                    return Err(AgentConversationRefusal::NotFound);
                };
                if held.profile_id != profile_id {
                    return Err(AgentConversationRefusal::OtherProfile);
                }
                requested.to_string()
            }
            None => {
                self.evict_stalest_agent_conversations();
                let id = self.next_conversation_id(at);
                self.agent_conversations.insert(
                    id.clone(),
                    AgentConversation::new(id.clone(), profile_id.to_string(), at),
                );
                id
            }
        };
        let conversation = self
            .agent_conversations
            .get_mut(&id)
            .ok_or(AgentConversationRefusal::NotFound)?;
        conversation.turns = conversation.turns.saturating_add(1);
        conversation.last_turn_at = at;
        Ok(conversation)
    }

    /// Make room for one more conversation by dropping the stalest ones, so the store
    /// never holds more than [`MAX_AGENT_CONVERSATIONS`].
    fn evict_stalest_agent_conversations(&mut self) {
        while self.agent_conversations.len() >= MAX_AGENT_CONVERSATIONS {
            let stalest = self
                .agent_conversations
                .values()
                .min_by_key(|conversation| conversation.last_turn_at)
                .map(|conversation| conversation.id.clone());
            match stalest {
                Some(id) => {
                    self.agent_conversations.remove(&id);
                }
                // Unreachable while the map is non-empty, which the loop condition
                // guarantees for any non-zero ceiling; breaking keeps it finite.
                None => break,
            }
        }
    }

    /// Mint an unused `ConversationId`: the 36-character UUID form, which is exactly
    /// the modelled `ConversationIdString` maximum (len 1..36) and the form the
    /// service's other caller-visible opaque handles take.
    fn next_conversation_id(&mut self, at: SystemTime) -> String {
        loop {
            self.agent_conversations_started += 1;
            let id = super::uuid_form(&super::mint_hex_id(
                CONVERSATION_MINT_DOMAIN,
                self.agent_conversations_started,
                at,
            ));
            if !self.agent_conversations.contains_key(&id) {
                return id;
            }
        }
    }
}

/// The mint domain a `ConversationId` is derived under, so it cannot collide with a
/// resource identifier minted for the same counter and instant.
const CONVERSATION_MINT_DOMAIN: &str = "agent-conversation/";

/// What the agent reports it is doing before it answers a turn.
///
/// Composed from the stored profile and the stored conversation — the version the
/// turn resolved, how much content that version holds, and which turn of the
/// conversation this is — so a caller reading the stream learns the state the answer
/// was computed from.
pub fn agent_review_progress_message(profile: &DataTransformationProfile, turn: u64) -> String {
    format!(
        "Turn {turn}: reading version {DRAFT_PROFILE_VERSION} of profile '{}', which holds {} \
         mapping file(s) and {} bytes of content.",
        profile.id,
        profile.draft.mapping.len(),
        profile.draft.retained_bytes()
    )
}

/// The question the agent asks a `normal` turn, with the options that answer it.
///
/// It states what the agent will do if the change is confirmed, which is what the
/// confirmed turn then does: stamp the DRAFT. It quotes nothing of the caller's
/// message — that member is `@sensitive`.
pub fn agent_proposal_message(profile: &DataTransformationProfile) -> String {
    format!(
        "Profile '{}' is at version {DRAFT_PROFILE_VERSION}, holding {} mapping file(s). I can \
         record this change against that DRAFT and stamp it as the profile's latest update. \
         Send a '{AGENT_MESSAGE_TYPE_CONFIRMATION_RESPONSE}' message naming one of the options \
         to decide.",
        profile.name,
        profile.draft.mapping.len()
    )
}

/// What the agent answers the turn that CONFIRMED the change, after the DRAFT was
/// stamped.
pub fn agent_applied_message(profile: &DataTransformationProfile) -> String {
    format!(
        "Recorded the change against version {DRAFT_PROFILE_VERSION} of profile '{}', which \
         holds {} mapping file(s).",
        profile.name,
        profile.draft.mapping.len()
    )
}

/// What the agent answers the turn that DECLINED the change. Nothing was written, so
/// the sentence says so.
pub fn agent_declined_message(profile: &DataTransformationProfile) -> String {
    format!(
        "Left version {DRAFT_PROFILE_VERSION} of profile '{}' unchanged.",
        profile.name
    )
}

/// The in-band error a `confirmation_response` gets when the conversation is waiting
/// for no answer.
pub fn agent_nothing_to_confirm_message(profile_id: &str, conversation_id: &str) -> String {
    format!(
        "Conversation '{conversation_id}' is not waiting for a confirmation about profile \
         '{profile_id}'. Send a '{AGENT_MESSAGE_TYPE_NORMAL}' message to ask for a change first."
    )
}

/// Why a turn naming a `ConversationId` no record holds is refused.
///
/// The fixed text is `Conversation not found: `; the only variable part is the
/// `ConversationId` the request carried, taken verbatim, because that is the handle
/// the caller has to correct.
pub fn conversation_not_found_message(conversation_id: &str) -> String {
    format!("Conversation not found: {conversation_id}")
}

/// Why a turn naming a conversation of ANOTHER profile is refused.
///
/// A conversation "iteratively customiz[es]" one profile, so continuing it against a
/// different profile is not a conversation that cannot be found — it is a request
/// whose two identifiers do not go together, which is what the sentence states. The
/// variable part is the `ProfileId` the request carried: the conversation's own
/// profile is not named, so nothing about another profile leaks into the answer.
pub fn conversation_of_other_profile_message(profile_id: &str) -> String {
    format!("profileId '{profile_id}' does not match the conversation's profile")
}

/// The in-band error a `confirmation_response` gets when it names none of the options
/// the agent offered. The offered options are repeated so the caller can answer; the
/// message the caller sent is not quoted, because it is `@sensitive`.
pub fn agent_unoffered_option_message(options: &[String]) -> String {
    format!(
        "That message names none of the options this conversation offered: [{}].",
        options.join(", ")
    )
}

/// The `ChangeDescription` a confirmed turn stamps on the DRAFT.
///
/// The agent's own account of the change, naming the conversation it was agreed in,
/// so a later read tells an agent-recorded change from one
/// `UpdateDataTransformationProfile` carried a description for. Bounded by
/// construction: the conversation id is at most the modelled 36 characters, well
/// inside the modelled `ChangeDescription` maximum of 1000.
pub fn agent_change_description(conversation_id: &str) -> String {
    format!("Recorded by the data transformation agent in conversation '{conversation_id}'")
}

/// The `AgentErrorType` value an in-band `ErrorEvent` reports a bad confirmation
/// under, as the modelled enum spells it
/// (`smithy_query(query="shape",shape="AgentErrorType")`).
pub const AGENT_ERROR_VALIDATION: &str = "ValidationException";
