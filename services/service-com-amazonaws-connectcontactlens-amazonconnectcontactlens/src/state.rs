// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-connectcontactlens-amazonconnectcontactlens service.
//!
//! Contact Lens has NO public write API. The real-time analysis segments of a contact
//! are produced by Amazon Connect's voice analysis while the call runs, and
//! `ListRealtimeContactAnalysisSegments` only *reports* them. The create side of the
//! lifecycle therefore lives here, as an in-process seeding API on the state struct
//! ([`ComAmazonawsConnectcontactlensAmazonconnectcontactlensState::put_session`],
//! [`ComAmazonawsConnectcontactlensAmazonconnectcontactlensState::append_segments`],
//! [`ComAmazonawsConnectcontactlensAmazonconnectcontactlensState::complete_session`],
//! [`ComAmazonawsConnectcontactlensAmazonconnectcontactlensState::put_rule_tags`])
//! which the producing side — a test today, the Amazon Connect emulator's contact flow
//! later — drives through the one shared engine. The operation handler never writes a
//! segment; the only state change it makes is materialising the 24-hour expiry the
//! clock already made due, plus the page cursor it hands back.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::RealtimeContactAnalysisSegment;

/// How long a contact's voice analysis data stays readable.
///
/// From the modeled operation doc (`smithy_query(query="doc",
/// shape="ListRealtimeContactAnalysisSegments")`): "Voice data is retained for 24
/// hours. You must invoke this API during that time." Past that the session is gone and
/// the operation answers `ResourceNotFoundException`.
pub const VOICE_DATA_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);

/// The largest page `ListRealtimeContactAnalysisSegments` can return, and the page size
/// used when a request names no `MaxResults`.
///
/// Both the `MaxResults` range (1..100) and the `Segments` list length (0..100) cap at
/// 100 in the model, so no page can carry more than this many segments.
pub const MAX_PAGE_SIZE: i32 = 100;

/// A real-time analysis session is identified by the `(InstanceId, ContactId)` pair the
/// operation takes; neither half alone is unique.
pub type SessionKey = (String, String);

/// One contact's real-time analysis session: the segments Amazon Connect has produced
/// for it so far, when analysis started (which drives [`VOICE_DATA_RETENTION`]), and
/// whether analysis has finished.
///
/// `segments` holds the modeled `RealtimeContactAnalysisSegment` shape itself rather
/// than a parallel "stored" mirror of it, so serialising a page is a direct copy of
/// what was stored and no member can drift between the two. It is APPEND-ONLY, in the
/// arrival (chronological) order Connect produces segments in: a page cursor is an
/// index into this list, so re-ordering or removing an element would make an
/// outstanding cursor skip or duplicate a segment.
#[derive(Debug, Clone)]
pub struct AnalysisSession {
    pub instance_id: String,
    pub contact_id: String,
    pub started_at: SystemTime,
    pub completed: bool,
    pub segments: Vec<RealtimeContactAnalysisSegment>,
}

impl AnalysisSession {
    /// An open (not yet completed) session with no segments, started now.
    pub fn new(instance_id: impl Into<String>, contact_id: impl Into<String>) -> Self {
        Self {
            instance_id: instance_id.into(),
            contact_id: contact_id.into(),
            started_at: SystemTime::now(),
            completed: false,
            segments: Vec::new(),
        }
    }

    /// Start this session at `started_at` instead of now — what a seeder uses to place
    /// a session inside or outside the retention window.
    #[must_use]
    pub fn started_at(mut self, started_at: SystemTime) -> Self {
        self.started_at = started_at;
        self
    }

    /// Mark analysis finished (`true`) or still running (`false`).
    #[must_use]
    pub fn completed(mut self, completed: bool) -> Self {
        self.completed = completed;
        self
    }

    /// Seed this session's segments, in arrival order.
    #[must_use]
    pub fn with_segments(
        mut self,
        segments: impl IntoIterator<Item = RealtimeContactAnalysisSegment>,
    ) -> Self {
        self.segments = segments.into_iter().collect();
        self
    }

