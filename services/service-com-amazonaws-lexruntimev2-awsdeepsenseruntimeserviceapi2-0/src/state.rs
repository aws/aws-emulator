// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-lexruntimev2-awsdeepsenseruntimeserviceapi2-0 service.
//!
//! The service is the Lex V2 *runtime*: its only resource is the conversation
//! session a client holds with one bot alias in one locale. `PutSession`,
//! `RecognizeText`, `RecognizeUtterance`, `GetSession` and `DeleteSession` are
//! five views of the one [`Session`] stored here, keyed by the four
//! `@httpLabel` values that address it.
//!
//! Beside the store itself, this module holds what more than one of those
//! operations needs: [`take_turn`], the one conversation turn `RecognizeText`
//! and `RecognizeUtterance` are two encodings of, [`headers`], the wire form of
//! the structured `x-amz-lex-*` headers `PutSession` and `RecognizeUtterance`
//! answer with, [`bot_model`], the built bot locale a call is served against,
//! [`bot_definition`], what that bot says about a dialog state a caller seeds,
//! and [`nlu`], the interpretation and dialog of one turn.

pub mod bot_definition;
pub mod bot_model;
pub mod headers;
pub mod nlu;

use crate::ops::types::{ActiveContext, Interpretation, Message, SessionState};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::time::{Duration, SystemTime};

/// The intent Amazon Lex V2 reports when no intent of the bot locale matches
/// the input: the fallback intent every locale carries, which the control plane
/// adds to a locale as it creates it.
pub const FALLBACK_INTENT: &str = "FallbackIntent";

/// `Close`: "indicates that there will not be a response from the user"
/// (`DialogAction$type` docs).
pub const DIALOG_ACTION_CLOSE: &str = "Close";

/// `Lex` itself interpreted the input, as opposed to `Bedrock`
/// (`Interpretation$interpretationSource`).
pub const INTERPRETATION_SOURCE: &str = "Lex";

/// The response content type a caller gets when it asks for none, and the one
/// value of `responseContentType` that is not speech: "if the value is
/// `text/plain;charset=utf-8`, Amazon Lex V2 returns text in the response"
/// (`RecognizeUtteranceRequest$responseContentType`, which lists every accepted
/// value).
pub const TEXT_CONTENT_TYPE: &str = "text/plain;charset=utf-8";

/// The prefix every speech `responseContentType` carries: all of the model's
/// speech values (`audio/mpeg`, `audio/ogg`, `audio/pcm`, `audio/*`) begin with
/// it, and so does every audio `requestContentType` it lists.
pub const AUDIO_PREFIX: &str = "audio/";

/// The most characters of utterance ONE turn interprets.
///
/// `RecognizeText` binds the utterance as `Text`, which the model bounds at
/// `len 1..1024`, and the framework refuses a longer one before the handler
/// runs. `RecognizeUtterance` carries the SAME utterance as a request payload,
/// which the model cannot bound, so the bound is applied here: one turn is one
/// turn whichever encoding carries it, and the work of interpreting it — the
/// words it is split into, and the comparisons against the locale's sample
/// utterances that each of them costs — is bounded by the same number for both.
pub const MAX_UTTERANCE_CHARS: usize = 1024;

/// Does this `responseContentType` ask for synthesized speech?
///
/// "If the value begins with `audio/`, Amazon Lex V2 returns speech in the
/// response" — which it synthesizes with Amazon Polly. Held here rather than in
/// either operation, because the two operations that take a
/// `responseContentType` must answer the same way for the same value; the
/// comparison is case-insensitive, as a media type is compared.
pub fn asks_for_speech(response_content_type: &str) -> bool {
    response_content_type
        .trim_start()
        .to_ascii_lowercase()
        .starts_with(AUDIO_PREFIX)
}

/// Per-service state for com-amazonaws-lexruntimev2-awsdeepsenseruntimeserviceapi2-0. A handler reaches it through [`service_state`].
///
/// The session map is BOUNDED, in two independent ways, because every entry in
/// it is created from caller input:
///
/// * it is swept of idle sessions on every call ([`IDLE_SESSION_TTL`]), which is
///   the retention the real service applies, and
/// * it refuses to grow past [`MAX_SESSIONS`] entries, so a caller looping over
///   fresh session ids is refused rather than served until the process dies.
///
/// Both bounds are applied in [`Self::session`] / [`Self::session_mut`], which
/// is the ONLY path that inserts, so the bound provably precedes the allocation.
#[derive(Debug, Default)]
pub struct ComAmazonawsLexruntimev2Awsdeepsenseruntimeserviceapi20State {
    /// Every live session, keyed by its bot / alias / locale / session id.
    /// Bounded: see this struct's own documentation.
    pub sessions: HashMap<SessionKey, Session>,
    /// The monotonic source the minted `originatingRequestId`s are derived from.
    request_ids_minted: u64,
}

/// How long a session survives without a request.
///
/// Amazon Lex V2 keeps "information about a user's conversation with the bot"
/// for the bot's `idleSessionTTLInSeconds`, which "you can specify between 60 (1
/// minute) and 86400 (24 hours) seconds" with a default of 300 seconds (5
/// minutes). A session is built here by the *runtime*, which holds no bot
/// configuration to read that setting from, so the documented default is the
/// retention applied — and an idle session is gone, exactly as it is in the real
/// service.
pub const IDLE_SESSION_TTL: Duration = Duration::from_secs(300);

