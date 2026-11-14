// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-connectparticipant-amazonconnectparticipantservicelambda service.
//!
//! One chat session per contact. `CreateParticipantConnection` mints a session for a
//! participant token and binds a freshly generated connection token to it; every other
//! operation authenticates by looking that connection token up here.
//!
//! Every store is bounded. Connection bindings and sessions are reclaimed once the clock
//! has passed their expiry ([`State::prune_expired`], called by the operations that write
//! state), and the counts a caller can drive are capped by [`MAX_SESSIONS`],
//! [`MAX_CONNECTIONS_PER_SESSION`] and [`MAX_TRANSCRIPT_ITEMS`], past which a request is
//! refused rather than served.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;

/// How long a connection token stays usable.
///
/// `CreateParticipantConnection`'s own documentation: "The expiry time for the
/// connection token is different than the ChatDurationInMinutes. Expiry time for the
/// connection token is 1 day."
pub const CONNECTION_TOKEN_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// How long the returned websocket URL may be connected to.
///
/// `CreateParticipantConnection`'s own documentation: "For WEBSOCKET Type: The
/// response URL for has a connect expiry timeout of 100s."
pub const WEBSOCKET_CONNECT_TTL: Duration = Duration::from_secs(100);

/// How long a session is retained: exactly as long as the newest connection token that
/// can reach it, so a session never outlives every token for it. Pushed out whenever
/// another connection is created; past it, [`State::prune_expired`] reclaims the session.
///
/// Equal to [`CONNECTION_TOKEN_TTL`] because a token minted now expires that far ahead.
pub const SESSION_RETENTION: Duration = CONNECTION_TOKEN_TTL;

/// The most chat sessions this emulator holds at once. State is in memory, so the count a
/// caller can drive is capped; `CreateParticipantConnection` refuses a new session past it
/// rather than growing without limit.
pub const MAX_SESSIONS: usize = 10_000;

/// The most live connection tokens one session may hold. `CreateParticipantConnection`'s
/// documentation points at "the allowed number of open websocket connections per
/// participant" in the Amazon Connect feature specifications; this is this emulator's
/// own bound on that count.
pub const MAX_CONNECTIONS_PER_SESSION: usize = 100;

/// The most items one session's transcript holds. `SendMessage` / `SendEvent` refuse a
/// further item past it. It also bounds the per-request work `GetTranscript` does, and
/// the idempotency map, which only ever gains an entry alongside an item.
pub const MAX_TRANSCRIPT_ITEMS: usize = 10_000;

/// `ConnectionType` values this emulator serves, from `CONNECTION_TYPE_VALUES`.
pub const CONNECTION_TYPE_WEBSOCKET: &str = "WEBSOCKET";
/// See [`CONNECTION_TYPE_WEBSOCKET`].
pub const CONNECTION_TYPE_CONNECTION_CREDENTIALS: &str = "CONNECTION_CREDENTIALS";
/// See [`CONNECTION_TYPE_WEBSOCKET`].
pub const CONNECTION_TYPE_WEBRTC: &str = "WEBRTC_CONNECTION";

/// `ChatItemType$MESSAGE`, the transcript item `SendMessage` appends.
pub const CHAT_ITEM_TYPE_MESSAGE: &str = "MESSAGE";
/// `ChatItemType$EVENT`, the transcript item `SendEvent` appends.
pub const CHAT_ITEM_TYPE_EVENT: &str = "EVENT";

/// `ParticipantRole$CUSTOMER`: the role of a participant that connects with a
/// participant token obtained from `StartChatContact`.
pub const PARTICIPANT_ROLE_CUSTOMER: &str = "CUSTOMER";

/// The `AccessDeniedException$Message` this service answers with.
///
/// The text is fixed: this service tells a caller whose token does not authenticate it
/// only that access was denied, never which token failed or whether the resource it
/// named exists. Every operation that refuses with this error words it the same way, so
/// the wording lives here rather than at each call site.
pub const ACCESS_DENIED_MESSAGE: &str = "Access denied";

/// One message or event stored in a session's transcript, in the order it was sent.
#[derive(Debug, Clone)]
pub struct TranscriptItem {
    /// `Item$Id`.
    pub id: String,
    /// `Item$AbsoluteTime`, ISO-8601 `yyyy-MM-ddThh:mm:ss.SSSZ`.
    pub absolute_time: String,
    /// `Item$Content`. Absent for an event sent without content.
    pub content: Option<String>,
    /// `Item$ContentType`.
    pub content_type: String,
    /// `Item$Type`: [`CHAT_ITEM_TYPE_MESSAGE`] or [`CHAT_ITEM_TYPE_EVENT`].
    pub item_type: String,
    /// `Item$ParticipantId`.
    pub participant_id: String,
    /// `Item$DisplayName`.
    pub display_name: Option<String>,
    /// `Item$ParticipantRole`.
    pub participant_role: String,
    /// `Item$ContactId`.
    pub contact_id: String,
}

