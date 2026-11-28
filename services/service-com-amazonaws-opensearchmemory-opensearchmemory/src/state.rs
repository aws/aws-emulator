// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-opensearchmemory-opensearchmemory service.
//!
//! One record per *space*, and inside it one record per *event*. Every implemented
//! operation computes its answer from these records: a handler persists what it was
//! given, derives what the service itself owns (ids, the ARN, timestamps, status,
//! generated strategy names, generated session ids) and then renders the stored
//! record. Nothing is echoed straight back out of a request.
//!
//! The pieces several operations share live here rather than being copied into each
//! `src/ops/*.rs`, because `src/lib.rs` is generated and this module is the crate's
//! one shared, editable home: id minting, the clock-owed status moves, the
//! `@idempotencyToken` ledgers, the opaque `ListEvents` cursor, the response
//! projections, and the modelled error payloads each handler lifts into its own
//! operation error enum.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ::aws_smithy_types::{DateTime, Document, Number};

use crate::ops::types::{
    BlobSource, ConflictException, ConversationalPayload, CreateSpaceOutput,
    EncryptionConfiguration, Event, EventDetail, EventPayload, EventSummary, GetSpaceOutput,
    InternalServerException, MemoryStrategy, MemoryStrategyConfiguration,
    ResourceNotFoundException, ServiceQuotaExceededException, StructuredDataSource, Tag,
    UnifiedConversationalSource, ValidationException, ValidationExceptionField,
};

/// This service's ARN namespace (`OPERATIONS.md`, "Wire protocol": `osm`).
pub const ARN_NAMESPACE: &str = "osm";

/// The region an emulated ARN carries when the request names none.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `resourceType` a refusal about a space carries.
pub const SPACE_RESOURCE_TYPE: &str = "space";

/// The `resourceType` a refusal about an event carries.
pub const EVENT_RESOURCE_TYPE: &str = "event";

/// How long a space stays in a transient status before the move the clock owes is
/// materialised: `CREATING` -> `ACTIVE`, and `DELETING` -> gone.
///
/// `CreateSpace`'s own doc says the operation "returns before provisioning finishes,
/// with status CREATING. Call GetSpace until the status is ACTIVE", and `DeleteSpace`'s
/// says it "returns with status DELETING. When deletion finishes, GetSpace returns
/// ResourceNotFoundException" — so both statuses are real and observable. AWS documents
/// no duration for either, so the span is CHOSEN. It is never zero, and nothing sleeps
/// for it: the due instant is stored on the record and [`Space::settle`] materialises
/// the move once the clock has passed it.
pub const TRANSITION_SPAN: Duration = Duration::from_secs(3);

/// How long an `@idempotencyToken` names the result of the request that first used it.
///
/// Both `clientToken` docs state the window: a retry "with the same token and the same
/// parameters within 2 hours returns the space that the first request created"
/// (`CreateSpace`), and "After 2 hours, the token starts a new event" (`CreateEvent`).
pub const IDEMPOTENCY_WINDOW: Duration = Duration::from_secs(2 * 60 * 60);

/// The page `ListEvents` serves when the request names no `maxResults`: its own doc,
/// "The maximum number of events to return, from 1 to 100. The default is 20."
pub const DEFAULT_LIST_EVENTS_PAGE_SIZE: i32 = 20;

/// The bounds of `maxResults` (`smithy_query` reports `range {min: 1, max: 100}`).
pub const MIN_PAGE_SIZE: i32 = 1;
/// See [`MIN_PAGE_SIZE`].
pub const MAX_PAGE_SIZE: i32 = 100;

/// The longest `nextToken` this service will even look at: `PaginationToken`'s
/// modelled `length {min: 1, max: 2048}`.
pub const MAX_PAGE_TOKEN_LEN: usize = 2048;

/// The Amazon Bedrock model ids `extractionModelId` accepts.
///
/// The member's own doc names them: "an Amazon Bedrock model ID without an
/// inference-profile prefix: anthropic.claude-opus-5-5 or anthropic.claude-haiku-5-5.
/// Another value fails with ValidationException, and the message lists the supported
/// IDs."
pub const SUPPORTED_EXTRACTION_MODEL_IDS: &[&str] =
    &["anthropic.claude-haiku-5-5", "anthropic.claude-opus-5-5"];

/// How many distinct metadata keys one space may use across all of its events and
/// memories: the `MetadataMap` doc, "A space can use at most 50 distinct metadata keys
/// across all of its events and memories."
pub const MAX_METADATA_KEYS_PER_SPACE: usize = 50;