/// The most sessions this emulator holds at once.
///
/// A resource bound, not a modelled quota: the four labels that key a session
/// are caller-chosen, so without a cap one client can allocate sessions until
/// the emulator runs out of memory. A request that would create one past the cap
/// is refused with the modelled `ThrottlingException` instead of allocating.
pub const MAX_SESSIONS: usize = 10_000;

/// The most bytes of attribute keys and values one session's map may hold.
///
/// Amazon Lex counts the session and request attributes of a conversation
/// against a 12 KB total, and the attributes here accumulate over the turns of a
/// session, so the same cap is what keeps one session's map bounded. It bounds
/// the entry count too: every entry costs at least one byte of key.
pub const MAX_ATTRIBUTE_BYTES: usize = 12 * 1024;

/// The session store is at [`MAX_SESSIONS`] and the request would add another.
#[derive(Debug, Clone, Copy)]
pub struct SessionStoreFull;

impl SessionStoreFull {
    /// What to tell the caller: a capacity refusal, with the cap in it.
    pub fn message(self) -> String {
        format!(
            "this emulator holds at most {MAX_SESSIONS} concurrent sessions and is at that \
             limit; delete a session or retry once one goes idle"
        )
    }
}

/// The attribute-name namespace Amazon Lex V2 keeps for itself.
///
/// "The namespace x-amz-lex: is reserved for special attributes. Don't create
/// any request attributes with the prefix x-amz-lex:" —
/// `PutSessionRequest$requestAttributes` and
/// `RecognizeTextRequest$requestAttributes`.
pub const RESERVED_ATTRIBUTE_PREFIX: &str = "x-amz-lex:";

/// Why a request's attribute map was refused. Both arms are the caller's to fix,
/// so a handler answers them with the modelled `ValidationException`.
#[derive(Debug, Clone)]
pub enum AttributesRefused {
    /// The map would grow past [`MAX_ATTRIBUTE_BYTES`].
    TooLarge {
        /// The member the caller has to fix.
        member: &'static str,
        /// The size the member's value would have reached.
        bytes: usize,
    },
    /// A key sits in the namespace Amazon Lex V2 reserves.
    ReservedKey {
        /// The member the caller has to fix.
        member: &'static str,
        /// The offending key.
        key: String,
    },
}

impl AttributesRefused {
    /// What to tell the caller: the member and the rule it broke, so the request
    /// can be fixed from the message alone.
    pub fn message(&self) -> String {
        match self {
            Self::TooLarge { member, bytes } => format!(
                "{member} would hold {bytes} bytes of keys and values; the attributes of a \
                 session must not exceed {MAX_ATTRIBUTE_BYTES} bytes in total"
            ),
            Self::ReservedKey { member, key } => format!(
                "{member} must not contain the key '{key}': the '{RESERVED_ATTRIBUTE_PREFIX}' \
                 namespace is reserved for Amazon Lex V2's own attributes"
            ),
        }
    }
}

/// The bytes of keys and values `stored` would hold once `supplied` is folded in.
fn merged_attribute_bytes(
    stored: Option<&HashMap<String, String>>,
    supplied: &HashMap<String, String>,
) -> usize {
    let mut merged: HashMap<&str, &str> = stored
        .map(|map| {
            map.iter()
                .map(|(key, value)| (key.as_str(), value.as_str()))
                .collect()
        })
        .unwrap_or_default();
    for (key, value) in supplied {
        merged.insert(key, value);
    }
    merged.iter().map(|(key, value)| key.len() + value.len()).sum()
}

/// Refuse, BEFORE anything is stored, a merge that would grow one session's
/// attributes past [`MAX_ATTRIBUTE_BYTES`].
pub fn check_attribute_bytes(
    member: &'static str,
    stored: Option<&HashMap<String, String>>,
    supplied: Option<&HashMap<String, String>>,
) -> Result<(), AttributesRefused> {
    let Some(supplied) = supplied else {
        return Ok(());
    };
    let bytes = merged_attribute_bytes(stored, supplied);
    if bytes > MAX_ATTRIBUTE_BYTES {
        return Err(AttributesRefused::TooLarge { member, bytes });
    }
    Ok(())
}

/// Check a `requestAttributes` map: it is bounded like any other attribute map,
/// and in addition it may not name a key in the reserved namespace, because
/// "the namespace x-amz-lex: is reserved for special attributes" and a caller is
/// told not to create any.
pub fn check_request_attributes(
    member: &'static str,
    stored: Option<&HashMap<String, String>>,
    supplied: Option<&HashMap<String, String>>,
) -> Result<(), AttributesRefused> {
    if let Some(supplied) = supplied {
        // Keys are compared case-insensitively: the prefix names a namespace,
        // and HTTP-style attribute names are not case-sensitive.
        if let Some(key) = supplied
            .keys()
            .find(|key| key.to_ascii_lowercase().starts_with(RESERVED_ATTRIBUTE_PREFIX))
        {
            return Err(AttributesRefused::ReservedKey {
                member,
                key: key.clone(),
            });
        }
    }
    check_attribute_bytes(member, stored, supplied)
}