    /// The `(InstanceId, ContactId)` key this session is stored under.
    pub fn key(&self) -> SessionKey {
        (self.instance_id.clone(), self.contact_id.clone())
    }

    /// Whether this session's voice data is already past [`VOICE_DATA_RETENTION`] at
    /// `now`. A session whose `started_at` is in the future is not expired.
    pub fn is_expired_at(&self, now: SystemTime) -> bool {
        now.duration_since(self.started_at)
            .is_ok_and(|age| age >= VOICE_DATA_RETENTION)
    }
}

/// The tags assigned to one Amazon Connect rule, keyed by tag key.
///
/// A matched category names the rule that created it (`CategoryDetails.RuleId`), and a
/// request's `ListFilter` is evaluated against THESE tags — so the tags are what decides
/// which categories a caller may view.
pub type RuleTags = HashMap<String, String>;

/// A rule is identified by the instance it belongs to and its own identifier; a rule
/// identifier is unique within one Amazon Connect instance, not across instances.
pub type RuleKey = (String, String);

/// Where an opaque `NextToken` resumes: the session it belongs to, and the index of the
/// first segment the next page returns.
///
/// The token is never parsed; it is looked up, and only ever through
/// [`ComAmazonawsConnectcontactlensAmazonconnectcontactlensState::cursor_for`], which
/// refuses a cursor belonging to a different session. So a token the service did not
/// mint for THIS contact cannot resume a page inside another contact's segments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    pub instance_id: String,
    pub contact_id: String,
    pub next_index: usize,
}

/// Per-service state for com-amazonaws-connectcontactlens-amazonconnectcontactlens. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsConnectcontactlensAmazonconnectcontactlensState {
    /// Analysis sessions, keyed by the `(InstanceId, ContactId)` pair.
    sessions: HashMap<SessionKey, AnalysisSession>,
    /// Live page cursors, keyed by the opaque `NextToken` handed to the client. This is
    /// what makes a token validatable: resolution is a lookup, never a parse.
    cursors: HashMap<String, Cursor>,
    /// The tags on the Amazon Connect rules whose matches appear in the stored segments,
    /// keyed by `(InstanceId, RuleId)`. Rules outlive the contacts they match on, so they
    /// are held here rather than inside a session and are NOT dropped when one expires.
    rule_tags: HashMap<RuleKey, RuleTags>,
    /// Index of `cursors` by the position the cursor resumes at, so asking twice for
    /// the token of one position hands back the SAME token instead of minting a second
    /// one. Without it, listing is not idempotent: two identical requests would answer
    /// with different `NextToken`s and the cursor map would grow per call.
    tokens_by_position: HashMap<(SessionKey, usize), String>,
    /// Counter behind the opaque token text. It carries no client-readable structure —
    /// a client must treat a `NextToken` as opaque and pass it back unchanged.
    next_cursor_seq: u64,
}

impl ComAmazonawsConnectcontactlensAmazonconnectcontactlensState {
    /// SEEDING HOOK — register (or replace) the analysis session for one contact.
    ///
    /// Returns the session it replaced, if any. Replacing a session drops the cursors
    /// minted against the old one: their indices described a different segment list.
    pub fn put_session(&mut self, session: AnalysisSession) -> Option<AnalysisSession> {
        let key = session.key();
        self.forget_cursors(&key);
        self.sessions.insert(key, session)
    }

    /// SEEDING HOOK — append segments to an existing session, in arrival order.
    ///
    /// `false` when no session is stored for `key`, so a caller cannot silently create
    /// one without a start time.
    pub fn append_segments(
        &mut self,
        key: &SessionKey,
        segments: impl IntoIterator<Item = RealtimeContactAnalysisSegment>,
    ) -> bool {
        match self.sessions.get_mut(key) {
            Some(session) => {
                session.segments.extend(segments);
                true
            }
            None => false,
        }
    }

    /// SEEDING HOOK — mark a session's analysis finished, which is what lets the final
    /// page omit `NextToken`. `false` when no session is stored for `key`.
    pub fn complete_session(&mut self, key: &SessionKey) -> bool {
        match self.sessions.get_mut(key) {
            Some(session) => {
                session.completed = true;
                true
            }
            None => false,
        }
    }