/// The metadata key prefix the service reserves for itself: `CreateEvent`'s `metadata`
/// doc, "Keys that begin with x-amz- ... are reserved".
pub const RESERVED_METADATA_KEY_PREFIX: &str = "x-amz-";

/// The individual metadata keys the service reserves: `CreateEvent`'s `metadata` doc,
/// "the keys documentS3Source and documentS3Url".
pub const RESERVED_METADATA_KEYS: &[&str] = &["documentS3Source", "documentS3Url"];

/// How many memory strategies one space may hold: `MemoryStrategyList`'s modelled
/// `length {max: 10}`, restated in the member's doc as "up to 10 strategies".
pub const MAX_MEMORY_STRATEGIES: usize = 10;

/// The most spaces this store holds at once, after which `CreateSpace` answers the
/// `ServiceQuotaExceededException` it models ("the number of spaces in your account").
///
/// AWS publishes no number for it, so this one is the EMULATOR's own ceiling, and it is
/// what bounds the store: a caller looping `CreateSpace` cannot grow the shared store
/// past it. A `DeleteSpace` frees a slot.
pub const MAX_SPACES: usize = 50;

/// The most events ONE space holds at once. The emulator's own ceiling: together with
/// [`MAX_SPACES`] it bounds everything this store keeps.
pub const MAX_EVENTS_PER_SPACE: usize = 1_000;

/// The most `@idempotencyToken` entries ONE ledger remembers at once.
///
/// A token entry outlives neither [`IDEMPOTENCY_WINDOW`] nor this many newer tokens, so
/// a caller looping creates with fresh tokens cannot grow the ledger without bound. The
/// oldest entry is dropped first, which is the one whose window is closest to expiring.
pub const MAX_TRACKED_CLIENT_TOKENS: usize = 256;

/// How deep [`canonical_document`] renders a `document` member before it stops.
///
/// A fingerprint is only ever compared with another fingerprint, so the one thing this
/// bound costs is that two payloads differing ONLY below this depth look alike to an
/// idempotency replay. It keeps a deeply nested `document` a caller sent from recursing
/// without a limit.
const MAX_DOCUMENT_DEPTH: usize = 64;

/// The status of a space: the closed set the model declares (`smithy_query
/// query="shape" shape="SpaceStatus"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpaceStatus {
    /// The space is being created.
    Creating,
    /// The space is ready to use.
    Active,
    /// A change to the space is being applied. Nothing reports this yet: `UpdateSpace`
    /// is still scaffolded.
    Updating,
    /// The space is being deleted.
    Deleting,
    /// Never reported: the model's own doc for this value says that after a space is
    /// deleted `GetSpace` answers `ResourceNotFoundException`. It names the TARGET of
    /// the move `DeleteSpace` starts, which purges the record.
    Deleted,
    /// Creating, updating or deleting the space failed; `statusReason` says why.
    Failed,
}

impl SpaceStatus {
    /// The value this status takes on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::Active => "ACTIVE",
            Self::Updating => "UPDATING",
            Self::Deleting => "DELETING",
            Self::Deleted => "DELETED",
            Self::Failed => "FAILED",
        }
    }
}

/// A status move the clock owes: at `due`, the space becomes `target`.
#[derive(Debug, Clone, Copy)]
pub struct PendingSpaceChange {
    pub due: SystemTime,
    pub target: SpaceStatus,
}

/// One stored event of a space.
#[derive(Debug, Clone)]
pub struct StoredEvent {
    pub event_id: String,
    pub space_id: String,
    pub actor_id: String,
    /// The session the event belongs to: the one the request named, or the one the
    /// service generated for it.
    pub session_id: String,
    pub event_timestamp_millis: i64,
    pub payload: EventPayload,
    pub metadata: BTreeMap<String, String>,
    /// Where this event falls in its space's creation order. It breaks the tie when two
    /// events carry the same `eventTimestamp`, so `ListEvents` pages deterministically.
    pub seq: u64,
}

impl StoredEvent {
    /// This event's position in a `ListEvents` ordering.
    pub fn sort_key(&self) -> (i64, u64) {
        (self.event_timestamp_millis, self.seq)
    }

    /// The metadata as the wire map. Present but empty for an event that carries none:
    /// the member's own doc says it "May be empty".
    pub fn metadata_map(&self) -> Option<HashMap<String, String>> {
        Some(
            self.metadata
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        )
    }

    /// The `EventDetail` `CreateEvent` answers, read off the record.
    pub fn detail(&self) -> EventDetail {
        EventDetail {
            event_id: self.event_id.clone(),
            space_id: self.space_id.clone(),
            actor_id: self.actor_id.clone(),
            session_id: self.session_id.clone(),
            event_timestamp: DateTime::from_millis(self.event_timestamp_millis),
        }
    }