impl ComAmazonawsLexruntimev2Awsdeepsenseruntimeserviceapi20State {
    /// Drop every session that has gone idle for longer than
    /// [`IDLE_SESSION_TTL`].
    ///
    /// This is work the CLOCK made due, not work a caller asked for: the
    /// sessions were already gone at this instant whether or not anybody called,
    /// so a reporting operation may materialise it too. It is what keeps the map
    /// from holding a session forever when no client ever deletes one.
    pub fn expire_idle_sessions(&mut self, now: SystemTime) {
        self.sessions
            .retain(|_, session| match now.duration_since(session.updated_at) {
                Ok(idle) => idle < IDLE_SESSION_TTL,
                // The clock went backwards, so no time has passed for this
                // session and it cannot have gone idle yet.
                Err(_) => true,
            });
    }

    /// The session `key` addresses, if it is still live. Expires idle sessions
    /// first, so a session past its TTL is reported as absent.
    pub fn session(&mut self, key: &SessionKey, now: SystemTime) -> Option<&Session> {
        self.expire_idle_sessions(now);
        self.sessions.get(key)
    }

    /// The session `key` addresses, created when absent — the ONE insertion
    /// path, so both bounds on the map are applied here.
    ///
    /// Idle sessions are expired first (that is what makes room), and a request
    /// that would push the map past [`MAX_SESSIONS`] is refused instead of
    /// allocating.
    pub fn session_mut(
        &mut self,
        key: SessionKey,
        now: SystemTime,
    ) -> Result<&mut Session, SessionStoreFull> {
        self.expire_idle_sessions(now);
        let fresh = !self.sessions.contains_key(&key);
        if fresh && self.sessions.len() >= MAX_SESSIONS {
            return Err(SessionStoreFull);
        }
        // "Amazon Lex V2 creates a session when the user starts a
        // conversation", and the session it creates carries the
        // `originatingRequestId` every state it reports is stamped with, so the
        // id is minted where a session comes into existence and lives as long as
        // the session does. A turn of a conversation that is already there mints
        // nothing — it is stamped with the id its own first turn earned — which
        // is why minting is done only for a key the map does not hold, and the
        // insertion below runs in exactly that case.
        let minted = if fresh {
            self.mint_originating_request_id()
        } else {
            String::new()
        };
        let session_id = key.session_id.clone();
        Ok(self
            .sessions
            .entry(key)
            .or_insert_with(|| Session::new(&session_id, now, minted)))
    }

    /// Mint an `originatingRequestId` no live session holds: "a unique
    /// identifier for a specific request" (`SessionState$originatingRequestId`,
    /// a `NonEmptyString`), in the hyphenated form Amazon Lex reports it in.
    ///
    /// The bytes are derived from a monotonic counter through a digest, so two
    /// sessions never share one, and the result is checked against the live
    /// sessions before it is handed out; the LAYOUT is
    /// [`service_common::uuid_from_random_bytes`]'s, so this module spells out
    /// no group boundaries of its own.
    fn mint_originating_request_id(&mut self) -> String {
        loop {
            self.request_ids_minted += 1;
            let digest = Sha256::digest(self.request_ids_minted.to_be_bytes());
            let mut bytes = [0u8; 16];
            bytes.copy_from_slice(&digest[..16]);
            let candidate = ::service_common::uuid_from_random_bytes(bytes);
            if !self
                .sessions
                .values()
                .any(|session| session.originating_request_id == candidate)
            {
                return candidate;
            }
        }
    }

    /// Remove the session `key` addresses, if it is still live.
    pub fn remove_session(&mut self, key: &SessionKey, now: SystemTime) -> Option<Session> {
        self.expire_idle_sessions(now);
        self.sessions.remove(key)
    }

    /// OPEN one turn of the conversation `key` addresses: charge the turn, fold
    /// in what the request supplied, and report the dialog state the utterance
    /// is then interpreted against.
    ///
    /// The first of the two steps [`take_turn`] drives — the interpretation
    /// itself runs between them, OUTSIDE this store's lock, so one caller's
    /// utterance is not matched inside the critical section every session of the
    /// service shares.
    ///
    /// `supplied_state` and `supplied_attributes` are the dialog state and
    /// request attributes the request carried.
    pub fn open_turn(
        &mut self,
        key: SessionKey,
        now: SystemTime,
        supplied_state: Option<SessionState>,
        supplied_attributes: Option<HashMap<String, String>>,
    ) -> Result<SessionState, TurnRefused> {
        // Refuse an attribute map this turn may not store BEFORE the session is
        // looked up for writing, so a refused turn leaves the conversation
        // exactly as it was — and a refused FIRST turn leaves no session behind
        // at all, so a following GetSession is still the modelled 404. These
        // maps accumulate over the turns of a session, which is why the bound is
        // checked against the merge with what is already stored (nothing, for a
        // session that does not exist yet) rather than against the request
        // alone.
        {
            let stored = self.session(&key, now);
            check_attribute_bytes(
                "sessionState.sessionAttributes",
                stored.and_then(|session| session.session_state.session_attributes.as_ref()),
                supplied_state
                    .as_ref()
                    .and_then(|supplied| supplied.session_attributes.as_ref()),
            )
            .map_err(TurnRefused::Attributes)?;
            check_request_attributes(
                "requestAttributes",
                stored.map(|session| &session.request_attributes),
                supplied_attributes.as_ref(),
            )
            .map_err(TurnRefused::Attributes)?;
        }

        // "Amazon Lex V2 creates a session when the user starts a
        // conversation": the first utterance of a session starts it, so an
        // absent session is created here rather than refused. `session_mut` is
        // the bounded insertion path: it expires idle sessions and refuses to
        // grow the store past its cap, so a caller looping over fresh session
        // ids is refused rather than served until the emulator runs out of
        // memory.
        let session = self.session_mut(key, now).map_err(TurnRefused::StoreFull)?;

        // Charge the turn FIRST, so the contexts already on the session pay for
        // this turn ("each request and response from the bot is a turn") while
        // the ones this request supplies below start on their full budget.
        session.spend_turn(now);

        if let Some(supplied) = supplied_state {
            session.merge_session_state(supplied, now);
        }
        if let Some(supplied) = supplied_attributes {
            merge_attributes(&mut session.request_attributes, supplied);
        }

        // What the utterance is interpreted against: the conversation as it
        // stands, with the dialog state the request supplied already folded in.
        Ok(session.session_state.clone())
    }