/// What one connection token authenticates, and when it stops doing so.
///
/// The expiry lives HERE rather than on the session, so a token is judged by the window
/// it was minted with: creating another connection cannot extend one already handed out.
#[derive(Debug, Clone)]
pub struct ConnectionBinding {
    /// The contact whose session this token reaches.
    pub contact_id: String,
    /// The `Expiry` this token was reported with.
    pub expiry: SystemTime,
}

/// A view one chat's participant was shown, and what `DescribeView` reports for it.
///
/// A `ViewToken` is "an encrypted token originating from the interactive message of a
/// ShowView block operation" (`DescribeViewRequest$ViewToken`), so it is minted where
/// the view is SHOWN — the contact flow the chat runs — and no call this service's API
/// carries creates one, exactly as no call here creates the chat session itself. The
/// component that shows a view therefore writes this record into this store (the engine
/// keys state by TYPE, so a copy of its own would be invisible to the reader here) and
/// `DescribeView` only reports it; nothing in this service invents a view or a token for
/// one.
///
/// Every member is the `View` member it is named for, so the read reports what was
/// stored and derives nothing from the request.
#[derive(Debug, Clone)]
pub struct ShownView {
    /// The contact whose participant was shown this view. Only a connection token that
    /// authenticates THIS contact's participant may read it: a view token minted for one
    /// chat is not a token for another's.
    pub contact_id: String,
    /// `View$Id`.
    pub id: String,
    /// `View$Arn`.
    pub arn: String,
    /// `View$Name`.
    pub name: String,
    /// `View$Version`.
    pub version: i32,
    /// `ViewContent$InputSchema`.
    pub input_schema: Option<String>,
    /// `ViewContent$Template`.
    pub template: Option<String>,
    /// `ViewContent$Actions`.
    pub actions: Vec<String>,
}

/// A participant's chat session: who its items are attributed to, whether it is still
/// connected, and the transcript every later call reads and appends to.
#[derive(Debug, Clone)]
pub struct ChatSession {
    /// The contact this session belongs to.
    pub contact_id: String,
    /// `GetTranscriptResponse$InitialContactId`.
    pub initial_contact_id: String,
    /// The participant every item this session sends is attributed to.
    pub participant_id: String,
    /// That participant's `ParticipantRole`.
    pub participant_role: String,
    /// That participant's chat display name, when one is known.
    ///
    /// Its one source is the `ParticipantDetails` the chat was started with on the
    /// connect service (`StartChatContact`), which writes the session here: no input
    /// THIS service models carries a display name, so a session created by
    /// `CreateParticipantConnection` for a token no started chat registered has none.
    pub display_name: Option<String>,
    /// False once `DisconnectParticipant` ran: every later call is refused.
    pub connected: bool,
    /// Whether the caller asked to be marked as connected (`ConnectParticipant`).
    ///
    /// Recorded because it is what the real service uses to mark a customer participant
    /// as connected for message streaming; no operation in this slice reads it, and
    /// message streaming is its own gap.
    pub connect_participant: bool,
    /// The transcript, oldest item first.
    pub transcript: Vec<TranscriptItem>,
    /// Idempotency: `<item type>:<ClientToken>` -> the id of the item it created. Gains
    /// an entry only alongside a transcript item, so [`MAX_TRANSCRIPT_ITEMS`] bounds it.
    pub idempotency: HashMap<String, String>,
    /// When this session, and the participant token that reaches it, is reclaimed: the
    /// expiry of the newest connection token minted for it. Pushed out whenever another
    /// connection is created.
    pub retain_until: SystemTime,
}

impl ChatSession {
    /// The item `id` names, if this session's transcript holds it.
    pub fn item(&self, id: &str) -> Option<&TranscriptItem> {
        self.transcript.iter().find(|item| item.id == id)
    }

    /// The item an idempotency `key` already created on this session: the caller is
    /// repeating a request, so it must be answered with what the first call stored.
    pub fn replayed(&self, key: Option<&str>) -> Option<&TranscriptItem> {
        self.item(self.idempotency.get(key?)?)
    }

    /// Whether another item may be appended, or this transcript holds its
    /// [`MAX_TRANSCRIPT_ITEMS`] limit.
    pub fn has_room(&self) -> bool {
        self.transcript.len() < MAX_TRANSCRIPT_ITEMS
    }