    /// The `Event` `GetEvent` answers, read off the record — payload included.
    pub fn event(&self) -> Event {
        Event {
            event_id: self.event_id.clone(),
            space_id: self.space_id.clone(),
            actor_id: self.actor_id.clone(),
            session_id: self.session_id.clone(),
            payload: self.payload.clone(),
            metadata: self.metadata_map(),
            event_timestamp: DateTime::from_millis(self.event_timestamp_millis),
        }
    }

    /// The `EventSummary` `ListEvents` answers: the same record without its payload.
    pub fn summary(&self) -> EventSummary {
        EventSummary {
            event_id: self.event_id.clone(),
            metadata: self.metadata_map(),
            event_timestamp: DateTime::from_millis(self.event_timestamp_millis),
        }
    }
}

/// A stored space: its configuration, its status, and its events.
#[derive(Debug, Clone)]
pub struct Space {
    pub space_id: String,
    pub arn: String,
    pub name: String,
    pub status: SpaceStatus,
    pub status_reason: Option<String>,
    pub created_at_millis: i64,
    /// The strategies as STORED — every one carrying a name, generated from its type
    /// where the request omitted it.
    pub memory_strategies: Vec<MemoryStrategy>,
    pub event_expiry_days: Option<i32>,
    pub memory_expiry_days: Option<i32>,
    pub extraction_model_id: Option<String>,
    pub encryption_configuration: Option<EncryptionConfiguration>,
    pub tags: Vec<Tag>,
    /// Events by `eventId`.
    pub events: BTreeMap<String, StoredEvent>,
    /// Every distinct metadata key this space has ever used, which is what
    /// [`MAX_METADATA_KEYS_PER_SPACE`] bounds.
    pub metadata_keys: BTreeSet<String>,
    /// Set while a status move the clock owes has not come due yet.
    pub pending: Option<PendingSpaceChange>,
    /// Source of this space's `eventId`s and of [`StoredEvent::seq`]. Advanced only
    /// through [`Space::mint_event_id`], so it never goes backwards.
    pub next_event_seq: u64,
}

impl Space {
    /// Materialise the status move the clock has ALREADY made due.
    ///
    /// Answers `true` when the record is due to be PURGED — the end of the move
    /// `DeleteSpace` starts, after which `GetSpace` answers
    /// `ResourceNotFoundException`.
    pub fn settle(&mut self, now: SystemTime) -> bool {
        let Some(pending) = self.pending else {
            return false;
        };
        if now < pending.due {
            return false;
        }
        self.pending = None;
        if pending.target == SpaceStatus::Deleted {
            return true;
        }
        self.status = pending.target;
        false
    }

    /// Whether this space accepts new events: only a space that finished provisioning
    /// does. `CreateSpace`'s doc: "The operation returns before provisioning finishes,
    /// with status CREATING. Call GetSpace until the status is ACTIVE, and then use the
    /// space."
    pub fn accepts_events(&self) -> bool {
        self.status == SpaceStatus::Active
    }

    /// Whether this space still serves reads of its events.
    ///
    /// A space that has not finished provisioning has none, and `DeleteSpace`'s doc
    /// says that "After you delete the space, you can't retrieve its events" — so a
    /// space being deleted serves none either, even while `GetSpace` still reports
    /// `DELETING`.
    pub fn serves_events(&self) -> bool {
        matches!(self.status, SpaceStatus::Active | SpaceStatus::Updating)
    }

    /// Mint the next `eventId` of this space, with the sequence number that orders it.
    ///
    /// Stays inside `EventId`'s modelled `length {min: 1, max: 128}`.
    pub fn mint_event_id(&mut self) -> (String, u64) {
        self.next_event_seq += 1;
        (format!("ev-{:012x}", self.next_event_seq), self.next_event_seq)
    }

    /// The `CreateSpaceOutput` this record renders.
    pub fn create_output(&self) -> CreateSpaceOutput {
        CreateSpaceOutput {
            space_id: self.space_id.clone(),
            arn: self.arn.clone(),
            name: self.name.clone(),
            status: self.status.as_str().to_string(),
            created_at: DateTime::from_millis(self.created_at_millis),
        }
    }