    /// RECORD the turn [`nlu::interpret`] decided on, and report what it left on
    /// the session.
    ///
    /// The second of [`take_turn`]'s two steps. The session was opened for this
    /// turn by [`Self::open_turn`]; it is looked up again through the one
    /// bounded insertion path, so a conversation another request deleted in
    /// between is started rather than resurrected silently.
    pub fn record_turn(
        &mut self,
        key: SessionKey,
        now: SystemTime,
        plan: nlu::TurnPlan,
    ) -> Result<Turn, TurnRefused> {
        let session = self.session_mut(key, now).map_err(TurnRefused::StoreFull)?;
        session.session_state.intent = Some(plan.intent);
        // "The next action that the bot should take in its interaction with the
        // user": the step the dialog reached, which is what a caller drives the
        // conversation off. A `Delegate` a request carried is an instruction —
        // "the next action is determined by Amazon Lex V2" — so what the turn
        // reports is the decision, not the instruction.
        session.session_state.dialog_action = Some(plan.dialog_action);
        // "A list of messages last sent to the user": the ones THIS turn sends —
        // a slot's prompt, or the intent's closing response — and none when the
        // bot has nothing authored to say at the step the dialog reached.
        session.messages = plan.messages;
        // What the recognition made of the utterance, written back onto the
        // session so a following GetSession reports the same interpretations.
        session.interpretations = plan.interpretations;

        Ok(Turn {
            session_id: session.session_id.clone(),
            session_state: session.visible_session_state(now),
            messages: session.messages.clone(),
            // The RESPONSE to a recognition also carries the fallback intent
            // Amazon Lex V2 inserts among its alternative intents: it is part of
            // the answer to this utterance, not of what the session holds, so a
            // later read of the session does not report it.
            interpretations: nlu::with_inserted(
                session.interpretations.clone(),
                plan.inserted_fallback,
            ),
            request_attributes: session.request_attributes.clone(),
        })
    }
}

/// Take ONE turn of the conversation `key` addresses, and report what the turn
/// left on the session.
///
/// This is the whole of what an utterance does to a session, held here because
/// the service offers the SAME turn through two encodings: `RecognizeText`,
/// which carries the input and the dialog state as JSON in the body, and
/// `RecognizeUtterance`, which carries them as a request payload and gzipped
/// headers. The two differ only in how a turn is spelled on the wire ("Sends
/// user input to Amazon Lex V2" is the documented purpose of both), so both
/// drive this one function and cannot drift apart.
///
/// `text` is the utterance the turn processes — bounded by
/// [`MAX_UTTERANCE_CHARS`] before it gets here — and `model` the built bot
/// locale it is interpreted against ([`bot_model::built_locale`]), which is what
/// makes the turn this bot's turn rather than a bot-independent one: the intent,
/// the slots it fills and the next dialog step all come out of
/// [`nlu::interpret`].
///
/// The store is locked TWICE, once to open the turn and once to record it, so
/// the interpretation — the only caller-driven work of a turn — runs outside the
/// critical section every session of this service shares, and no request can
/// hold that lock for longer than a map lookup and a clone.
pub fn take_turn(
    store: &::engine::ServiceState<ComAmazonawsLexruntimev2Awsdeepsenseruntimeserviceapi20State>,
    key: SessionKey,
    now: SystemTime,
    text: &str,
    model: &bot_model::BotLocaleModel,
    supplied_state: Option<SessionState>,
    supplied_attributes: Option<HashMap<String, String>>,
) -> Result<Turn, TurnRefused> {
    let opened = store
        .lock()
        .open_turn(key.clone(), now, supplied_state, supplied_attributes)?;
    let plan = nlu::interpret(model, text, &opened);
    store.lock().record_turn(key, now, plan)
}

/// The identity of a session: the four `@httpLabel` values every operation in
/// this service binds.
///
/// All four are part of the key because the model addresses a session as
/// `/bots/{botId}/botAliases/{botAliasId}/botLocales/{localeId}/sessions/{sessionId}`
/// — the same `sessionId` under a different bot, alias or locale is a different
/// conversation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionKey {
    pub bot_id: String,
    pub bot_alias_id: String,
    pub locale_id: String,
    pub session_id: String,
}