    /// Append an item sent at `now`, attributed to this session's participant, recording
    /// `key` so a repeat replays it. Returns the `Id` and `AbsoluteTime` it was stored
    /// with — the values the sending operation reports.
    pub fn append(
        &mut self,
        item_type: &str,
        content: Option<String>,
        content_type: String,
        key: Option<String>,
        now: SystemTime,
    ) -> (String, String) {
        let item = TranscriptItem {
            id: new_id(),
            absolute_time: format_instant(now),
            content,
            content_type,
            item_type: item_type.to_string(),
            participant_id: self.participant_id.clone(),
            display_name: self.display_name.clone(),
            participant_role: self.participant_role.clone(),
            contact_id: self.contact_id.clone(),
        };
        let stored = (item.id.clone(), item.absolute_time.clone());
        if let Some(key) = key {
            self.idempotency.insert(key, item.id.clone());
        }
        self.transcript.push(item);
        stored
    }
}

/// Per-service state for com-amazonaws-connectparticipant-amazonconnectparticipantservicelambda. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsConnectparticipantAmazonconnectparticipantservicelambdaState {
    /// Every chat session, keyed by its contact id.
    pub sessions: HashMap<String, ChatSession>,
    /// Connection token -> what it authenticates: the index every operation other than
    /// `CreateParticipantConnection` resolves its caller through.
    pub connections: HashMap<String, ConnectionBinding>,
    /// Participant token -> contact id, so re-using a participant token resolves to the
    /// session it already created.
    pub participant_tokens: HashMap<String, String>,
    /// View token -> the view that token was minted for: what `DescribeView` reports.
    /// Written where a view is shown to a participant (see [`ShownView`]), and bounded
    /// by the sessions it belongs to — [`State::prune_expired`] drops a view with the
    /// session whose chat it was shown in.
    pub shown_views: HashMap<String, ShownView>,
}

impl ComAmazonawsConnectparticipantAmazonconnectparticipantservicelambdaState {
    /// The session `connection_token` authenticates, when that token has not expired and
    /// its session is still connected.
    pub fn session_for_connection_token(
        &self,
        connection_token: &str,
        now: SystemTime,
    ) -> Option<&ChatSession> {
        let binding = self.connections.get(connection_token)?;
        if now >= binding.expiry {
            return None;
        }
        self.sessions
            .get(&binding.contact_id)
            .filter(|session| session.connected)
    }

    /// [`Self::session_for_connection_token`], for a caller that appends to the transcript.
    pub fn session_for_connection_token_mut(
        &mut self,
        connection_token: &str,
        now: SystemTime,
    ) -> Option<&mut ChatSession> {
        let binding = self.connections.get(connection_token)?;
        if now >= binding.expiry {
            return None;
        }
        let contact_id = binding.contact_id.clone();
        self.sessions
            .get_mut(&contact_id)
            .filter(|session| session.connected)
    }

    /// The view `view_token` names, when it was shown to `contact_id`'s participant.
    ///
    /// A token that names no view here, or one shown in another contact's chat, resolves
    /// to nothing: a view token is a credential for one participant of one chat.
    pub fn view_for_token(&self, view_token: &str, contact_id: &str) -> Option<&ShownView> {
        self.shown_views
            .get(view_token)
            .filter(|view| view.contact_id == contact_id)
    }

    /// How many live connection tokens reach `contact_id`.
    pub fn connection_count(&self, contact_id: &str) -> usize {
        self.connections
            .values()
            .filter(|binding| binding.contact_id == contact_id)
            .count()
    }

    /// Reclaim what the clock has already retired: every connection token past its
    /// expiry, every session past its retention window, and the index entries that
    /// pointed at them.
    ///
    /// This materialises a change that was due at this instant whether or not anybody
    /// called, so it is driven from the operations that write state.
    pub fn prune_expired(&mut self, now: SystemTime) {
        self.sessions
            .retain(|_, session| now < session.retain_until);
        let sessions = &self.sessions;
        // One pass over the bindings: a binding dies when its own window closes or when
        // the session it reaches has been reclaimed.
        self.connections.retain(|_, binding| {
            now < binding.expiry && sessions.contains_key(&binding.contact_id)
        });
        self.participant_tokens
            .retain(|_, contact_id| sessions.contains_key(contact_id));
        // A view was shown inside one chat, so it is reachable exactly as long as that
        // chat's session is.
        self.shown_views
            .retain(|_, view| sessions.contains_key(&view.contact_id));
    }

    /// Drop every connection token that reaches `contact_id`: a disconnected
    /// participant's tokens can never serve another request.
    pub fn drop_connections(&mut self, contact_id: &str) {
        self.connections
            .retain(|_, binding| binding.contact_id != contact_id);
    }
}