    /// The `GetSpaceOutput` this record renders: the identifiers plus every
    /// configuration member `CreateSpace` persisted.
    pub fn describe(&self) -> GetSpaceOutput {
        GetSpaceOutput {
            space_id: self.space_id.clone(),
            arn: self.arn.clone(),
            name: self.name.clone(),
            status: self.status.as_str().to_string(),
            status_reason: self.status_reason.clone(),
            created_at: DateTime::from_millis(self.created_at_millis),
            // "Empty for a space that has no memory strategies" (`memoryStrategies`
            // doc), so the member is present rather than absent.
            memory_strategies: Some(self.memory_strategies.clone()),
            event_expiry_days: self.event_expiry_days,
            memory_expiry_days: self.memory_expiry_days,
            extraction_model_id: self.extraction_model_id.clone(),
            encryption_configuration: self.encryption_configuration.clone(),
        }
    }
}

/// What a `CreateSpace` `clientToken` names: the space the first request created, and
/// the parameters it was used with.
#[derive(Debug, Clone)]
pub struct CreateSpaceToken {
    pub space_id: String,
    /// The canonical form of the request's parameters ([`create_space_fingerprint`]).
    /// A retry carrying a different one is a conflict, not a replay.
    pub fingerprint: String,
    pub recorded_at: SystemTime,
}

/// What a `CreateEvent` `clientToken` names: the event the first request created, and
/// the parameters it was used with.
#[derive(Debug, Clone)]
pub struct CreateEventToken {
    pub space_id: String,
    pub event_id: String,
    /// Whether the first request NAMED a `sessionId`. When it did not, the service
    /// generates a new one on every attempt, so a retry is a conflict rather than a
    /// replay — the member's own doc says so.
    pub named_session: bool,
    /// The canonical form of the request's parameters ([`create_event_fingerprint`]).
    pub fingerprint: String,
    pub recorded_at: SystemTime,
}

/// Per-service state for com-amazonaws-opensearchmemory-opensearchmemory. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsOpensearchmemoryOpensearchmemoryState {
    /// Spaces by `spaceId`. A `BTreeMap` so a listing pages deterministically.
    pub spaces: BTreeMap<String, Space>,
    /// `CreateSpace` idempotency ledger, by `clientToken`.
    pub create_space_tokens: BTreeMap<String, CreateSpaceToken>,
    /// `CreateEvent` idempotency ledger, by `clientToken`.
    pub create_event_tokens: BTreeMap<String, CreateEventToken>,
    /// Source of the `spaceId`s this store mints.
    next_space_seq: u64,
    /// Source of the `sessionId`s this store generates for a `CreateEvent` that names
    /// none.
    next_session_seq: u64,
}

impl ComAmazonawsOpensearchmemoryOpensearchmemoryState {
    /// Materialise everything the clock has ALREADY made due, so no operation reports a
    /// status the clock has moved on from or honours a client token whose window has
    /// closed. Each of these was owed at this instant whether or not anybody called.
    pub fn settle_due(&mut self, now: SystemTime) {
        self.spaces.retain(|_, space| !space.settle(now));
        let window = |recorded_at: SystemTime| {
            now.duration_since(recorded_at)
                .is_ok_and(|age| age >= IDEMPOTENCY_WINDOW)
        };
        self.create_space_tokens
            .retain(|_, token| !window(token.recorded_at));
        self.create_event_tokens
            .retain(|_, token| !window(token.recorded_at));
    }

    /// Mint the next `spaceId`: `sp-` and 12 lowercase hex digits, which is
    /// `SpaceId`'s modelled `^sp-[a-f0-9]{12}$`.
    pub fn mint_space_id(&mut self) -> String {
        self.next_space_seq += 1;
        format!("sp-{:012x}", self.next_space_seq)
    }

    /// Generate a `sessionId` for a `CreateEvent` that named none. Stays inside
    /// `SessionId`'s modelled `^[a-zA-Z0-9][a-zA-Z0-9-_]*$` and `length 1..255`.
    pub fn mint_session_id(&mut self) -> String {
        self.next_session_seq += 1;
        format!("session-{:012x}", self.next_session_seq)
    }

    /// Record what a `CreateSpace` `clientToken` now names, dropping the oldest entry
    /// when the ledger is full.
    pub fn remember_create_space_token(&mut self, token: String, record: CreateSpaceToken) {
        evict_oldest(&mut self.create_space_tokens, |entry| entry.recorded_at);
        self.create_space_tokens.insert(token, record);
    }

    /// Record what a `CreateEvent` `clientToken` now names, dropping the oldest entry
    /// when the ledger is full.
    pub fn remember_create_event_token(&mut self, token: String, record: CreateEventToken) {
        evict_oldest(&mut self.create_event_tokens, |entry| entry.recorded_at);
        self.create_event_tokens.insert(token, record);
    }
}