impl SessionKey {
    /// Build a key from the four labels of a request.
    pub fn new(bot_id: &str, bot_alias_id: &str, locale_id: &str, session_id: &str) -> Self {
        Self {
            bot_id: bot_id.to_string(),
            bot_alias_id: bot_alias_id.to_string(),
            locale_id: locale_id.to_string(),
            session_id: session_id.to_string(),
        }
    }
}

/// An active context together with the instant it was set, which is what makes
/// its `timeToLiveInSeconds` measurable.
#[derive(Debug, Clone)]
pub struct StoredActiveContext {
    pub context: ActiveContext,
    /// When the context was last supplied by a caller. `timeToLiveInSeconds` is
    /// counted from here.
    pub set_at: SystemTime,
    /// Turns the context still has to live. Starts at the caller's
    /// `turnsToLive` and is spent one per conversation turn.
    pub turns_remaining: i32,
}

/// One conversation session.
#[derive(Debug, Clone)]
pub struct Session {
    /// The session's own id (the `sessionId` label it was addressed by).
    pub session_id: String,
    /// The dialog state, minus its active contexts: those live in
    /// [`Session::active_contexts`], which carries the expiry bookkeeping the
    /// modelled shape has nowhere to put. Read the state a caller observes
    /// through [`Session::visible_session_state`], never this field directly.
    pub session_state: SessionState,
    /// The active contexts of the session, with their expiry bookkeeping.
    pub active_contexts: Vec<StoredActiveContext>,
    /// The messages last set for the session, in order.
    pub messages: Vec<Message>,
    /// The interpretations of the last recognized input.
    pub interpretations: Vec<Interpretation>,
    /// Request attributes accumulated over the session.
    pub request_attributes: HashMap<String, String>,
    /// The `originatingRequestId` every state this session reports is stamped
    /// with: minted when the session came into existence and stable for its
    /// whole life, which is what lets a client tell the turns of one
    /// conversation apart from a conversation started on the same session id
    /// after a `DeleteSession`. A caller that supplies one of its own in a
    /// `sessionState` replaces it.
    pub originating_request_id: String,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// Conversation turns served for this session. "Each request and response
    /// from the bot is a turn" (`ActiveContextTimeToLive$turnsToLive` docs), so
    /// only `RecognizeText` bumps it.
    pub turns: u64,
}

impl Session {
    /// A brand-new, empty session, stamped with the `originatingRequestId` the
    /// store minted for it.
    pub fn new(session_id: &str, now: SystemTime, originating_request_id: String) -> Self {
        Self {
            session_id: session_id.to_string(),
            session_state: SessionState::default(),
            active_contexts: Vec::new(),
            messages: Vec::new(),
            interpretations: Vec::new(),
            request_attributes: HashMap::new(),
            originating_request_id,
            created_at: now,
            updated_at: now,
            turns: 0,
        }
    }

    /// Replace the stored active contexts with the ones a caller supplied; each
    /// starts its time- and turn-budget now.
    pub fn set_active_contexts(&mut self, contexts: Vec<ActiveContext>, now: SystemTime) {
        self.active_contexts = contexts
            .into_iter()
            .map(|context| StoredActiveContext {
                turns_remaining: context.time_to_live.turns_to_live,
                context,
                set_at: now,
            })
            .collect();
    }

    /// Is this context still active at `now`?
    ///
    /// `ActiveContextTimeToLive` gives a context two independent budgets — "the
    /// number of seconds that the context is active" and "the number of turns
    /// that the context is active" — so a context lives until the first of them
    /// is used up.
    fn is_live(stored: &StoredActiveContext, now: SystemTime) -> bool {
        if stored.turns_remaining <= 0 {
            return false;
        }
        let ttl = u64::try_from(stored.context.time_to_live.time_to_live_in_seconds).unwrap_or(0);
        match now.duration_since(stored.set_at) {
            Ok(elapsed) => elapsed < Duration::from_secs(ttl),
            // The clock moved backwards between the two reads, so no time has
            // passed for this context and it cannot have timed out yet.
            Err(_) => true,
        }
    }

    /// The active contexts a caller observes at `now`: expired ones are not
    /// reported, with the turns each has left written back into the modelled
    /// `turnsToLive` so a caller can see them run down.
    pub fn live_contexts(&self, now: SystemTime) -> Vec<ActiveContext> {
        self.active_contexts
            .iter()
            .filter(|stored| Self::is_live(stored, now))
            .map(|stored| {
                let mut context = stored.context.clone();
                context.time_to_live.turns_to_live = stored.turns_remaining;
                context
            })
            .collect()
    }

    /// The session state a caller observes at `now`: the stored dialog state
    /// with the contexts that are still active, stamped with the session's
    /// `originatingRequestId`.
    pub fn visible_session_state(&self, now: SystemTime) -> SessionState {
        let mut state = self.session_state.clone();
        // `activeContexts` is the list of contexts that "indicate to Amazon Lex
        // V2 the context of a request", whose modelled `ActiveContextsList`
        // admits a list of `len 0..20`: the list is reported on every state,
        // empty for a conversation that has set no context, so a client can
        // iterate it without first testing whether the member is there.
        state.active_contexts = Some(self.live_contexts(now));
        // The active intent is reported with the dialog members every reported
        // intent carries, whatever the dialog has put on it.
        state.intent = state.intent.map(nlu::active_intent);
        // "A unique identifier for a specific request": the service reports it
        // on every state it answers with, so it is read off the session rather
        // than only echoed back when a caller supplied one.
        state.originating_request_id = Some(self.originating_request_id.clone());
        // `sessionAttributes` is the "map of key/value pairs representing
        // session-specific context information" a client reads and writes across
        // the turns of a conversation: the map is reported on every state, empty
        // for a session nothing has put an attribute on, so a client can index
        // it without first testing whether the member is there.
        state.session_attributes = Some(state.session_attributes.unwrap_or_default());
        state
    }