/// A fresh opaque identifier: a contact id, a participant id, a chat item id or a
/// connection token. Shaped like the UUIDs Amazon Connect uses for all of them.
///
/// Built from a per-process seed and a monotonic counter, so two ids from one emulator
/// never collide and nothing about an id is derived from the request.
pub fn new_id() -> String {
    static SEED: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let seed = *SEED.get_or_init(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos() as u64)
    });
    let serial = NEXT.fetch_add(1, Ordering::Relaxed);
    // The RFC 4122 shape: version 4 in the third group, variant `8` in the fourth.
    format!(
        "{:08x}-{:04x}-4{:03x}-8{:03x}-{:012x}",
        (seed >> 32) as u32,
        (seed >> 16) as u16,
        (seed & 0xfff) as u16,
        ((serial >> 48) & 0xfff) as u16,
        serial & 0xffff_ffff_ffff,
    )
}

/// The idempotency key an operation records a `ClientToken` under, or `None` when the
/// caller sent no token (the `ClientToken` shape permits an empty string, which asks
/// for no idempotency at all).
///
/// Scoped by item type because a `ClientToken` makes ONE operation idempotent: the same
/// token on `SendMessage` and on `SendEvent` is two distinct requests.
pub fn idempotency_key(item_type: &str, client_token: Option<&str>) -> Option<String> {
    let token = client_token?;
    (!token.is_empty()).then(|| format!("{item_type}:{token}"))
}

/// `at` as an `ISO8601Datetime` / `Instant`, in the documented
/// `yyyy-MM-ddThh:mm:ss.SSSZ` form.
///
/// The calendar arithmetic and the RFC 3339 rendering come from `aws-smithy-types`'
/// [`DateTime`], the timestamp type this framework already uses on the wire;
/// [`millisecond_precision`] only pins the fractional part to the three digits the model
/// documents, which `Format::DateTime` renders variably.
pub fn format_instant(at: SystemTime) -> String {
    let millis = at
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis().min(i64::MAX as u128) as i64);
    let rendered = DateTime::from_millis(millis)
        .fmt(Format::DateTime)
        // `Format::DateTime` renders any instant this type can hold.
        .expect("an Instant renders as RFC 3339");
    millisecond_precision(&rendered)
}

/// `value` re-rendered in the documented `yyyy-MM-ddThh:mm:ss.SSSZ` form, when it is an
/// `ISO8601Datetime` / `Instant` at all; `None` for anything else.
///
/// Normalising before comparing is what lets the transcript scan compare instants
/// lexicographically: two renderings of one moment that differ only in how many
/// fractional digits they carry would not compare equal as text.
pub fn normalise_instant(value: &str) -> Option<String> {
    let parsed = DateTime::from_str(value, Format::DateTime).ok()?;
    let rendered = parsed.fmt(Format::DateTime).ok()?;
    Some(millisecond_precision(&rendered))
}

/// Whether `value` is an `ISO8601Datetime` / `Instant`.
pub fn is_instant(value: &str) -> bool {
    normalise_instant(value).is_some()
}

/// `rendered` with its fractional second pinned to exactly three digits, the precision
/// the model documents (`yyyy-MM-ddThh:mm:ss.SSSZ`, e.g. `2019-11-08T02:41:28.172Z`).
/// `Format::DateTime` omits the fraction when it is zero and carries more digits when it
/// is not, so the width is fixed here rather than at each call site.
fn millisecond_precision(rendered: &str) -> String {
    let Some(body) = rendered.strip_suffix('Z') else {
        return rendered.to_string();
    };
    match body.split_once('.') {
        None => format!("{body}.000Z"),
        Some((head, fraction)) => {
            let mut digits: String = fraction
                .chars()
                .filter(|character| character.is_ascii_digit())
                .take(3)
                .collect();
            while digits.len() < 3 {
                digits.push('0');
            }
            format!("{head}.{digits}Z")
        }
    }
}

/// The websocket URL a connection token is reached on, as the `Websocket$Url` of this
/// service's `CreateParticipantConnection` and of the connect service's
/// `StartChatContact` — both of which mint a connection token and report the URL for
/// it, so the form lives here and neither spells it itself.
///
/// `host` is the endpoint the URL names; the token travels in the query string, which is
/// how a client that can only open a URL carries it.
pub fn websocket_url(host: &str, connection_token: &str) -> String {
    format!("wss://{host}/participant/websocket?connectionToken={connection_token}")
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsConnectparticipantAmazonconnectparticipantservicelambdaState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsConnectparticipantAmazonconnectparticipantservicelambdaState>)
}