/// Make room in a client-token ledger: while it holds [`MAX_TRACKED_CLIENT_TOKENS`] or
/// more, drop the entry recorded longest ago.
fn evict_oldest<T>(ledger: &mut BTreeMap<String, T>, recorded_at: impl Fn(&T) -> SystemTime) {
    while ledger.len() >= MAX_TRACKED_CLIENT_TOKENS {
        let Some(oldest) = ledger
            .iter()
            .min_by_key(|(key, entry)| (recorded_at(entry), (*key).clone()))
            .map(|(key, _)| key.clone())
        else {
            return;
        };
        ledger.remove(&oldest);
    }
}

/// Epoch milliseconds for an instant, the unit the stored timestamps use.
pub fn epoch_millis_of(at: SystemTime) -> i64 {
    at.duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|since_epoch| i64::try_from(since_epoch.as_millis()).ok())
        .unwrap_or_default()
}

/// The epoch milliseconds a wire `Timestamp` names, rounded to the nearest millisecond.
///
/// Saturating throughout, so an absurd timestamp a request carries yields a far-away
/// millisecond count rather than overflowing.
pub fn epoch_millis(timestamp: &DateTime) -> i64 {
    timestamp
        .secs()
        .saturating_mul(1000)
        .saturating_add(i64::from(
            (timestamp.subsec_nanos() + 500_000) / 1_000_000,
        ))
}

/// The ARN of a space in `region`: `arn:aws:osm:<region>:<account>:space/<spaceId>`,
/// which is `SpaceArn`'s modelled pattern.
pub fn space_arn(region: &str, space_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("space/{space_id}"))
}

/// The failure a handler answers an unreachable store with.
///
/// Nothing about the request caused it and no member of the request can be changed to
/// avoid it, so it is reported through the shape this service models for a server-side
/// failure rather than through a client-input refusal.
pub fn store_unreachable(operation: &str) -> InternalServerException {
    InternalServerException {
        message: format!("{operation} could not reach this service's store."),
    }
}

/// The refusal for a `spaceId` this store holds no record for.
pub fn space_not_found(space_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("Space {space_id} doesn't exist."),
        resource_id: Some(space_id.to_string()),
        resource_type: Some(SPACE_RESOURCE_TYPE.to_string()),
    }
}

/// The refusal for an `eventId` a space holds no record for — also the answer when a
/// supplied `actorId` or `sessionId` does not match the stored event, which the
/// members' own docs word as "as if the event did not exist".
pub fn event_not_found(space_id: &str, event_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("Event {event_id} doesn't exist in space {space_id}."),
        resource_id: Some(event_id.to_string()),
        resource_type: Some(EVENT_RESOURCE_TYPE.to_string()),
    }
}

/// The refusal for an operation that needs a space to have finished what it is doing.
pub fn space_busy(space: &Space) -> ConflictException {
    ConflictException {
        message: format!(
            "Space {} is {}. Wait for it to finish, and then retry.",
            space.space_id,
            space.status.as_str()
        ),
        resource_id: Some(space.space_id.clone()),
        resource_type: Some(SPACE_RESOURCE_TYPE.to_string()),
    }
}

/// The refusal for a space name another space in the account already holds.
pub fn duplicate_space_name(name: &str, space_id: &str) -> ConflictException {
    ConflictException {
        message: format!("A space named {name} already exists in this account in this Region."),
        resource_id: Some(space_id.to_string()),
        resource_type: Some(SPACE_RESOURCE_TYPE.to_string()),
    }
}

/// The refusal for a client token reused with different parameters.
pub fn token_reused(token: &str, resource_id: &str) -> ConflictException {
    ConflictException {
        message: format!(
            "The client token {token} was already used with different parameters. \
             Use a different client token, and then retry."
        ),
        resource_id: Some(resource_id.to_string()),
        resource_type: Some(SPACE_RESOURCE_TYPE.to_string()),
    }
}

/// The quota refusal for a create that would push this store past [`MAX_SPACES`].
pub fn space_quota_exceeded() -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: format!(
            "The request would exceed the quota of {MAX_SPACES} spaces in this account \
             in this Region."
        ),
        quota_code: None,
        service_code: None,
    }
}

/// A client-input refusal naming the member that was wrong, in the shape this service
/// models for one: `message`, the `reason` enum, and the field list a client reads.
pub fn invalid_field(
    field: &str,
    message: impl Into<String>,
    reason: &str,
) -> ValidationException {
    let message = message.into();
    ValidationException {
        message: message.clone(),
        reason: Some(reason.to_string()),
        field_list: Some(vec![ValidationExceptionField {
            name: field.to_string(),
            message,
        }]),
    }
}