    /// Spend one conversation turn: drop the contexts that have expired and take
    /// a turn off the ones that remain.
    pub fn spend_turn(&mut self, now: SystemTime) {
        self.active_contexts.retain(|stored| Self::is_live(stored, now));
        for stored in &mut self.active_contexts {
            stored.turns_remaining -= 1;
        }
        self.active_contexts
            .retain(|stored| stored.turns_remaining > 0);
        self.turns += 1;
        self.updated_at = now;
    }

    /// Store a dialog state a caller supplied in full, as `PutSession` does:
    /// every member the request carried becomes the session's state, completed
    /// against the bot's own locale
    /// ([`bot_model::BotLocaleModel::completed_session_state`]) — so a seeded
    /// dialog is read back as the dialog the bot has, interpretations included,
    /// rather than as the caller happened to spell it.
    ///
    /// The `originatingRequestId` is the SESSION's, not one turn's: it is minted
    /// with the conversation and reported for as long as it lives, so a request
    /// that carries none leaves the one the session already holds in place.
    pub fn replace_session_state(
        &mut self,
        supplied: SessionState,
        now: SystemTime,
        locale: &bot_model::BotLocaleModel,
    ) {
        let active_contexts = supplied.active_contexts.clone().unwrap_or_default();
        if let Some(originating_request_id) = supplied.originating_request_id.clone() {
            self.originating_request_id = originating_request_id;
        }
        let (supplied, interpretations) = locale.completed_session_state(supplied);
        self.session_state = SessionState {
            active_contexts: None,
            originating_request_id: None,
            ..supplied
        };
        // The dialog this call replaces is gone, and so are the interpretations
        // an earlier turn of it produced: what the session is interpreted as is
        // the dialog it now holds.
        self.interpretations = interpretations;
        self.set_active_contexts(active_contexts, now);
    }

    /// Merge a dialog state a caller supplied alongside an utterance, as
    /// `RecognizeText` does: a member the request carries overrides the stored
    /// one, a member it omits keeps the value the session already holds, and
    /// session attributes accumulate key by key (they are the session's own
    /// "application information passed between Amazon Lex V2 and a client
    /// application", so an omitted key is not a deletion).
    pub fn merge_session_state(&mut self, supplied: SessionState, now: SystemTime) {
        if let Some(dialog_action) = supplied.dialog_action {
            self.session_state.dialog_action = Some(dialog_action);
        }
        if let Some(intent) = supplied.intent {
            self.session_state.intent = Some(intent);
        }
        if let Some(originating_request_id) = supplied.originating_request_id {
            // A caller that names the request its conversation originated from
            // sets the session's own id to it, so every later state reports what
            // it named rather than the one minted with the session.
            self.originating_request_id = originating_request_id;
        }
        if let Some(runtime_hints) = supplied.runtime_hints {
            self.session_state.runtime_hints = Some(runtime_hints);
        }
        if let Some(attributes) = supplied.session_attributes {
            let mut stored = self.session_state.session_attributes.take().unwrap_or_default();
            merge_attributes(&mut stored, attributes);
            self.session_state.session_attributes = Some(stored);
        }
        if let Some(contexts) = supplied.active_contexts {
            self.set_active_contexts(contexts, now);
        }
    }
}

/// What one conversation turn left behind, read back off the stored session:
/// every value the two operations that advance a turn report to their caller.
#[derive(Debug, Clone)]
pub struct Turn {
    /// The session the turn advanced, by its own id.
    pub session_id: String,
    /// The dialog state after the turn, as a caller observes it.
    pub session_state: SessionState,
    /// The messages the session carries.
    pub messages: Vec<Message>,
    /// The interpretations of this turn's input.
    pub interpretations: Vec<Interpretation>,
    /// The request attributes accumulated over the session.
    pub request_attributes: HashMap<String, String>,
}

/// Why a turn was refused BEFORE it changed anything, so a refused turn leaves
/// the conversation exactly as it was.
#[derive(Debug, Clone)]
pub enum TurnRefused {
    /// An attribute map this turn may not store: the caller's to fix.
    Attributes(AttributesRefused),
    /// The session store is at capacity, so this turn cannot start a session.
    StoreFull(SessionStoreFull),
}

/// Fold `supplied` into `stored`: a supplied key overwrites, every other stored
/// key survives.
pub fn merge_attributes(stored: &mut HashMap<String, String>, supplied: HashMap<String, String>) {
    for (key, value) in supplied {
        stored.insert(key, value);
    }
}