    /// The session stored for `key`, if any. Retention is NOT applied here — call
    /// [`Self::purge_expired`] first, so one instant decides both expiry and the read.
    pub fn session(&self, key: &SessionKey) -> Option<&AnalysisSession> {
        self.sessions.get(key)
    }

    /// SEEDING HOOK — register (or replace) the tags on one Amazon Connect rule, the
    /// rule a matched category names through `CategoryDetails.RuleId`.
    ///
    /// Returns the tags it replaced, if any. Rules are a resource of the Amazon Connect
    /// instance rather than of a contact, so this is independent of any session.
    pub fn put_rule_tags(
        &mut self,
        instance_id: impl Into<String>,
        rule_id: impl Into<String>,
        tags: impl IntoIterator<Item = (String, String)>,
    ) -> Option<RuleTags> {
        self.rule_tags.insert(
            (instance_id.into(), rule_id.into()),
            tags.into_iter().collect(),
        )
    }

    /// The tags on one rule of one instance, or `None` when no rule of that instance is
    /// known by that identifier — which is also the answer for a matched category that
    /// names no rule at all.
    pub fn rule_tags(&self, instance_id: &str, rule_id: &str) -> Option<&RuleTags> {
        self.rule_tags
            .get(&(instance_id.to_string(), rule_id.to_string()))
    }

    /// Drop a session and every cursor minted against it.
    pub fn remove_session(&mut self, key: &SessionKey) -> Option<AnalysisSession> {
        self.forget_cursors(key);
        self.sessions.remove(key)
    }

    /// Drop every session whose voice data is past [`VOICE_DATA_RETENTION`] at `now`,
    /// and the cursors that pointed into them; returns how many were dropped.
    ///
    /// This is the expiry the clock already owed at `now` — it would have happened
    /// whether or not anybody called — which is why a reporting operation may
    /// materialise it.
    pub fn purge_expired(&mut self, now: SystemTime) -> usize {
        let expired: Vec<SessionKey> = self
            .sessions
            .iter()
            .filter(|(_, session)| session.is_expired_at(now))
            .map(|(key, _)| key.clone())
            .collect();
        for key in &expired {
            self.sessions.remove(key);
            self.forget_cursors(key);
        }
        expired.len()
    }

    /// The opaque `NextToken` that resumes `key` at `next_index`, minting one only the
    /// first time that position is handed out.
    pub fn token_for(&mut self, key: &SessionKey, next_index: usize) -> String {
        if let Some(token) = self.tokens_by_position.get(&(key.clone(), next_index)) {
            return token.clone();
        }
        let token = format!("{:032x}", self.next_cursor_seq);
        self.next_cursor_seq += 1;
        self.cursors.insert(
            token.clone(),
            Cursor {
                instance_id: key.0.clone(),
                contact_id: key.1.clone(),
                next_index,
            },
        );
        self.tokens_by_position
            .insert((key.clone(), next_index), token.clone());
        token
    }

    /// Resolve a client-supplied `NextToken` FOR one session. `None` means no live
    /// cursor backs it, or the cursor it names belongs to another session — both of
    /// which the operation answers with `InvalidRequestException`.
    ///
    /// The ownership test lives here, not in the handler, so it cannot be forgotten:
    /// there is no unscoped lookup to call.
    pub fn cursor_for(&self, key: &SessionKey, token: &str) -> Option<&Cursor> {
        self.cursors
            .get(token)
            .filter(|cursor| cursor.instance_id == key.0 && cursor.contact_id == key.1)
    }

    /// Drop every cursor (and its position index entry) belonging to one session.
    fn forget_cursors(&mut self, key: &SessionKey) {
        self.cursors
            .retain(|_, cursor| cursor.instance_id != key.0 || cursor.contact_id != key.1);
        self.tokens_by_position
            .retain(|(cursor_key, _), _| cursor_key != key);
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsConnectcontactlensAmazonconnectcontactlensState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsConnectcontactlensAmazonconnectcontactlensState>)
}