/// Resolve the memory strategies a `CreateSpace` request carries into the strategies a
/// space STORES, or the refusal they earn.
///
/// Three rules, all from the `MemoryStrategy` shape's own docs: a strategy's name is
/// "unique within the space"; a strategy that omits one has the service "generate one
/// from the type, for example `semantic_1`"; and "A `CUSTOM` strategy must set
/// `extractionPrompt`".
pub fn resolve_memory_strategies(
    requested: Vec<MemoryStrategy>,
) -> Result<Vec<MemoryStrategy>, ValidationException> {
    if requested.len() > MAX_MEMORY_STRATEGIES {
        return Err(invalid_field(
            "memoryStrategies",
            format!(
                "A space holds at most {MAX_MEMORY_STRATEGIES} memory strategies, but {} \
                 were requested.",
                requested.len()
            ),
            "fieldValidationFailed",
        ));
    }

    let mut used: BTreeSet<String> = BTreeSet::new();
    for strategy in &requested {
        let Some(name) = strategy.name.as_deref() else {
            continue;
        };
        if !used.insert(name.to_string()) {
            return Err(invalid_field(
                "memoryStrategies",
                format!("A memory strategy named {name} is already in this space."),
                "fieldValidationFailed",
            ));
        }
    }

    let mut resolved = Vec::with_capacity(requested.len());
    for mut strategy in requested {
        if strategy.r#type == "CUSTOM"
            && strategy
                .configuration
                .as_ref()
                .and_then(|configuration| configuration.extraction_prompt.as_deref())
                .is_none_or(str::is_empty)
        {
            return Err(invalid_field(
                "memoryStrategies",
                "A CUSTOM memory strategy must set configuration.extractionPrompt.",
                "fieldValidationFailed",
            ));
        }
        if strategy.name.is_none() {
            let stem = strategy.r#type.to_lowercase();
            let mut ordinal = 1_usize;
            let generated = loop {
                let candidate = format!("{stem}_{ordinal}");
                if !used.contains(&candidate) {
                    break candidate;
                }
                ordinal += 1;
            };
            used.insert(generated.clone());
            strategy.name = Some(generated);
        }
        resolved.push(strategy);
    }
    Ok(resolved)
}

/// Refuse an `extractionModelId` outside the set the member's doc names, with the
/// supported ids in the message as that doc requires.
pub fn check_extraction_model_id(model_id: Option<&str>) -> Result<(), ValidationException> {
    let Some(model_id) = model_id else {
        return Ok(());
    };
    if SUPPORTED_EXTRACTION_MODEL_IDS.contains(&model_id) {
        return Ok(());
    }
    Err(invalid_field(
        "extractionModelId",
        format!(
            "extractionModelId {model_id} isn't supported. Supported model IDs: {}.",
            SUPPORTED_EXTRACTION_MODEL_IDS.join(", ")
        ),
        "fieldValidationFailed",
    ))
}

/// The only document media type a `blob` payload may carry: the `BlobPayload.contentType`
/// doc, "Only `application/pdf` is supported. Any other value fails with
/// `ValidationException`." (`BlobContentType` still ACCEPTS `image/png` and `image/jpeg`
/// on the wire, which is why the refusal belongs to the handler and not to the boundary.)
pub const SUPPORTED_BLOB_CONTENT_TYPE: &str = "application/pdf";

/// Refuse an event payload the service cannot store, in the one place both the reason
/// and the member name are known.
pub fn check_payload(payload: &EventPayload) -> Result<(), ValidationException> {
    if let EventPayload::Blob(blob) = payload {
        if blob.content_type != SUPPORTED_BLOB_CONTENT_TYPE {
            return Err(invalid_field(
                "payload.blob.contentType",
                format!(
                    "Content type {} isn't supported. Only {SUPPORTED_BLOB_CONTENT_TYPE} is.",
                    blob.content_type
                ),
                "unsupportedField",
            ));
        }
    }
    Ok(())
}

/// Refuse a metadata key the service reserves for itself, naming the key as
/// `CreateEvent`'s `metadata` doc requires.
pub fn check_metadata_keys<'a>(
    keys: impl IntoIterator<Item = &'a String>,
) -> Result<(), ValidationException> {
    for key in keys {
        if key.starts_with(RESERVED_METADATA_KEY_PREFIX)
            || RESERVED_METADATA_KEYS.contains(&key.as_str())
        {
            return Err(invalid_field(
                "metadata",
                format!("The metadata key {key} is reserved by the service."),
                "fieldValidationFailed",
            ));
        }
    }
    Ok(())
}