/// What Amazon Lex V2 answers when the session a request addresses is not one it
/// holds: the conversation was never started, or it has gone idle for longer
/// than the bot keeps it.
///
/// The session id the request carried is the only variable part of the sentence
/// — the bot, alias and locale that address it are not named — and the
/// apostrophe is the typographic one (U+2019) the service writes it with. Held
/// here because every operation that reads a session by its four labels answers
/// an absent one the same way.
pub fn session_not_found_message(session_id: &str) -> String {
    format!("The session for user {session_id} doesn\u{2019}t exist or has expired.")
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsLexruntimev2Awsdeepsenseruntimeserviceapi20State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsLexruntimev2Awsdeepsenseruntimeserviceapi20State>)
}

#[cfg(test)]
mod tests {
    //! The time half of a context's two budgets. The turn half is exercised
    //! end-to-end over HTTP in `tests/all/session_lifecycle.rs`; this one needs
    //! a clock the caller chooses, so it is driven here against the stored
    //! session directly rather than by sleeping for a day.

    use super::*;
    use crate::ops::types::ActiveContextTimeToLive;

    fn context(name: &str, seconds: i32, turns: i32) -> ActiveContext {
        ActiveContext {
            name: name.to_string(),
            time_to_live: ActiveContextTimeToLive {
                time_to_live_in_seconds: seconds,
                turns_to_live: turns,
            },
            context_attributes: HashMap::new(),
        }
    }

    /// A context is reported until its `timeToLiveInSeconds` have elapsed since
    /// it was set, at every published end of that range (5..86400 seconds) and
    /// one second either side of each.
    #[test]
    fn a_context_is_reported_until_its_seconds_elapse() {
        let set_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        // (case, timeToLiveInSeconds, seconds elapsed, still reported)
        let cases: &[(&str, i32, u64, bool)] = &[
            ("the shortest ttl, just set", 5, 0, true),
            ("the shortest ttl, one second short", 5, 4, true),
            ("the shortest ttl, on the second", 5, 5, false),
            ("the shortest ttl, one second past", 5, 6, false),
            ("an hour, half spent", 3600, 1800, true),
            ("the longest ttl, one second short", 86400, 86399, true),
            ("the longest ttl, on the second", 86400, 86400, false),
            ("the longest ttl, a second past", 86400, 86401, false),
        ];

        for (case, seconds, elapsed, expected) in cases {
            let mut session = Session::new("s", set_at, "request-1".to_string());
            session.set_active_contexts(vec![context("checkout", *seconds, 20)], set_at);
            let now = set_at + Duration::from_secs(*elapsed);
            let live = session.live_contexts(now);
            assert_eq!(
                live.len(),
                usize::from(*expected),
                "case {case:?}: a context with a {seconds}s ttl after {elapsed}s \
                 must{} be reported, got {live:?}",
                if *expected { "" } else { " not" }
            );
            let state = session.visible_session_state(now);
            // The member is reported on every state, so what tells an expired
            // context from a live one is the length of the list, not whether
            // the list is there at all.
            assert_eq!(
                state.active_contexts.as_ref().map(Vec::len),
                Some(usize::from(*expected)),
                "case {case:?}: the session state a caller observes must agree, got {:?}",
                state.active_contexts,
            );
        }
    }

    /// Reporting a session changes nothing: an expired context is left out of
    /// the answer, and the next reader sees exactly the same thing.
    #[test]
    fn reading_a_session_does_not_change_it() {
        let set_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let mut session = Session::new("s", set_at, "request-1".to_string());
        session.set_active_contexts(
            vec![context("short", 5, 20), context("long", 86400, 20)],
            set_at,
        );
        let later = set_at + Duration::from_secs(60);

        let first = session.live_contexts(later);
        let second = session.live_contexts(later);
        assert_eq!(
            first.len(),
            1,
            "only the unexpired context is reported, got {first:?}"
        );
        assert_eq!(
            first.iter().map(|c| c.name.clone()).collect::<Vec<_>>(),
            second.iter().map(|c| c.name.clone()).collect::<Vec<_>>(),
            "a second read must report the same contexts as the first",
        );
        assert_eq!(
            session.active_contexts.len(),
            2,
            "a read must not drop the stored context: that is the next turn's work",
        );
        assert_eq!(session.turns, 0, "a read is not a conversation turn");
    }

    /// A turn spends one turn of every context and drops what has run out.
    #[test]
    fn a_turn_spends_one_turn_of_each_context() {
        let set_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let mut session = Session::new("s", set_at, "request-1".to_string());
        session.set_active_contexts(
            vec![context("one", 86400, 1), context("three", 86400, 3)],
            set_at,
        );

        session.spend_turn(set_at + Duration::from_secs(1));
        let after_one = session.live_contexts(set_at + Duration::from_secs(1));
        assert_eq!(
            after_one.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            vec!["three"],
            "the one-turn context is spent, got {after_one:?}"
        );
        assert_eq!(
            after_one[0].time_to_live.turns_to_live, 2,
            "the three-turn context has two turns left, got {after_one:?}"
        );
        assert_eq!(session.turns, 1, "the turn counter follows the turns served");
    }