/// The canonical text of a `CreateSpace` request's parameters: what an
/// `@idempotencyToken` replay is compared against.
///
/// The RESOLVED strategies go in, so a retry that omits a strategy name the service
/// generated still matches the first request.
pub fn create_space_fingerprint(
    name: &str,
    strategies: &[MemoryStrategy],
    event_expiry_days: Option<i32>,
    memory_expiry_days: Option<i32>,
    extraction_model_id: Option<&str>,
    encryption_configuration: Option<&EncryptionConfiguration>,
    tags: &[Tag],
) -> String {
    let mut out = String::new();
    push_field(&mut out, "name", name);
    push_field(&mut out, "eventExpiryDays", &render_opt_i32(event_expiry_days));
    push_field(
        &mut out,
        "memoryExpiryDays",
        &render_opt_i32(memory_expiry_days),
    );
    push_field(
        &mut out,
        "extractionModelId",
        extraction_model_id.unwrap_or("-"),
    );
    push_field(
        &mut out,
        "kmsKeyArn",
        encryption_configuration
            .and_then(|configuration| configuration.kms_key_arn.as_deref())
            .unwrap_or("-"),
    );
    for strategy in strategies {
        push_field(&mut out, "strategy", &render_strategy(strategy));
    }
    // Tags are a SET on the wire, so they are compared in key order rather than in the
    // order the request happened to list them.
    let ordered: BTreeMap<&str, &str> = tags
        .iter()
        .map(|tag| (tag.key.as_str(), tag.value.as_str()))
        .collect();
    for (key, value) in ordered {
        push_field(&mut out, "tag", &format!("{key}={value}"));
    }
    out
}

/// The canonical text of a `CreateEvent` request's parameters.
///
/// `eventTimestamp` is deliberately absent: the member's own doc says "eventTimestamp
/// isn't compared".
pub fn create_event_fingerprint(
    space_id: &str,
    actor_id: &str,
    session_id: Option<&str>,
    payload: &EventPayload,
    metadata: &BTreeMap<String, String>,
    infer: Option<bool>,
) -> String {
    let mut out = String::new();
    push_field(&mut out, "spaceId", space_id);
    push_field(&mut out, "actorId", actor_id);
    push_field(&mut out, "sessionId", session_id.unwrap_or("-"));
    push_field(
        &mut out,
        "infer",
        match infer {
            None => "-",
            Some(true) => "true",
            Some(false) => "false",
        },
    );
    push_field(&mut out, "payload", &render_payload(payload));
    for (key, value) in metadata {
        push_field(&mut out, "metadata", &format!("{key}={value}"));
    }
    out
}

/// Append one length-delimited field to a fingerprint, so no two different member sets
/// can render to the same text.
fn push_field(out: &mut String, name: &str, value: &str) {
    out.push_str(name);
    out.push(':');
    out.push_str(&value.len().to_string());
    out.push(':');
    out.push_str(value);
    out.push(';');
}

fn render_opt_i32(value: Option<i32>) -> String {
    value.map_or_else(|| "-".to_string(), |value| value.to_string())
}