    /// The store's first bound: a session that has gone idle for longer than
    /// [`IDLE_SESSION_TTL`] is gone, which is what keeps the map from holding a
    /// session forever when no client deletes one.
    #[test]
    fn an_idle_session_is_expired() {
        let created = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let key = SessionKey::new("ABCDEFGHIJ", "TSTALIASID", "en_US", "s");

        // (case, seconds since the last request, still there)
        let cases: &[(&str, u64, bool)] = &[
            ("just used", 0, true),
            ("one second short of the ttl", IDLE_SESSION_TTL.as_secs() - 1, true),
            ("exactly the ttl", IDLE_SESSION_TTL.as_secs(), false),
            ("long past the ttl", IDLE_SESSION_TTL.as_secs() * 10, false),
        ];

        for (case, idle, expected) in cases {
            let mut store = ComAmazonawsLexruntimev2Awsdeepsenseruntimeserviceapi20State::default();
            store
                .session_mut(key.clone(), created)
                .expect("an empty store has room");
            let now = created + Duration::from_secs(*idle);
            assert_eq!(
                store.session(&key, now).is_some(),
                *expected,
                "case {case:?}: after {idle}s idle the session must{} be reported",
                if *expected { "" } else { " not" }
            );
            assert_eq!(
                store.sessions.len(),
                usize::from(*expected),
                "case {case:?}: an expired session must be dropped from the map, not just hidden",
            );
        }
    }

    /// The store's second bound: it refuses to grow past [`MAX_SESSIONS`], and
    /// the refusal precedes the allocation. Sessions already in the map stay
    /// usable, and expiring one makes room again.
    #[test]
    fn the_store_refuses_to_grow_past_its_cap() {
        let created = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let mut store = ComAmazonawsLexruntimev2Awsdeepsenseruntimeserviceapi20State::default();
        let key = |index: usize| {
            SessionKey::new("ABCDEFGHIJ", "TSTALIASID", "en_US", &format!("s{index}"))
        };

        for index in 0..MAX_SESSIONS {
            assert!(
                store.session_mut(key(index), created).is_ok(),
                "case {index}: a session below the cap must be created",
            );
        }
        assert_eq!(store.sessions.len(), MAX_SESSIONS);
        assert!(
            store.session_mut(key(MAX_SESSIONS), created).is_err(),
            "a new key past the cap must be refused, not allocated",
        );
        assert_eq!(
            store.sessions.len(),
            MAX_SESSIONS,
            "the refused key must not have been inserted",
        );
        assert!(
            store.session_mut(key(0), created).is_ok(),
            "a session already in the store stays usable at the cap",
        );

        // One request later than the TTL: every session is idle, so the sweep
        // makes room and the next caller is served.
        let later = created + IDLE_SESSION_TTL;
        assert!(
            store.session_mut(key(MAX_SESSIONS), later).is_ok(),
            "once the idle sessions are swept there is room again",
        );
        assert_eq!(
            store.sessions.len(),
            1,
            "the sweep must have dropped the idle sessions",
        );
    }

    /// The per-session attribute bound: the merge is measured before anything is
    /// stored, at the cap and one byte either side of it.
    #[test]
    fn an_attribute_map_is_measured_against_the_cap() {
        let stored: HashMap<String, String> =
            [("a".to_string(), "x".repeat(1000))].into_iter().collect();

        // (case, supplied key, supplied value bytes, accepted)
        let cases: &[(&str, &str, usize, bool)] = &[
            ("a small addition", "b", 10, true),
            (
                "filling the cap exactly",
                "b",
                MAX_ATTRIBUTE_BYTES - 1001 - 1,
                true,
            ),
            (
                "one byte past the cap",
                "b",
                MAX_ATTRIBUTE_BYTES - 1001,
                false,
            ),
            (
                "overwriting the stored key cannot grow it",
                "a",
                1000,
                true,
            ),
        ];

        for (case, supplied_key, value_bytes, accepted) in cases {
            let supplied: HashMap<String, String> = [(
                (*supplied_key).to_string(),
                "y".repeat(*value_bytes),
            )]
            .into_iter()
            .collect();
            let result =
                check_attribute_bytes("sessionAttributes", Some(&stored), Some(&supplied));
            assert_eq!(
                result.is_ok(),
                *accepted,
                "case {case:?}: a {value_bytes}-byte value must{} be accepted, got {result:?}",
                if *accepted { "" } else { " not" }
            );
        }

        assert!(
            check_attribute_bytes("sessionAttributes", Some(&stored), None).is_ok(),
            "a request that supplies no attributes cannot grow the map",
        );
    }

    /// The reserved namespace is refused on `requestAttributes` only — it is the
    /// member whose docs reserve it — and the byte bound still applies there.
    #[test]
    fn a_reserved_request_attribute_key_is_refused() {
        let reserved: HashMap<String, String> = [(
            format!("{RESERVED_ATTRIBUTE_PREFIX}channel"),
            "web".to_string(),
        )]
        .into_iter()
        .collect();
        let ordinary: HashMap<String, String> =
            [("channel".to_string(), "web".to_string())].into_iter().collect();

        assert!(
            matches!(
                check_request_attributes("requestAttributes", None, Some(&reserved)),
                Err(AttributesRefused::ReservedKey { .. })
            ),
            "a key in the reserved namespace must be refused",
        );
        assert!(
            check_request_attributes("requestAttributes", None, Some(&ordinary)).is_ok(),
            "an ordinary key must be accepted",
        );
        assert!(
            check_attribute_bytes("sessionState.sessionAttributes", None, Some(&reserved)).is_ok(),
            "session attributes carry no reserved-namespace rule of their own",
        );
    }
}