fn render_strategy(strategy: &MemoryStrategy) -> String {
    let mut out = String::new();
    push_field(&mut out, "type", &strategy.r#type);
    push_field(&mut out, "name", strategy.name.as_deref().unwrap_or("-"));
    push_field(
        &mut out,
        "description",
        strategy.description.as_deref().unwrap_or("-"),
    );
    if let Some(configuration) = strategy.configuration.as_ref() {
        push_field(&mut out, "configuration", &render_configuration(configuration));
    }
    out
}

fn render_configuration(configuration: &MemoryStrategyConfiguration) -> String {
    let mut out = String::new();
    push_field(
        &mut out,
        "extractionPrompt",
        configuration.extraction_prompt.as_deref().unwrap_or("-"),
    );
    for (name, value) in [
        ("extractEveryNMessages", configuration.extract_every_n_messages),
        ("extractEveryNTokens", configuration.extract_every_n_tokens),
        ("idleTimeoutSeconds", configuration.idle_timeout_seconds),
        ("extractionSeedTopK", configuration.extraction_seed_top_k),
        ("extractionSeedRecentN", configuration.extraction_seed_recent_n),
    ] {
        push_field(&mut out, name, &render_opt_i32(value));
    }
    push_field(
        &mut out,
        "consolidationEnabled",
        match configuration.consolidation_enabled {
            None => "-",
            Some(true) => "true",
            Some(false) => "false",
        },
    );
    out
}

/// The canonical text of an event payload: the union member that is set, and its
/// content.
fn render_payload(payload: &EventPayload) -> String {
    let mut out = String::new();
    match payload {
        EventPayload::Conversational(ConversationalPayload::Unified(unified)) => {
            push_field(&mut out, "member", "conversational");
            match &unified.source {
                UnifiedConversationalSource::Object(object) => {
                    for message in &object.messages {
                        push_field(
                            &mut out,
                            "message",
                            &format!("{}={}", message.role, message.content.text),
                        );
                    }
                }
            }
        }
        EventPayload::structuredData(structured) => {
            push_field(&mut out, "member", "structuredData");
            match &structured.source {
                StructuredDataSource::Json(document) => {
                    let mut rendered = String::new();
                    canonical_document(document, 0, &mut rendered);
                    push_field(&mut out, "json", &rendered);
                }
            }
        }
        EventPayload::Blob(blob) => {
            push_field(&mut out, "member", "blob");
            push_field(&mut out, "contentType", &blob.content_type);
            match &blob.source {
                BlobSource::s3Location(location) => {
                    push_field(&mut out, "uri", &location.uri);
                }
            }
        }
    }
    out
}

/// Render a `document` member canonically: object members in SORTED key order, so two
/// documents holding the same content render identically whatever order the underlying
/// `HashMap` iterates in.
pub fn canonical_document(value: &Document, depth: usize, out: &mut String) {
    if depth > MAX_DOCUMENT_DEPTH {
        out.push('…');
        return;
    }
    match value {
        Document::Null => out.push_str("null"),
        Document::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
        Document::String(text) => {
            out.push('"');
            out.push_str(&text.len().to_string());
            out.push(':');
            out.push_str(text);
            out.push('"');
        }
        Document::Number(number) => {
            out.push('#');
            out.push_str(&number_bits(number).to_string());
        }
        Document::Array(items) => {
            out.push('[');
            for item in items {
                canonical_document(item, depth + 1, out);
                out.push(',');
            }
            out.push(']');
        }
        Document::Object(members) => {
            let ordered: BTreeMap<&String, &Document> = members.iter().collect();
            out.push('{');
            for (key, member) in ordered {
                out.push_str(&key.len().to_string());
                out.push(':');
                out.push_str(key);
                out.push('=');
                canonical_document(member, depth + 1, out);
                out.push(',');
            }
            out.push('}');
        }
    }
}

/// A `document` number as the bits of its `f64` value, so the three `Number` variants
/// that carry the same value render identically and no float comparison is needed.
fn number_bits(number: &Number) -> u64 {
    match number {
        Number::PosInt(value) => (*value as f64).to_bits(),
        Number::NegInt(value) => (*value as f64).to_bits(),
        Number::Float(value) => value.to_bits(),
    }
}

/// The collection a `ListEvents` cursor is issued FOR: a token minted for one
/// actor/session of one space, in one sort order, names a position in THAT listing and
/// in no other.
pub fn list_events_scope(space_id: &str, actor_id: &str, session_id: &str, order: &str) -> String {
    format!("events/{space_id}/{actor_id}/{session_id}/{order}")
}

/// The opaque `nextToken` for a page of `scope` that stopped at `last`.
///
/// Hex of `<scope>|<millis>|<seq>`, so the token stays opaque to a client yet resumes
/// EXACTLY where the page stopped: the next page starts at the first event strictly
/// after that position in the requested order, whether or not that event still exists.
pub fn encode_page_token(scope: &str, last: (i64, u64)) -> String {
    let (millis, seq) = last;
    ::hex::encode(format!("{scope}|{millis}|{seq}").as_bytes())
}

/// The position a `nextToken` resumes after in `scope`, or `None` when the token is not
/// one this service issued FOR that listing.
///
/// Only the ONE spelling [`encode_page_token`] writes for a position names that
/// position: a token is accepted only when it is byte-identical to the token this
/// service would mint for the scope and position it decodes to. The hex decode is laxer
/// than the hex encode — it reads `A-F` as well as the `a-f` the minter emits — so
/// without that comparison a re-cased copy of a cursor, a string no response of this
/// service ever carried, would resume a listing.
pub fn decode_page_token(scope: &str, token: &str) -> Option<(i64, u64)> {
    if token.len() > MAX_PAGE_TOKEN_LEN {
        return None;
    }
    let plaintext = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    let mut fields = plaintext.rsplitn(3, '|');
    let seq = fields.next()?.parse::<u64>().ok()?;
    let millis = fields.next()?.parse::<i64>().ok()?;
    let position = (millis, seq);
    (encode_page_token(scope, position) == token).then_some(position)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with the modelled server error [`store_unreachable`] builds, never with
/// a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsOpensearchmemoryOpensearchmemoryState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsOpensearchmemoryOpensearchmemoryState>)
}
