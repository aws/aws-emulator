// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-bedrockagentcore-amazonbedrockagentcore service.
//!
//! The AgentCore Memory *event* data plane is stored here: a memory container holds
//! actors, an actor holds sessions, and a session holds its events in the order they
//! were created. Every one of `CreateEvent` / `ListSessions` / `ListEvents` /
//! `GetEvent` / `DeleteEvent` computes its answer from these maps; nothing is canned.
//!
//! A session also holds the content `IngestData` submitted for long-term extraction,
//! in the same (memory, actor, session) tree: that content is not a short-term event,
//! so it is kept beside a session's events rather than among them, and a session that
//! only ever received ingested content exists with no events of its own.
//!
//! A memory container also holds the LONG-TERM memory records a caller created with
//! its own content (`BatchCreateMemoryRecords`), keyed by the `memoryRecordId` this
//! emulator minted for each one. A record belongs to the container, not to an actor or
//! a session, so it lives beside the actors rather than under one, and
//! `GetMemoryRecord` / `DeleteMemoryRecord` address it by that id alone.
//!
//! The AgentCore *tools* data plane keeps its own map: a code interpreter holds the
//! sessions `StartCodeInterpreterSession` started on it, and
//! `GetCodeInterpreterSession` reports one of them back — its status read off the
//! clock against the time-to-live the request asked for, or off the stop that already
//! terminated it ([`CodeInterpreterSessionRecord::status_as_of`]).
//!
//! A session also owns the WORKSPACE its tools read and write: the files
//! `InvokeCodeInterpreter`'s file tools put in it, kept in memory beside the session
//! record and reachable from no other session. Nothing of it touches a host path: a
//! tool addresses a file by a workspace-relative key ([`normalize_session_path`]), and
//! the bytes live in [`SessionFile`].
//!
//! The AgentCore *identity* plane keeps a third: every workload access token this
//! service issued, in [`IssuedWorkloadAccessTokens`]. A token is MINTED by the call that
//! asks for one ([`mint_workload_access_token`]) and recorded here with the workload it
//! names, the user it acts for and the window it is valid in, so this service can later
//! say which workload and user a token it handed out stands for rather than trusting the
//! bytes a caller presents. The registry is bounded by the expiry of each record and by
//! [`MAX_ISSUED_WORKLOAD_ACCESS_TOKENS`], both materialised on the WRITE path.
//!
//! # The store has an explicit ceiling
//!
//! `CreateEvent` and `IngestData` are the memory plane's growth sites, and a caller
//! chooses every key they insert
//! under (`memoryId`, `actorId`, `sessionId`, `clientToken`), so request volume alone
//! must not be able to grow the process heap without limit. Each level therefore
//! carries a maximum and evicts its least recently written record once that maximum is
//! passed ([`MAX_MEMORIES`], [`MAX_ACTORS_PER_MEMORY`], [`MAX_SESSIONS_PER_ACTOR`],
//! [`MAX_EVENTS_PER_SESSION`], [`MAX_IDEMPOTENCY_RECORDS_PER_MEMORY`]), and the
//! payload-carrying records are additionally bounded ACROSS the whole store by
//! [`MAX_STORED_EVENTS`], a ring over every stored event in insertion order. So the
//! store holds at most `MAX_STORED_EVENTS` events (the only records whose size a
//! caller influences, each itself bounded by the modelled `payload` <= 100 items and
//! `metadata` <= 15 entries) and at most
//! `MAX_MEMORIES * MAX_ACTORS_PER_MEMORY * MAX_SESSIONS_PER_ACTOR` session records,
//! whatever a caller sends. Ingested content is bounded the same way, by
//! [`MAX_INGESTIONS_PER_SESSION`] per session and [`MAX_STORED_INGESTIONS`] across the
//! store. AWS publishes no retention for any of this, so these are
//! the EMULATOR's own bounds, set far above any test or client working set.
//!
//! `StartCodeInterpreterSession` is the tools plane's only growth site, and a caller
//! chooses its `codeInterpreterIdentifier` and `clientToken`, so it is bounded the
//! same way: [`MAX_CODE_INTERPRETERS`], [`MAX_CODE_INTERPRETER_SESSIONS`] and
//! [`MAX_IDEMPOTENCY_RECORDS_PER_CODE_INTERPRETER`], each evicting its least recently
//! written record.
//!
//! A session's workspace is a growth site too — `InvokeCodeInterpreter`'s `writeFiles`
//! tool chooses both the paths and the bytes — so it is bounded on the WRITE path by
//! [`MAX_SESSION_FILES`] and [`MAX_SESSION_FILE_BYTES`], and a write that would pass
//! either is refused rather than evicting a file a caller still expects to read. Those
//! two bound ONE session, so the payload-carrying tools-plane state is bounded ACROSS the
//! whole store as well, by [`MAX_STORED_SESSION_BYTES`] over every session's workspace and
//! execution context together — the tools plane's equivalent of [`MAX_STORED_EVENTS`],
//! without which the product of the per-session ceilings and the session counts would be
//! what a caller looping on start-session and `writeFiles` could grow this process to.
//! Each stop a `clientToken` replays is recorded too, bounded by
//! [`MAX_STOP_TOKENS_PER_SESSION`].
//!
//! The BROWSER tools plane is its own concern and lives in [`browser`]: a browser holds
//! the sessions `StartBrowserSession` started on it, read back by `GetBrowserSession`,
//! bounded the same way as the code interpreter plane.
//!
//! The AgentCore A/B TESTING plane is a concern of its own, so it lives in
//! [`ab_tests`]: the tests `CreateABTest` minted, what `ListABTests` reports from them,
//! and the bound that plane grows under ([`MAX_AB_TESTS`]).
//!
//! The AgentCore PAYMENTS plane is a concern of its own too, in [`payment_sessions`]: the
//! payment sessions `CreatePaymentSession` opened under a payment manager, what the reads
//! of the plane report from them, and the bound it grows under ([`MAX_PAYMENT_SESSIONS`]).
//! A payment manager itself belongs to the CONTROL plane, so this plane resolves one
//! against that plane's own store ([`payment_manager_resolved`]) rather than its own
//! records, exactly as the browser plane resolves a browser.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::{Duration, SystemTime};

mod ab_tests;
pub use ab_tests::*;

/// The batch-evaluation plane: [`BatchEvaluationRecord`], the job records
/// `StartBatchEvaluation` writes and `GetBatchEvaluation` reports back.
mod batch_evaluations;
pub use batch_evaluations::*;

/// The browser tools plane: [`BrowserStore`] and [`BrowserSessionRecord`], plus the
/// lookups and the write path the browser session operations go through.
mod browser;
pub use browser::*;

/// The long-term memory extraction plane: [`ExtractionJobUnit`], the units of extraction
/// work a memory container holds, derived from the messages its events carry.
mod extraction_jobs;
pub use extraction_jobs::*;

/// The payments plane: [`PaymentSessionRecord`], the payment sessions
/// `CreatePaymentSession` opens under a payment manager and every later read of one reports.
mod payment_sessions;
pub use payment_sessions::*;

use crate::ops::types::{
    Branch, Certificate, Event, ExtractionConfig, IngestPayloadType, MemoryContent, MemoryRecord,
    MemoryRecordMetadataValue, MemoryRecordOutput, MetadataValue, PayloadType, SessionSummary,
    ToolsFileSystemConfiguration,
};

/// The most memory containers this emulator retains; the least recently written one
/// is evicted once the store passes it.
pub const MAX_MEMORIES: usize = 32;

/// The most actors this emulator retains inside one memory container.
pub const MAX_ACTORS_PER_MEMORY: usize = 32;

/// The most sessions this emulator retains for one actor.
pub const MAX_SESSIONS_PER_ACTOR: usize = 64;

/// The most events this emulator retains in one session. It also bounds the work a
/// single `ListEvents` does: filtering and sorting walk one session's events.
pub const MAX_EVENTS_PER_SESSION: usize = 1_000;

/// The most events this emulator retains across EVERY session, as a ring in
/// insertion order: the oldest event (and the now-empty session, actor and memory it
/// was the last of) is dropped once the store passes it.
pub const MAX_STORED_EVENTS: usize = 10_000;

/// The most ingested-content records this emulator retains in one session.
pub const MAX_INGESTIONS_PER_SESSION: usize = 1_000;

/// The most ingested-content records this emulator retains across EVERY session, as a
/// ring in insertion order: the oldest record (and the now-empty session, actor and
/// memory it was the last of) is dropped once the store passes it.
pub const MAX_STORED_INGESTIONS: usize = 10_000;

/// The most `clientToken` records this emulator retains for one memory container.
pub const MAX_IDEMPOTENCY_RECORDS_PER_MEMORY: usize = 1_000;

/// The most long-term memory records this emulator retains in one memory container.
pub const MAX_MEMORY_RECORDS_PER_MEMORY: usize = 1_000;

/// The most long-term memory records this emulator retains across EVERY memory
/// container, as a ring in insertion order: the oldest record (and the now-empty memory
/// container it was the last of) is dropped once the store passes it.
pub const MAX_STORED_MEMORY_RECORDS: usize = 10_000;

/// The most code interpreters this emulator retains sessions for; the least recently
/// used one is evicted once the store passes it.
pub const MAX_CODE_INTERPRETERS: usize = 32;

/// The most sessions this emulator retains for one code interpreter.
pub const MAX_CODE_INTERPRETER_SESSIONS: usize = 256;

/// The most `clientToken` records this emulator retains for one code interpreter.
pub const MAX_IDEMPOTENCY_RECORDS_PER_CODE_INTERPRETER: usize = 1_000;

/// The most files one code interpreter session's workspace holds.
///
/// An EMULATOR ceiling, not an AWS quota: the workspace lives in this process's memory
/// and a caller chooses every path it writes, so without a ceiling one session could
/// grow the heap without end. A write past it is refused, which is what a workspace
/// with no room left does — nothing already written is dropped.
pub const MAX_SESSION_FILES: usize = 1_024;

/// The most bytes one code interpreter session's workspace holds across every file in
/// it. The same EMULATOR ceiling as [`MAX_SESSION_FILES`], over size rather than count:
/// the modelled `content` member admits 100 MB per block, which no in-memory workspace
/// can take per request.
pub const MAX_SESSION_FILE_BYTES: usize = 8 * 1024 * 1024;

/// The most bytes EVERY code interpreter session in this emulator holds AT ONCE, across
/// every workspace and every execution context, [as accounted
/// here](CodeInterpreterSessionRecord::accounted_bytes).
///
/// The companion of [`MAX_SESSION_FILE_BYTES`] and [`MAX_CONTEXT_BYTES`], which bound ONE
/// session: without a whole-store ceiling the PRODUCT of those and the session counts
/// ([`MAX_CODE_INTERPRETERS`] times [`MAX_CODE_INTERPRETER_SESSIONS`]) is what a caller
/// looping on start-session and `writeFiles` could grow this process's heap to, which is
/// orders of magnitude past any host the emulator runs on. The memory plane is bounded the
/// same way, per level AND across the store ([`MAX_STORED_EVENTS`],
/// [`MAX_STORED_INGESTIONS`], [`MAX_STORED_MEMORY_RECORDS`]); this is the tools plane's
/// equivalent for the state that carries caller-chosen payloads.
///
/// A write that would pass it is refused exactly as a full workspace is
/// ([`WorkspaceFull::StoreBytes`]) — nothing already stored is dropped.
pub const MAX_STORED_SESSION_BYTES: usize = 64 * 1024 * 1024;

/// What ONE workspace entry is charged beside its content in the whole-store accounting
/// ([`MAX_STORED_SESSION_BYTES`]): the map entry holding it, on top of the key and the
/// bytes, both of which the caller chooses.
///
/// Without it a caller could store millions of EMPTY files for free, whose entries are
/// real heap even though their content is not, so the accounting charges every file a
/// fixed overhead as well as what it holds.
pub const SESSION_FILE_ENTRY_OVERHEAD_BYTES: usize = 256;

/// The most replayed-stop `clientToken` records one session keeps, so a caller looping
/// on `StopCodeInterpreterSession` with a fresh token cannot grow the record.
pub const MAX_STOP_TOKENS_PER_SESSION: usize = 256;

/// The longest workspace-relative path this emulator addresses a session file by, and
/// the most segments it admits. `MaxLenString` bounds a tool's `path` at 100 MB, which
/// is no path, so these are the EMULATOR's own bounds on the key side of a write.
pub const MAX_SESSION_PATH_BYTES: usize = 1_024;
/// See [`MAX_SESSION_PATH_BYTES`].
pub const MAX_SESSION_PATH_SEGMENTS: usize = 32;

/// The most scripts one session's execution context keeps, and the most bytes across
/// them. The EMULATOR's own ceilings, for the same reason the workspace has them: a
/// caller chooses both how many scripts it runs in a session and how long each is, and
/// the context is re-evaluated on each later run, so it bounds work as well as memory.
/// The oldest script gives way first.
pub const MAX_CONTEXT_SCRIPTS: usize = 16;
/// See [`MAX_CONTEXT_SCRIPTS`].
pub const MAX_CONTEXT_BYTES: usize = 256 * 1024;

/// The time-to-live a code interpreter session takes when the call that starts it names
/// none: `CodeInterpreterSessionTimeout`'s modelled `@default` (source: smithy_query
/// query="operation" operation="StartCodeInterpreterSession", `sessionTimeoutSeconds`
/// range 1..28800, default 900).
pub const DEFAULT_CODE_INTERPRETER_SESSION_TIMEOUT_SECONDS: i32 = 900;

/// The most workload access tokens this emulator holds issuances for at once.
///
/// An EMULATOR ceiling, not an AWS quota: every call that asks for a token appends one
/// record, so without a ceiling a caller looping on the identity plane would grow the
/// process heap without end. The issuance closest to its own expiry gives way first, so
/// the tokens a live workflow just obtained are the ones that stay resolvable.
pub const MAX_ISSUED_WORKLOAD_ACCESS_TOKENS: usize = 10_000;

/// How long a workload access token this emulator issues stays valid.
///
/// AWS publishes no lifetime for a workload access token, so this window is the
/// EMULATOR's own: short enough that a caller can observe one run out, long enough for a
/// workflow that obtains a token and then spends it. The window is what the token's own
/// `exp` claim carries and what [`IssuedWorkloadAccessTokens`] resolves it until.
pub const WORKLOAD_ACCESS_TOKEN_TTL: Duration = Duration::from_secs(900);

/// The region a request is treated as signed for when it carries no SigV4 credential
/// scope to read one from (an anonymous call); the emulator's default region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN service namespace, as every AgentCore ARN spells it.
pub const ARN_SERVICE: &str = "bedrock-agentcore";

/// `CodeInterpreterSessionStatus` for a session that is still inside its
/// time-to-live, so it can run code (source: smithy_query
/// query="shape" shape="CodeInterpreterSessionStatus", values READY | TERMINATED).
pub const CODE_INTERPRETER_SESSION_READY: &str = "READY";

/// `CodeInterpreterSessionStatus` for a session whose time-to-live has run out, from
/// the same enum.
pub const CODE_INTERPRETER_SESSION_TERMINATED: &str = "TERMINATED";

/// `MemoryRecordStatus` for a record a batch operation processed (source: smithy_query
/// query="shape" shape="MemoryRecordStatus", values SUCCEEDED | FAILED).
pub const MEMORY_RECORD_STATUS_SUCCEEDED: &str = "SUCCEEDED";

/// The message a memory-record read or delete is refused with when the record the
/// request addresses is not one this memory container holds: AWS's wording for that
/// `ResourceNotFoundException`, whose text names nothing from the request.
pub const MEMORY_RECORD_NOT_FOUND_MESSAGE: &str = "MemoryItem does not exist";

/// The `memoryStrategyId` reported for a record whose creating request named no
/// strategy.
///
/// `MemoryRecordCreateInput$memoryStrategyId` is optional while
/// `MemoryRecord$memoryStrategyId` is `@required`, so a record created with content of
/// the caller's own and no strategy association still has to be reported under one.
/// Memory strategies are owned by the CONTROL plane, a different service, so the only
/// grouping this data plane can name for such a record is the one AgentCore uses for
/// memory a caller fills itself.
pub const CALLER_MANAGED_MEMORY_STRATEGY_ID: &str = "CUSTOM";

/// Seconds in one day, the unit `recordExpiryDurationDays` states a record expiry in.
const SECONDS_PER_DAY: i64 = 24 * 60 * 60;

/// The instant a record created at `created_at` expires when the creating request asked
/// for an expiry of `days` days ("The duration, in days, after which the records in this
/// request expire"), as a duration from that instant.
pub fn record_expires_at(
    created_at: ::aws_smithy_types::DateTime,
    days: i32,
) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_secs(
        created_at
            .secs()
            .saturating_add(i64::from(days).saturating_mul(SECONDS_PER_DAY)),
    )
}

/// How this service words the `ConflictException` a call gets when the code
/// interpreter session it names is no longer running: `Session already terminated:
/// <sessionId>`, with the session taken from the request.
///
/// One sentence for both callers — the stop that cannot stop it again and the tool
/// invocation that cannot run in it — so the two cannot word the same condition
/// differently.
pub fn session_already_terminated(session_id: &str) -> String {
    format!("Session already terminated: {session_id}")
}

/// Per-service state for com-amazonaws-bedrockagentcore-amazonbedrockagentcore. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState {
    /// Memory containers this data plane has observed, keyed by the memory id as
    /// [`normalize_memory_id`] spells it. Bounded by [`MAX_MEMORIES`].
    pub memories: HashMap<String, MemoryStore>,
    /// Every stored event, in insertion order, as a ring bounded by
    /// [`MAX_STORED_EVENTS`]. An entry whose event is already gone (deleted, or
    /// evicted by a per-level bound) is simply skipped when it reaches the front.
    pub order: VecDeque<EventKey>,
    /// Every stored ingestion, in insertion order, as a ring bounded by
    /// [`MAX_STORED_INGESTIONS`], read the same way as [`Self::order`].
    pub ingest_order: VecDeque<IngestKey>,
    /// Every stored long-term memory record, in insertion order, as a ring bounded by
    /// [`MAX_STORED_MEMORY_RECORDS`], read the same way as [`Self::order`].
    pub record_order: VecDeque<MemoryRecordKey>,
    /// Monotonic counter feeding the hex suffix of every generated `eventId` and
    /// `sessionId`, so two records minted inside the same millisecond still get
    /// distinct ids, and ordering the records each bound evicts by.
    pub event_seq: u64,
    /// Code interpreters this data plane has started a session on, keyed by the
    /// `codeInterpreterIdentifier` the caller named. Bounded by
    /// [`MAX_CODE_INTERPRETERS`].
    ///
    /// An entry here is this plane's INDEX of the sessions started on an interpreter,
    /// not a claim that the interpreter exists: the interpreter itself is created
    /// through the CONTROL plane, a different service, and whether this account holds
    /// one is answered by the shared inventory ([`holds_code_interpreter`]) before any
    /// call that starts a session writes here.
    pub code_interpreters: HashMap<String, CodeInterpreterStore>,
    /// Browsers this data plane has started a session on, keyed by the
    /// `browserIdentifier` the caller named, for the same reason
    /// [`Self::code_interpreters`] is keyed by its own identifier: the browser itself is
    /// created through the CONTROL plane, a different service. Bounded by
    /// [`MAX_BROWSERS`].
    pub browsers: HashMap<String, BrowserStore>,
    /// Every workload access token this service issued and has not pruned.
    pub workload_access_tokens: IssuedWorkloadAccessTokens,
    /// The A/B tests `CreateABTest` created, keyed by the `abTestId` this emulator
    /// minted for each one. Bounded by [`MAX_AB_TESTS`]; see [`ab_tests`].
    pub ab_tests: BTreeMap<String, ABTestRecord>,
    /// The `abTestId` each `CreateABTest` `clientToken` already created, so a replayed
    /// token answers with that test instead of creating a second one. Bounded by
    /// [`MAX_AB_TEST_IDEMPOTENCY_RECORDS`].
    pub ab_test_idempotency: BTreeMap<String, String>,
    /// The batch evaluation jobs `StartBatchEvaluation` started, keyed by the
    /// `batchEvaluationId` this emulator minted for each one. Bounded by
    /// [`MAX_BATCH_EVALUATIONS`]; see [`batch_evaluations`].
    pub batch_evaluations: BTreeMap<String, BatchEvaluationRecord>,
    /// The `batchEvaluationId` each `StartBatchEvaluation` `clientToken` already
    /// started, so a replayed token answers with that job instead of starting a second
    /// one. Bounded by [`MAX_BATCH_EVALUATION_IDEMPOTENCY_RECORDS`].
    pub batch_evaluation_idempotency: BTreeMap<String, String>,
    /// The payment sessions `CreatePaymentSession` opened, keyed by the
    /// `paymentSessionId` this emulator minted for each one. Bounded by
    /// [`MAX_PAYMENT_SESSIONS`]; see [`payment_sessions`].
    pub payment_sessions: BTreeMap<String, PaymentSessionRecord>,
    /// The `paymentSessionId` each `CreatePaymentSession` `clientToken` already opened, so
    /// a replayed token answers with that session instead of opening a second one. Bounded
    /// by [`MAX_PAYMENT_SESSION_IDEMPOTENCY_RECORDS`].
    pub payment_session_idempotency: BTreeMap<String, String>,
}

impl ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState {
    /// The next value of the event sequence. Never returns the same value twice for
    /// one engine, which is what makes a generated `eventId` unique.
    pub fn next_seq(&mut self) -> u64 {
        self.event_seq += 1;
        self.event_seq
    }

    /// The stored event addressed by (memory, actor, session, event), or `None` when
    /// any step of that path is absent — [`Self::resolve_event`] without the message
    /// saying which step it was, for a caller that has no refusal to word (a replayed
    /// `clientToken` looking for what it already created).
    pub fn find_event(
        &self,
        memory_id: &str,
        actor_id: &str,
        session_id: &str,
        event_id: &str,
    ) -> Option<&StoredEvent> {
        self.resolve_event(memory_id, actor_id, session_id, event_id)
            .ok()
    }

    /// The stored session addressed by (memory, actor, session), or `None` when any step
    /// of that path is absent — [`Self::resolve_session`] read the same way.
    pub fn session(
        &self,
        memory_id: &str,
        actor_id: &str,
        session_id: &str,
    ) -> Option<&SessionStore> {
        self.resolve_session(memory_id, actor_id, session_id).ok()
    }

    /// This plane's INDEX of what has been written into the memory container `memory_id`
    /// names (either spelling [`normalize_memory_id`] accepts), or `None` when nothing has
    /// been written into it yet.
    ///
    /// NOT a claim about whether the CONTAINER exists: a container is created through the
    /// CONTROL plane, so whether the account holds one is answered by the shared inventory
    /// ([`memory_container_resolved`]), which every operation addressing something inside a
    /// container asks FIRST. An empty index therefore means the container holds nothing,
    /// not that it is absent.
    pub fn memory_index(&self, memory_id: &str) -> Option<&MemoryStore> {
        self.memories.get(&normalize_memory_id(memory_id))
    }

    /// The session a request addressed inside a container the caller has ALREADY resolved
    /// ([`memory_container_resolved`]), or the message naming the session as the missing
    /// resource.
    ///
    /// A session is identified by its actor as well as its `sessionId`, and an actor is
    /// not addressable on its own in this plane (it has no read of its own, and a listing
    /// for an actor with nothing stored is an empty page rather than a refusal), so an
    /// absent actor is an absent session — and so is a container nothing has been written
    /// into, whose index is this plane's record of its contents rather than its existence.
    pub fn resolve_session(
        &self,
        memory_id: &str,
        actor_id: &str,
        session_id: &str,
    ) -> Result<&SessionStore, String> {
        self.memory_index(memory_id)
            .and_then(|memory| memory.actors.get(actor_id))
            .and_then(|actor| actor.sessions.get(session_id))
            .ok_or_else(|| resource_not_found_message(NotFoundResource::Session, session_id))
    }

    /// The event a request addressed, resolved inside out the same way: the session inside
    /// the already-resolved container, then the event inside that session.
    pub fn resolve_event(
        &self,
        memory_id: &str,
        actor_id: &str,
        session_id: &str,
        event_id: &str,
    ) -> Result<&StoredEvent, String> {
        self.resolve_session(memory_id, actor_id, session_id)?
            .events
            .iter()
            .find(|event| event.event_id == event_id)
            .ok_or_else(|| resource_not_found_message(NotFoundResource::EventRead, event_id))
    }

    /// The session a WRITING operation addressed: the same walk as
    /// [`Self::resolve_session`], and the same message from the shared builder, taken
    /// mutably. Rust's borrow rules make this a second walk rather than a reuse of the
    /// read-only one; the wording and the order it reports are still the builder's.
    pub fn resolve_session_mut(
        &mut self,
        memory_id: &str,
        actor_id: &str,
        session_id: &str,
    ) -> Result<&mut SessionStore, String> {
        self.memories
            .get_mut(&normalize_memory_id(memory_id))
            .and_then(|memory| memory.actors.get_mut(actor_id))
            .and_then(|actor| actor.sessions.get_mut(session_id))
            .ok_or_else(|| resource_not_found_message(NotFoundResource::Session, session_id))
    }

    /// Remove the event a request addressed and hand it back, resolving the path exactly
    /// as [`Self::resolve_event`] does, so a delete refuses with the same resource a read
    /// of the same path refuses with.
    ///
    /// The bookkeeping `insert_event` wrote goes with the event: the `clientToken` record
    /// that points at it has nothing left to replay, and the whole-store ring drops its
    /// key rather than carrying a stale one (reaching the front later would prune the
    /// session this delete deliberately left in place).
    pub fn remove_event(
        &mut self,
        memory_id: &str,
        actor_id: &str,
        session_id: &str,
        event_id: &str,
    ) -> Result<StoredEvent, String> {
        // The session is reported by the resolver; the event by the step below, which is
        // also what decides that anything is removed at all.
        let session = self.resolve_session_mut(memory_id, actor_id, session_id)?;
        let at = session
            .events
            .iter()
            .position(|event| event.event_id == event_id);
        let removed = at
            .and_then(|at| session.events.remove(at))
            .ok_or_else(|| resource_not_found_message(NotFoundResource::EventDelete, event_id))?;
        // The session borrow ends here, so the container's own bookkeeping follows.
        if let Some(memory) = self.memories.get_mut(&normalize_memory_id(memory_id)) {
            memory
                .idempotency
                .retain(|_, located| located.event_id != removed.event_id);
        }
        self.order.retain(|held| held.event_id != removed.event_id);
        Ok(removed)
    }

    /// Persist one event — indexing its memory, actor and session on the way — and
    /// hold every bound in [the module's ceiling](self#the-store-has-an-explicit-ceiling).
    ///
    /// The memory container's entry is this plane's INDEX of what is stored inside it, not
    /// a claim that the container exists: the container is created through the CONTROL
    /// plane, and the caller of this method has already resolved it against the shared
    /// inventory ([`holds_memory`]).
    pub fn insert_event(&mut self, event: StoredEvent, client_token: Option<String>) {
        let key = EventKey {
            memory_id: event.memory_id.clone(),
            actor_id: event.actor_id.clone(),
            session_id: event.session_id.clone(),
            event_id: event.event_id.clone(),
        };
        let seq = event.seq;

        let memory = self.memories.entry(key.memory_id.clone()).or_default();
        memory.last_seq = seq;
        if let Some(token) = client_token {
            memory.idempotency.insert(
                token,
                EventLocator {
                    actor_id: key.actor_id.clone(),
                    session_id: key.session_id.clone(),
                    event_id: key.event_id.clone(),
                    seq,
                },
            );
            while memory.idempotency.len() > MAX_IDEMPOTENCY_RECORDS_PER_MEMORY {
                let Some(oldest) =
                    least_recently_written(&memory.idempotency, |locator| locator.seq)
                else {
                    break;
                };
                memory.idempotency.remove(&oldest);
            }
        }

        let actor = memory.actors.entry(key.actor_id.clone()).or_default();
        actor.last_seq = seq;
        let session = actor
            .sessions
            .entry(key.session_id.clone())
            .or_insert_with(|| SessionStore {
                session_id: key.session_id.clone(),
                actor_id: key.actor_id.clone(),
                // The session came into being with its first event, so that event's
                // `eventTimestamp` is the session's `createdAt`.
                created_at: event.event_timestamp,
                last_seq: seq,
                events: VecDeque::new(),
                ingestions: VecDeque::new(),
            });
        session.last_seq = seq;
        session.events.push_back(event);
        while session.events.len() > MAX_EVENTS_PER_SESSION {
            session.events.pop_front();
        }

        while actor.sessions.len() > MAX_SESSIONS_PER_ACTOR {
            let Some(oldest) = least_recently_written(&actor.sessions, |session| session.last_seq)
            else {
                break;
            };
            actor.sessions.remove(&oldest);
        }
        while memory.actors.len() > MAX_ACTORS_PER_MEMORY {
            let Some(oldest) = least_recently_written(&memory.actors, |actor| actor.last_seq) else {
                break;
            };
            memory.actors.remove(&oldest);
        }
        while self.memories.len() > MAX_MEMORIES {
            let Some(oldest) = least_recently_written(&self.memories, |memory| memory.last_seq)
            else {
                break;
            };
            self.memories.remove(&oldest);
        }

        self.order.push_back(key);
        while self.order.len() > MAX_STORED_EVENTS {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.evict_event(&oldest);
        }
    }

    /// Drop an evicted event and every container it was the last record of, so a
    /// container cannot outlive the events that created it.
    ///
    /// This is the EVICTION path, not `DeleteEvent`: a caller's delete leaves the
    /// session in place (AWS reports a session until its own cleanup removes an empty
    /// one), while an evicted event leaves nothing behind to report.
    fn evict_event(&mut self, key: &EventKey) {
        let Some(memory) = self.memories.get_mut(&key.memory_id) else {
            return;
        };
        if let Some(actor) = memory.actors.get_mut(&key.actor_id) {
            if let Some(session) = actor.sessions.get_mut(&key.session_id) {
                session.events.retain(|event| event.event_id != key.event_id);
                // A session that still holds ingested content is still a session this
                // memory has, so only an empty one is dropped.
                if session.events.is_empty() && session.ingestions.is_empty() {
                    actor.sessions.remove(&key.session_id);
                }
            }
            if actor.sessions.is_empty() {
                memory.actors.remove(&key.actor_id);
            }
        }
        memory
            .idempotency
            .retain(|_, locator| locator.event_id != key.event_id);
        self.prune_empty_memory(&key.memory_id);
    }

    /// The content a `clientToken` already ingested into this memory container, or
    /// `None` when that token ingested none (or the record it wrote is no longer held).
    pub fn ingestion_for_token(
        &self,
        memory_id: &str,
        client_token: &str,
    ) -> Option<&IngestedContent> {
        let memory = self.memories.get(memory_id)?;
        let locator = memory.ingest_idempotency.get(client_token)?;
        self.session(memory_id, &locator.actor_id, &locator.session_id)?
            .ingestions
            .iter()
            .find(|ingestion| ingestion.seq == locator.seq)
    }

    /// Persist one ingested-content record — registering its memory, actor and session
    /// on the way — and hold every bound in
    /// [the module's ceiling](self#the-store-has-an-explicit-ceiling).
    ///
    /// The session is registered here exactly as an event registers one: the content
    /// belongs to a session of an actor in the memory container, and a session that
    /// received only ingested content holds no events.
    pub fn insert_ingestion(&mut self, ingestion: IngestedContent, client_token: Option<String>) {
        let key = IngestKey {
            memory_id: ingestion.memory_id.clone(),
            actor_id: ingestion.actor_id.clone(),
            session_id: ingestion.session_id.clone(),
            seq: ingestion.seq,
        };
        let seq = ingestion.seq;

        let memory = self.memories.entry(key.memory_id.clone()).or_default();
        memory.last_seq = seq;
        if let Some(token) = client_token {
            memory.ingest_idempotency.insert(
                token,
                IngestLocator {
                    actor_id: key.actor_id.clone(),
                    session_id: key.session_id.clone(),
                    seq,
                },
            );
            while memory.ingest_idempotency.len() > MAX_IDEMPOTENCY_RECORDS_PER_MEMORY {
                let Some(oldest) =
                    least_recently_written(&memory.ingest_idempotency, |locator| locator.seq)
                else {
                    break;
                };
                memory.ingest_idempotency.remove(&oldest);
            }
        }

        let actor = memory.actors.entry(key.actor_id.clone()).or_default();
        actor.last_seq = seq;
        let session = actor
            .sessions
            .entry(key.session_id.clone())
            .or_insert_with(|| SessionStore {
                session_id: key.session_id.clone(),
                actor_id: key.actor_id.clone(),
                // The session came into being with this content, so the content's
                // `contentTimestamp` is the session's `createdAt`.
                created_at: ingestion.content_timestamp,
                last_seq: seq,
                events: VecDeque::new(),
                ingestions: VecDeque::new(),
            });
        session.last_seq = seq;
        session.ingestions.push_back(ingestion);
        while session.ingestions.len() > MAX_INGESTIONS_PER_SESSION {
            session.ingestions.pop_front();
        }

        while actor.sessions.len() > MAX_SESSIONS_PER_ACTOR {
            let Some(oldest) = least_recently_written(&actor.sessions, |session| session.last_seq)
            else {
                break;
            };
            actor.sessions.remove(&oldest);
        }
        while memory.actors.len() > MAX_ACTORS_PER_MEMORY {
            let Some(oldest) = least_recently_written(&memory.actors, |actor| actor.last_seq) else {
                break;
            };
            memory.actors.remove(&oldest);
        }
        while self.memories.len() > MAX_MEMORIES {
            let Some(oldest) = least_recently_written(&self.memories, |memory| memory.last_seq)
            else {
                break;
            };
            self.memories.remove(&oldest);
        }

        self.ingest_order.push_back(key);
        while self.ingest_order.len() > MAX_STORED_INGESTIONS {
            let Some(oldest) = self.ingest_order.pop_front() else {
                break;
            };
            self.evict_ingestion(&oldest);
        }
    }

    /// Drop an evicted ingestion and every container it was the last record of, so a
    /// container cannot outlive the records that created it.
    fn evict_ingestion(&mut self, key: &IngestKey) {
        let Some(memory) = self.memories.get_mut(&key.memory_id) else {
            return;
        };
        if let Some(actor) = memory.actors.get_mut(&key.actor_id) {
            if let Some(session) = actor.sessions.get_mut(&key.session_id) {
                session
                    .ingestions
                    .retain(|ingestion| ingestion.seq != key.seq);
                if session.events.is_empty() && session.ingestions.is_empty() {
                    actor.sessions.remove(&key.session_id);
                }
            }
            if actor.sessions.is_empty() {
                memory.actors.remove(&key.actor_id);
            }
        }
        memory
            .ingest_idempotency
            .retain(|_, locator| locator.seq != key.seq);
        self.prune_empty_memory(&key.memory_id);
    }

    /// The long-term memory record addressed by (memory, record), or `None` when this
    /// emulator holds no such record.
    pub fn memory_record(
        &self,
        memory_id: &str,
        memory_record_id: &str,
    ) -> Option<&StoredMemoryRecord> {
        self.memories.get(memory_id)?.records.get(memory_record_id)
    }

    /// The records a `clientToken` already created in this memory container, in the
    /// order the request that minted them listed them — or `None` when that token
    /// created none (or one of the records it created is no longer held, so the batch
    /// can no longer be answered from what it wrote).
    pub fn memory_records_for_token(
        &self,
        memory_id: &str,
        client_token: &str,
    ) -> Option<Vec<&StoredMemoryRecord>> {
        let memory = self.memories.get(memory_id)?;
        memory
            .record_idempotency
            .get(client_token)?
            .memory_record_ids
            .iter()
            .map(|id| memory.records.get(id))
            .collect()
    }

    /// Persist the records one `BatchCreateMemoryRecords` created — registering the
    /// memory container on the way — and hold every bound in
    /// [the module's ceiling](self#the-store-has-an-explicit-ceiling).
    ///
    /// The memory container is registered here for the same reason an event registers
    /// one: the container itself is created through the CONTROL plane, a different
    /// service.
    pub fn insert_memory_records(
        &mut self,
        memory_id: &str,
        records: Vec<StoredMemoryRecord>,
        client_token: Option<String>,
    ) {
        let minted: Vec<String> = records
            .iter()
            .map(|record| record.memory_record_id.clone())
            .collect();
        let last_seq = records.iter().map(|record| record.seq).max().unwrap_or(0);

        let memory = self.memories.entry(memory_id.to_string()).or_default();
        memory.last_seq = memory.last_seq.max(last_seq);
        for record in records {
            memory
                .records
                .insert(record.memory_record_id.clone(), record);
        }
        if let Some(token) = client_token {
            memory.record_idempotency.insert(
                token,
                MemoryRecordBatchLocator {
                    memory_record_ids: minted.clone(),
                    seq: last_seq,
                },
            );
            while memory.record_idempotency.len() > MAX_IDEMPOTENCY_RECORDS_PER_MEMORY {
                let Some(oldest) =
                    least_recently_written(&memory.record_idempotency, |locator| locator.seq)
                else {
                    break;
                };
                memory.record_idempotency.remove(&oldest);
            }
        }

        for memory_record_id in minted {
            self.record_order.push_back(MemoryRecordKey {
                memory_id: memory_id.to_string(),
                memory_record_id,
            });
        }

        // Both record bounds are held through the one removal path, so an evicted
        // record never leaves its ring key or its `clientToken` record behind.
        while let Some(oldest) = self
            .memories
            .get(memory_id)
            .filter(|memory| memory.records.len() > MAX_MEMORY_RECORDS_PER_MEMORY)
            .and_then(|memory| least_recently_written(&memory.records, |record| record.seq))
        {
            self.remove_memory_record(memory_id, &oldest);
        }
        while self.record_order.len() > MAX_STORED_MEMORY_RECORDS {
            let Some(oldest) = self.record_order.pop_front() else {
                break;
            };
            self.remove_memory_record(&oldest.memory_id, &oldest.memory_record_id);
        }
        while self.memories.len() > MAX_MEMORIES {
            let Some(oldest) = least_recently_written(&self.memories, |memory| memory.last_seq)
            else {
                break;
            };
            self.memories.remove(&oldest);
        }
    }

    /// Remove one long-term memory record and everything that pointed at it, answering
    /// with the record that was removed (`None` when this emulator held no such record).
    ///
    /// This is `DeleteMemoryRecord`'s path and the eviction path both: in either case the
    /// record is gone, so the whole-store ring drops its key rather than carrying a stale
    /// one, and the `clientToken` record of the batch that created it goes too — that
    /// batch can no longer be answered from what it wrote.
    pub fn remove_memory_record(
        &mut self,
        memory_id: &str,
        memory_record_id: &str,
    ) -> Option<StoredMemoryRecord> {
        let memory = self.memories.get_mut(memory_id)?;
        let removed = memory.records.remove(memory_record_id)?;
        memory.record_idempotency.retain(|_, locator| {
            !locator
                .memory_record_ids
                .iter()
                .any(|id| id == memory_record_id)
        });
        self.record_order
            .retain(|key| key.memory_id != memory_id || key.memory_record_id != memory_record_id);
        self.prune_empty_memory(memory_id);
        Some(removed)
    }

    /// Drop every long-term memory record whose expiry the clock has already passed.
    ///
    /// A record expiry is a time-to-live: the record is gone at the instant it names,
    /// whether or not anybody calls, so materialising it is what any operation that
    /// reaches the records does first rather than reporting a record AWS would not.
    pub fn expire_memory_records(&mut self, now: SystemTime) {
        let expired: Vec<MemoryRecordKey> = self
            .memories
            .iter()
            .flat_map(|(memory_id, memory)| {
                memory
                    .records
                    .values()
                    .filter(|record| record.has_expired(now))
                    .map(|record| MemoryRecordKey {
                        memory_id: memory_id.clone(),
                        memory_record_id: record.memory_record_id.clone(),
                    })
            })
            .collect();
        for key in expired {
            self.remove_memory_record(&key.memory_id, &key.memory_record_id);
        }
    }

    /// Drop a memory container this data plane holds nothing for any more, so a
    /// container cannot outlive the records that registered it. A container that still
    /// holds actors OR long-term memory records is still a container this memory has.
    fn prune_empty_memory(&mut self, memory_id: &str) {
        if self
            .memories
            .get(memory_id)
            .is_some_and(|memory| memory.actors.is_empty() && memory.records.is_empty())
        {
            self.memories.remove(memory_id);
        }
    }

    /// The stored code interpreter session addressed by (code interpreter, session),
    /// or `None` when this emulator holds no such session.
    pub fn code_interpreter_session(
        &self,
        code_interpreter_identifier: &str,
        session_id: &str,
    ) -> Option<&CodeInterpreterSessionRecord> {
        self.code_interpreters
            .get(code_interpreter_identifier)?
            .sessions
            .get(session_id)
    }

    /// The stored code interpreter session addressed by (code interpreter, session),
    /// for a caller that is about to CHANGE it — a stop, or a tool that writes the
    /// session's workspace.
    pub fn code_interpreter_session_mut(
        &mut self,
        code_interpreter_identifier: &str,
        session_id: &str,
    ) -> Option<&mut CodeInterpreterSessionRecord> {
        self.code_interpreters
            .get_mut(code_interpreter_identifier)?
            .sessions
            .get_mut(session_id)
    }

    /// The session a `clientToken` already started on this code interpreter, or `None`
    /// when that token started none (or the session it started is no longer held).
    pub fn code_interpreter_session_for_token(
        &self,
        code_interpreter_identifier: &str,
        client_token: &str,
    ) -> Option<&CodeInterpreterSessionRecord> {
        let interpreter = self.code_interpreters.get(code_interpreter_identifier)?;
        let session_id = &interpreter.idempotency.get(client_token)?.session_id;
        interpreter.sessions.get(session_id)
    }

    /// What every code interpreter session OTHER than `(code_interpreter_identifier,
    /// session_id)` holds, [as the whole-store accounting charges
    /// it](CodeInterpreterSessionRecord::accounted_bytes).
    ///
    /// Read under the same guard as the write it bounds, and handed to the session's own
    /// write path, so a write holds [`MAX_STORED_SESSION_BYTES`] across the store as well
    /// as this session's own ceilings. The walk is over the sessions the store holds,
    /// which [`MAX_CODE_INTERPRETERS`], [`MAX_CODE_INTERPRETER_SESSIONS`] and
    /// [`MAX_STORED_SESSION_BYTES`] together bound.
    pub fn session_bytes_elsewhere(
        &self,
        code_interpreter_identifier: &str,
        session_id: &str,
    ) -> usize {
        self.code_interpreters
            .iter()
            .flat_map(|(identifier, interpreter)| {
                interpreter
                    .sessions
                    .iter()
                    .map(move |(held, session)| (identifier, held, session))
            })
            .filter(|(identifier, held, _)| {
                identifier.as_str() != code_interpreter_identifier || held.as_str() != session_id
            })
            .map(|(_, _, session)| session.accounted_bytes())
            .fold(0usize, usize::saturating_add)
    }

    /// Persist one code interpreter session — registering its code interpreter on the
    /// way — and hold every bound in
    /// [the module's ceiling](self#the-store-has-an-explicit-ceiling).
    pub fn insert_code_interpreter_session(
        &mut self,
        session: CodeInterpreterSessionRecord,
        client_token: Option<String>,
    ) {
        let seq = session.seq;
        let session_id = session.session_id.clone();
        let interpreter = self
            .code_interpreters
            .entry(session.code_interpreter_identifier.clone())
            .or_default();
        interpreter.last_seq = seq;
        interpreter.sessions.insert(session_id.clone(), session);
        if let Some(token) = client_token {
            interpreter
                .idempotency
                .insert(token, SessionLocator { session_id, seq });
            while interpreter.idempotency.len() > MAX_IDEMPOTENCY_RECORDS_PER_CODE_INTERPRETER {
                let Some(oldest) =
                    least_recently_written(&interpreter.idempotency, |locator| locator.seq)
                else {
                    break;
                };
                interpreter.idempotency.remove(&oldest);
            }
        }

        while interpreter.sessions.len() > MAX_CODE_INTERPRETER_SESSIONS {
            let Some(oldest) = least_recently_written(&interpreter.sessions, |session| session.seq)
            else {
                break;
            };
            interpreter.sessions.remove(&oldest);
            interpreter
                .idempotency
                .retain(|_, locator| locator.session_id != oldest);
        }
        while self.code_interpreters.len() > MAX_CODE_INTERPRETERS {
            let Some(oldest) =
                least_recently_written(&self.code_interpreters, |interpreter| interpreter.last_seq)
            else {
                break;
            };
            self.code_interpreters.remove(&oldest);
        }
    }
}

/// What one workload access token this service issued stands for: the workload identity
/// it was issued from, the user it acts on behalf of when the issuing call named one, and
/// the window the issuance is valid in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedWorkloadAccessToken {
    /// The `workloadName` the issuing call named.
    pub workload_name: String,
    /// The user the token acts on behalf of, as the issuing call named it. `None` for a
    /// token issued for the workload itself rather than for a user.
    pub user_id: Option<String>,
    /// When the token was issued, which is its own `iat`.
    pub issued_at: SystemTime,
    /// When the issuance stops being valid, which is its own `exp`.
    pub expires_at: SystemTime,
}

/// Every workload access token this service issued and has not pruned, keyed by the
/// token itself.
#[derive(Default)]
pub struct IssuedWorkloadAccessTokens {
    issued: HashMap<String, IssuedWorkloadAccessToken>,
}

/// Reports how many issuances are held and nothing else. The KEY of this registry is the
/// token, which is a bearer of the workload identity it names — so a derived rendering
/// would put every outstanding token into whatever formatted it, a log line included. The
/// issuances stay readable through [`IssuedWorkloadAccessTokens::issued`] by a caller that
/// already holds the token.
impl ::std::fmt::Debug for IssuedWorkloadAccessTokens {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("IssuedWorkloadAccessTokens")
            .field("issued", &self.issued.len())
            .finish_non_exhaustive()
    }
}

impl IssuedWorkloadAccessTokens {
    /// Record that this service issued `token` for `issued`.
    ///
    /// Prunes first: a record the clock has already expired is dropped, which is owed at
    /// this instant whether or not anybody asked for a token, and at the ceiling the
    /// record expiring soonest gives way, with the key breaking a tie so eviction is
    /// deterministic even among issuances that share a whole-second expiry.
    pub fn record(&mut self, token: String, issued: IssuedWorkloadAccessToken, now: SystemTime) {
        self.prune(now);
        while self.issued.len() >= MAX_ISSUED_WORKLOAD_ACCESS_TOKENS {
            let soonest = self
                .issued
                .iter()
                .min_by(|left, right| {
                    (left.1.expires_at, left.0.as_str())
                        .cmp(&(right.1.expires_at, right.0.as_str()))
                })
                .map(|(key, _)| key.clone());
            match soonest {
                Some(key) => {
                    self.issued.remove(&key);
                }
                None => break,
            }
        }
        self.issued.insert(token, issued);
    }

    /// What `token` was issued for, when this service issued it and the issuance has not
    /// expired at `now`. `None` for a token this service did not issue, and for one the
    /// clock has run out on.
    ///
    /// A READ: nothing is dropped here, so two callers asking about one token are
    /// answered alike.
    #[must_use]
    pub fn issued(&self, token: &str, now: SystemTime) -> Option<&IssuedWorkloadAccessToken> {
        self.issued.get(token).filter(|held| now < held.expires_at)
    }

    /// How many issuances the registry holds, expired records included until the next
    /// write prunes them.
    #[must_use]
    pub fn len(&self) -> usize {
        self.issued.len()
    }

    /// Whether the registry holds no issuance at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.issued.is_empty()
    }

    /// Drop what the clock has already made unresolvable.
    fn prune(&mut self, now: SystemTime) {
        self.issued.retain(|_, held| now < held.expires_at);
    }
}

/// The least recently written entry of `map`, by the sequence `seq_of` reads off a
/// record, with the key breaking a tie so eviction is deterministic.
fn least_recently_written<T>(
    map: &HashMap<String, T>,
    seq_of: impl Fn(&T) -> u64,
) -> Option<String> {
    map.iter()
        .min_by(|left, right| {
            (seq_of(left.1), left.0.as_str()).cmp(&(seq_of(right.1), right.0.as_str()))
        })
        .map(|(key, _)| key.clone())
}

/// Where one stored event lives, as the whole-store ring records it.
#[derive(Debug, Clone)]
pub struct EventKey {
    pub memory_id: String,
    pub actor_id: String,
    pub session_id: String,
    pub event_id: String,
}

/// Where one stored ingestion lives, as the whole-store ring records it. The
/// sequence identifies the record inside its session: ingested content carries no
/// modelled id of its own (`IngestData` answers with the `sessionId` only).
#[derive(Debug, Clone)]
pub struct IngestKey {
    pub memory_id: String,
    pub actor_id: String,
    pub session_id: String,
    pub seq: u64,
}

/// Where one stored long-term memory record lives, as the whole-store ring records it.
/// A memory record belongs to its memory container directly, so the key names no actor
/// and no session.
#[derive(Debug, Clone)]
pub struct MemoryRecordKey {
    pub memory_id: String,
    pub memory_record_id: String,
}

/// One AgentCore Memory container's data-plane contents.
#[derive(Debug, Default)]
pub struct MemoryStore {
    /// Actors that have produced at least one event in this memory. Bounded by
    /// [`MAX_ACTORS_PER_MEMORY`].
    pub actors: HashMap<String, ActorStore>,
    /// `clientToken` -> the event that token already created. `CreateEvent` is
    /// `@idempotent`, so a replayed token returns the first event rather than
    /// creating a second one. Bounded by [`MAX_IDEMPOTENCY_RECORDS_PER_MEMORY`], and
    /// a record is dropped with the event it points at.
    pub idempotency: HashMap<String, EventLocator>,
    /// `clientToken` -> the content that token already ingested. `IngestData` is
    /// `@idempotent` and its `clientToken` is an `@idempotencyToken`, so a replayed
    /// token answers with the session the first call used rather than ingesting the
    /// content twice. Kept apart from [`Self::idempotency`] because an ingestion and
    /// an event are different records. Bounded by
    /// [`MAX_IDEMPOTENCY_RECORDS_PER_MEMORY`], and a record is dropped with the
    /// ingestion it points at.
    pub ingest_idempotency: HashMap<String, IngestLocator>,
    /// The long-term memory records this container holds, keyed by the
    /// `memoryRecordId` this emulator minted for each one. Bounded by
    /// [`MAX_MEMORY_RECORDS_PER_MEMORY`].
    pub records: HashMap<String, StoredMemoryRecord>,
    /// `clientToken` -> the records that token already created.
    /// `BatchCreateMemoryRecords` is `@idempotent` and its `clientToken` is an
    /// `@idempotencyToken`, so a replayed token answers with the records the first call
    /// created rather than creating a second set. Bounded by
    /// [`MAX_IDEMPOTENCY_RECORDS_PER_MEMORY`], and a record is dropped with any of the
    /// memory records it points at.
    pub record_idempotency: HashMap<String, MemoryRecordBatchLocator>,
    /// The sequence of the last event written into this container, which is what its
    /// eviction order is read from.
    pub last_seq: u64,
}

/// Which records one `clientToken` already created, so a replayed batch can answer with
/// them.
#[derive(Debug, Clone)]
pub struct MemoryRecordBatchLocator {
    /// The ids the batch minted, in the order its request listed the records.
    pub memory_record_ids: Vec<String>,
    /// The highest sequence the batch wrote: its eviction order.
    pub seq: u64,
}

/// A stored long-term memory record: exactly what `BatchCreateMemoryRecords` persisted,
/// which is what `GetMemoryRecord` hands back.
#[derive(Debug, Clone)]
pub struct StoredMemoryRecord {
    /// `mem-<hex>`, minted by [`mint_memory_record_id`]; never client-supplied.
    pub memory_record_id: String,
    pub memory_id: String,
    /// The `requestIdentifier` the creating batch entry carried, which is what the batch
    /// response tracks each per-record outcome by.
    pub request_identifier: String,
    pub namespaces: Vec<String>,
    pub content: MemoryContent,
    /// The strategy this record is grouped under: the one the creating request named, or
    /// [`CALLER_MANAGED_MEMORY_STRATEGY_ID`] when it named none.
    pub memory_strategy_id: String,
    /// The instant the remembered content occurred, as the creating request stated it.
    pub timestamp: ::aws_smithy_types::DateTime,
    /// The instant this record was created, which is when the batch that wrote it ran.
    pub created_at: ::aws_smithy_types::DateTime,
    /// When a record expiry applies, the instant this record expires; `None` for a
    /// record created with no expiry, which does not expire.
    pub expires_at: Option<::aws_smithy_types::DateTime>,
    pub metadata: Option<HashMap<String, MemoryRecordMetadataValue>>,
    /// The value [`ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState::next_seq`]
    /// handed this record: its eviction order.
    pub seq: u64,
}

impl StoredMemoryRecord {
    /// This record as the modelled `MemoryRecord`, the shape `GetMemoryRecord` reports.
    pub fn to_memory_record(&self) -> MemoryRecord {
        MemoryRecord {
            memory_record_id: self.memory_record_id.clone(),
            content: self.content.clone(),
            memory_strategy_id: self.memory_strategy_id.clone(),
            namespaces: self.namespaces.clone(),
            created_at: self.created_at,
            timestamp: Some(self.timestamp),
            metadata: self.metadata.clone(),
            expires_at: self.expires_at,
        }
    }

    /// This record as the per-record outcome a batch operation reports for a record it
    /// processed, tracked by the `requestIdentifier` its input carried.
    pub fn to_output(&self) -> MemoryRecordOutput {
        MemoryRecordOutput {
            memory_record_id: self.memory_record_id.clone(),
            status: MEMORY_RECORD_STATUS_SUCCEEDED.to_string(),
            request_identifier: Some(self.request_identifier.clone()),
            error_code: None,
            error_message: None,
        }
    }

    /// Whether the record expiry this record carries has already run out at `now`.
    pub fn has_expired(&self, now: SystemTime) -> bool {
        let Some(expires_at) = self.expires_at else {
            return false;
        };
        unix_seconds(now) >= expires_at.secs()
    }
}

/// Where an already-created event lives, so a replayed `clientToken` can find it.
#[derive(Debug, Clone)]
pub struct EventLocator {
    pub actor_id: String,
    pub session_id: String,
    pub event_id: String,
    /// The sequence of the event this record points at: its eviction order.
    pub seq: u64,
}

/// Where an already-ingested record lives, so a replayed `clientToken` can find it.
#[derive(Debug, Clone)]
pub struct IngestLocator {
    pub actor_id: String,
    pub session_id: String,
    /// The sequence of the ingestion this record points at: its identity inside the
    /// session, and its eviction order.
    pub seq: u64,
}

/// One actor's sessions inside a memory container.
#[derive(Debug, Default)]
pub struct ActorStore {
    /// Bounded by [`MAX_SESSIONS_PER_ACTOR`].
    pub sessions: HashMap<String, SessionStore>,
    /// The sequence of the last event written for this actor: its eviction order.
    pub last_seq: u64,
}

/// One session: its identity, when it came into being, and its events in creation
/// order (`ListEvents` reports them newest-first, which is a view over this order,
/// not the order itself).
#[derive(Debug)]
pub struct SessionStore {
    pub session_id: String,
    pub actor_id: String,
    /// The `eventTimestamp` of the first event created in this session, which is the
    /// instant the session came into existence.
    pub created_at: ::aws_smithy_types::DateTime,
    /// The sequence of the last event written into this session: its eviction order.
    pub last_seq: u64,
    /// Creation-ordered, and bounded by [`MAX_EVENTS_PER_SESSION`]: the oldest event
    /// leaves the front once the session passes that bound.
    pub events: VecDeque<StoredEvent>,
    /// The content `IngestData` submitted into this session for long-term extraction,
    /// in ingestion order and bounded by [`MAX_INGESTIONS_PER_SESSION`]. Ingested
    /// content is not a short-term event, so it is never reported by `ListEvents` or
    /// `GetEvent`.
    pub ingestions: VecDeque<IngestedContent>,
}

impl SessionStore {
    /// This session as a `SessionSummary`, the shape `ListSessions` reports.
    pub fn summary(&self) -> SessionSummary {
        SessionSummary {
            session_id: self.session_id.clone(),
            actor_id: self.actor_id.clone(),
            created_at: self.created_at,
        }
    }
}

/// A stored ingestion: exactly what `IngestData` persisted for one call, the content
/// long-term extraction reads.
#[derive(Debug, Clone)]
pub struct IngestedContent {
    /// The id this service minted for the stored content, in the `EventId` form
    /// [`mint_event_id`] spells: an extraction job names each message of this content by
    /// it, which is the only identity `MessageMetadata` carries for a message.
    pub content_id: String,
    pub memory_id: String,
    pub actor_id: String,
    pub session_id: String,
    /// The instant the content occurred, as the request stated it.
    pub content_timestamp: ::aws_smithy_types::DateTime,
    /// The inline payload items the request submitted, in the order it sent them.
    pub payload: Vec<IngestPayloadType>,
    /// The namespace substitution the request asked extraction to use, kept as sent.
    pub extraction_config: Option<ExtractionConfig>,
    pub metadata: Option<HashMap<String, MetadataValue>>,
    /// The value [`ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState::next_seq`]
    /// handed this ingestion: its identity inside the session, and its eviction order.
    pub seq: u64,
}

/// A stored event: exactly what `CreateEvent` persisted, which is what `GetEvent`
/// and `ListEvents` hand back.
#[derive(Debug, Clone)]
pub struct StoredEvent {
    /// `<epoch-millis>#<hex>`, minted by [`mint_event_id`]; never client-supplied.
    pub event_id: String,
    pub memory_id: String,
    pub actor_id: String,
    pub session_id: String,
    pub event_timestamp: ::aws_smithy_types::DateTime,
    pub payload: Vec<PayloadType>,
    pub branch: Option<Branch>,
    pub metadata: Option<HashMap<String, MetadataValue>>,
    /// The `extractionMode` the creating request carried, kept as sent. `SKIP` excludes
    /// the event from long-term memory extraction, so its messages compose no unit of
    /// extraction work ([`EXTRACTION_MODE_SKIP`]); the member absent means the event "is
    /// processed for extraction as usual".
    pub extraction_mode: Option<String>,
    /// The value [`ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState::next_seq`]
    /// handed this event. It totally orders events created in the same millisecond
    /// and with the same `eventTimestamp`, so a listing is deterministic.
    pub seq: u64,
}

impl StoredEvent {
    /// This event as the modelled `Event`.
    ///
    /// `include_payload` is `ListEvents`' `includePayloads`: `Event$payload` is
    /// `@required`, so a listing that drops the payloads reports each event with an
    /// empty payload list — the closest the modelled shape can come to leaving it out.
    pub fn to_event(&self, include_payload: bool) -> Event {
        Event {
            memory_id: self.memory_id.clone(),
            actor_id: self.actor_id.clone(),
            session_id: self.session_id.clone(),
            event_id: self.event_id.clone(),
            event_timestamp: self.event_timestamp,
            payload: if include_payload {
                self.payload.clone()
            } else {
                Vec::new()
            },
            branch: self.branch.clone(),
            metadata: self.metadata.clone(),
        }
    }

    /// The total order key used to report events newest-first: by the modelled
    /// `eventTimestamp`, then by creation order for events sharing an instant.
    pub fn order_key(&self) -> (i64, u32, u64) {
        (
            self.event_timestamp.secs(),
            self.event_timestamp.subsec_nanos(),
            self.seq,
        )
    }

    /// The name of the branch this event is on, or `None` for the trunk (an event
    /// created without a `branch`).
    pub fn branch_name(&self) -> Option<&str> {
        self.branch.as_ref().map(|branch| branch.name.as_str())
    }
}

/// One code interpreter's sessions, as `StartCodeInterpreterSession` wrote them.
#[derive(Debug, Default)]
pub struct CodeInterpreterStore {
    /// Sessions keyed by their minted `sessionId`. Bounded by
    /// [`MAX_CODE_INTERPRETER_SESSIONS`].
    pub sessions: HashMap<String, CodeInterpreterSessionRecord>,
    /// `clientToken` -> the session that token already started.
    /// `StartCodeInterpreterSession` is `@idempotent` and its `clientToken` is an
    /// `@idempotencyToken`, so a replayed token answers with the first session rather
    /// than starting a second one. Bounded by
    /// [`MAX_IDEMPOTENCY_RECORDS_PER_CODE_INTERPRETER`], and a record is dropped with
    /// the session it points at.
    pub idempotency: HashMap<String, SessionLocator>,
    /// The sequence of the last session started on this code interpreter, which is
    /// what its eviction order is read from.
    pub last_seq: u64,
}

/// Where an already-started session lives, so a replayed `clientToken` can find it.
#[derive(Debug, Clone)]
pub struct SessionLocator {
    pub session_id: String,
    /// The sequence of the session this record points at: its eviction order.
    pub seq: u64,
}

/// One code interpreter session: what `StartCodeInterpreterSession` persisted, which
/// is what `GetCodeInterpreterSession` reports, plus what the calls that act ON the
/// session write — the stop that terminated it and the workspace its tools use.
#[derive(Debug, Clone)]
pub struct CodeInterpreterSessionRecord {
    /// Minted by [`mint_code_interpreter_session_id`]; never client-supplied.
    pub session_id: String,
    pub code_interpreter_identifier: String,
    pub name: Option<String>,
    pub created_at: SystemTime,
    /// The session's time-to-live in seconds, as the request asked for it (the model
    /// defaults it to 900 when a request omits it).
    pub session_timeout_seconds: i32,
    pub certificates: Option<Vec<Certificate>>,
    pub filesystem_configurations: Option<Vec<ToolsFileSystemConfiguration>>,
    /// When this session was last changed by a call: its creation instant until a call
    /// changes it, and the instant of the stop that terminated it afterwards. This is
    /// what `StopCodeInterpreterSession` reports as `lastUpdatedAt`.
    pub last_updated_at: SystemTime,
    /// When `StopCodeInterpreterSession` terminated this session, or `None` while no
    /// stop has. A stopped session is `TERMINATED` from that instant on, whatever is
    /// left of its time-to-live.
    pub stopped_at: Option<SystemTime>,
    /// The `clientToken` of each stop this session has already served, with the
    /// `lastUpdatedAt` that stop answered: `StopCodeInterpreterSession` is
    /// `@idempotent` and its `clientToken` an `@idempotencyToken`, so a replayed token
    /// answers with the stop it already performed rather than conflicting with it.
    /// Bounded by [`MAX_STOP_TOKENS_PER_SESSION`].
    pub stop_tokens: BTreeMap<String, SystemTime>,
    /// This session's workspace: the files its tools have written, keyed by the
    /// workspace-relative path [`normalize_session_path`] spells. Bounded by
    /// [`MAX_SESSION_FILES`] and [`MAX_SESSION_FILE_BYTES`].
    pub files: BTreeMap<String, SessionFile>,
    /// The scripts this session's `executeCode` tool has run to completion, oldest
    /// first: the session's execution CONTEXT, which the next run is evaluated on top
    /// of unless that run asked for the context to be cleared. A script that ended on
    /// an exception is not kept, and neither is one whose run hit a bound. Bounded by
    /// [`MAX_CONTEXT_SCRIPTS`] and [`MAX_CONTEXT_BYTES`], oldest first.
    pub context_scripts: Vec<String>,
    /// The value [`ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState::next_seq`]
    /// handed this session: its eviction order, and what keeps two sessions started in
    /// the same millisecond apart.
    pub seq: u64,
}

/// One file in a session's workspace: the bytes a `writeFiles` tool put there and when
/// it did. The bytes are held in this process's memory only — no host path is opened
/// for them, and no other session reaches them.
#[derive(Debug, Clone)]
pub struct SessionFile {
    pub bytes: Vec<u8>,
    pub written_at: SystemTime,
}

/// Why a workspace write was refused: the ceiling it would have passed, and that
/// ceiling's value, so a caller is told which limit it met.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceFull {
    /// [`MAX_SESSION_FILES`] would have been passed.
    Files(usize),
    /// [`MAX_SESSION_FILE_BYTES`] would have been passed.
    Bytes(usize),
    /// [`MAX_STORED_SESSION_BYTES`] would have been passed: this session has room, but
    /// the sessions of this emulator together do not.
    StoreBytes(usize),
}

/// Why a tool's `path` addresses no file in a workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathRejection {
    /// The path names nothing once its separators are removed.
    Empty,
    /// The path is absolute: a workspace path is relative to the workspace root.
    Absolute,
    /// The path walks out of the workspace with a `..` segment.
    Escapes,
    /// The path is longer than [`MAX_SESSION_PATH_BYTES`] or has more than
    /// [`MAX_SESSION_PATH_SEGMENTS`] segments.
    TooLong,
}

/// The workspace key a tool's `path` addresses, or why it addresses none.
///
/// A workspace path is relative to the session's workspace root and is a KEY in
/// [`CodeInterpreterSessionRecord::files`], never a host path: `.` segments are
/// dropped, repeated separators collapse, and a `..` segment is refused rather than
/// resolved, so no tool argument can name anything outside the session's own state.
/// Both bounds are checked before anything is stored.
pub fn normalize_session_path(path: &str) -> Result<String, PathRejection> {
    if path.len() > MAX_SESSION_PATH_BYTES {
        return Err(PathRejection::TooLong);
    }
    if path.starts_with('/') {
        return Err(PathRejection::Absolute);
    }
    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => continue,
            ".." => return Err(PathRejection::Escapes),
            named => segments.push(named),
        }
    }
    if segments.is_empty() {
        return Err(PathRejection::Empty);
    }
    if segments.len() > MAX_SESSION_PATH_SEGMENTS {
        return Err(PathRejection::TooLong);
    }
    Ok(segments.join("/"))
}

/// The workspace key a tool's `directoryPath` addresses: the same normalization as
/// [`normalize_session_path`], except that the empty path is the workspace ROOT (which
/// is how `listFiles` addresses the whole workspace) rather than a rejection.
pub fn normalize_session_directory(path: &str) -> Result<String, PathRejection> {
    match normalize_session_path(path) {
        Ok(key) => Ok(key),
        Err(PathRejection::Empty) => Ok(String::new()),
        Err(other) => Err(other),
    }
}

impl CodeInterpreterSessionRecord {
    /// A session as the call that starts it leaves it: the configuration that call
    /// asked for, created at `created_at`, with an empty workspace and an empty
    /// execution context.
    ///
    /// Both callers go through this: `StartCodeInterpreterSession`, and the tool
    /// invocation that names no session and therefore starts one.
    pub fn started(
        seq: u64,
        code_interpreter_identifier: String,
        name: Option<String>,
        session_timeout_seconds: i32,
        certificates: Option<Vec<Certificate>>,
        filesystem_configurations: Option<Vec<ToolsFileSystemConfiguration>>,
        created_at: SystemTime,
    ) -> Self {
        Self {
            session_id: mint_code_interpreter_session_id(seq),
            code_interpreter_identifier,
            name,
            created_at,
            session_timeout_seconds,
            certificates,
            filesystem_configurations,
            // Nothing has changed this session yet, so the last instant it changed is
            // the instant it came into being.
            last_updated_at: created_at,
            stopped_at: None,
            stop_tokens: BTreeMap::new(),
            files: BTreeMap::new(),
            context_scripts: Vec::new(),
            seq,
        }
    }
    /// The instant this session's time-to-live runs out, or `None` when that instant
    /// is not representable (so nothing can be past it).
    ///
    /// `sessionTimeoutSeconds` is modelled as "the duration in seconds
    /// (time-to-live) after which the session automatically terminates, regardless of
    /// ongoing activity", so the session's own requested value is the span — nothing
    /// chosen here. The model's range makes it positive; a value that is not is read as
    /// no time at all rather than as a time in the past.
    pub fn expires_at(&self) -> Option<SystemTime> {
        let ttl = Duration::from_secs(u64::try_from(self.session_timeout_seconds).unwrap_or(0));
        self.created_at.checked_add(ttl)
    }

    /// This session's `CodeInterpreterSessionStatus` at `now`: `TERMINATED` once a stop
    /// has terminated it or its time-to-live has run out, `READY` until then. Both
    /// readings come off stored state and the clock, so a read reports the termination
    /// that was already due without writing anything.
    pub fn status_as_of(&self, now: SystemTime) -> &'static str {
        if self.stopped_at.is_some() {
            return CODE_INTERPRETER_SESSION_TERMINATED;
        }
        match self.expires_at() {
            Some(expiry) if now >= expiry => CODE_INTERPRETER_SESSION_TERMINATED,
            _ => CODE_INTERPRETER_SESSION_READY,
        }
    }

    /// Terminate this session at `now`, as `StopCodeInterpreterSession` does: the
    /// status every later read reports becomes `TERMINATED`, and `now` is the
    /// `lastUpdatedAt` this stop and every replay of its `client_token` answer with.
    ///
    /// The workspace is dropped with the session's run, because the documented effect
    /// of a stop is that the session "releases associated resources" and cannot be
    /// restarted, so neither the files a tool wrote nor the execution context a run
    /// built is reachable afterwards.
    pub fn stop(&mut self, now: SystemTime, client_token: Option<&str>) {
        self.stopped_at = Some(now);
        self.last_updated_at = now;
        self.files.clear();
        self.context_scripts.clear();
        if let Some(token) = client_token {
            self.stop_tokens.insert(token.to_string(), now);
            while self.stop_tokens.len() > MAX_STOP_TOKENS_PER_SESSION {
                // The earliest-recorded token gives way first; the stop a caller just
                // replayed is the one that stays answerable.
                let Some(oldest) = self
                    .stop_tokens
                    .iter()
                    .min_by_key(|(key, at)| (**at, (*key).clone()))
                    .map(|(key, _)| key.clone())
                else {
                    break;
                };
                self.stop_tokens.remove(&oldest);
            }
        }
    }

    /// The `lastUpdatedAt` the stop this `client_token` already performed answered
    /// with, or `None` when this token has performed none.
    pub fn stop_replay(&self, client_token: &str) -> Option<SystemTime> {
        self.stop_tokens.get(client_token).copied()
    }

    /// How many bytes this session's workspace holds across every file in it.
    pub fn workspace_bytes(&self) -> usize {
        self.files.values().map(|file| file.bytes.len()).sum()
    }

    /// Whether this session's workspace has room for `batch` — the (key, size) pairs
    /// one call would store, each key at most once — or the ceiling the batch would
    /// pass.
    ///
    /// The whole batch is weighed at once, with the bytes each key already holds
    /// discounted, so a caller is told the workspace is full BEFORE any of its files is
    /// stored rather than part-way through.
    ///
    /// `bytes_elsewhere` is what every OTHER session of the store holds
    /// ([`ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState::session_bytes_elsewhere`]),
    /// so the whole-store ceiling [`MAX_STORED_SESSION_BYTES`] is weighed on the same
    /// write path as this session's own.
    pub fn room_for(
        &self,
        batch: &[(String, usize)],
        bytes_elsewhere: usize,
    ) -> Result<(), WorkspaceFull> {
        let mut added_keys = 0usize;
        let mut bytes = self.workspace_bytes();
        let mut accounted = self.accounted_bytes();
        for (key, size) in batch {
            match self.files.get(key) {
                Some(held) => {
                    bytes = bytes.saturating_sub(held.bytes.len());
                    accounted = accounted.saturating_sub(accounted_file_bytes(key, held.bytes.len()));
                }
                None => added_keys = added_keys.saturating_add(1),
            }
            bytes = bytes.saturating_add(*size);
            accounted = accounted.saturating_add(accounted_file_bytes(key, *size));
        }
        if self.files.len().saturating_add(added_keys) > MAX_SESSION_FILES {
            return Err(WorkspaceFull::Files(MAX_SESSION_FILES));
        }
        if bytes > MAX_SESSION_FILE_BYTES {
            return Err(WorkspaceFull::Bytes(MAX_SESSION_FILE_BYTES));
        }
        if bytes_elsewhere.saturating_add(accounted) > MAX_STORED_SESSION_BYTES {
            return Err(WorkspaceFull::StoreBytes(MAX_STORED_SESSION_BYTES));
        }
        Ok(())
    }

    /// Write `bytes` to the workspace key `path` at `now`, replacing whatever that key
    /// held, or refuse the write because the workspace has no room for it.
    ///
    /// Every ceiling is checked against what would be held AFTER this write, with the
    /// replaced file's own bytes discounted, so rewriting a file never fails on the size
    /// the previous version took. `bytes_elsewhere` is read the same way as in
    /// [`Self::room_for`].
    pub fn write_file(
        &mut self,
        path: &str,
        bytes: Vec<u8>,
        now: SystemTime,
        bytes_elsewhere: usize,
    ) -> Result<(), WorkspaceFull> {
        let replaced = self.files.get(path).map_or(0, |file| file.bytes.len());
        if !self.files.contains_key(path) && self.files.len() >= MAX_SESSION_FILES {
            return Err(WorkspaceFull::Files(MAX_SESSION_FILES));
        }
        let after = self
            .workspace_bytes()
            .saturating_sub(replaced)
            .saturating_add(bytes.len());
        if after > MAX_SESSION_FILE_BYTES {
            return Err(WorkspaceFull::Bytes(MAX_SESSION_FILE_BYTES));
        }
        let accounted_after = self
            .accounted_bytes()
            .saturating_sub(if self.files.contains_key(path) {
                accounted_file_bytes(path, replaced)
            } else {
                0
            })
            .saturating_add(accounted_file_bytes(path, bytes.len()));
        if bytes_elsewhere.saturating_add(accounted_after) > MAX_STORED_SESSION_BYTES {
            return Err(WorkspaceFull::StoreBytes(MAX_STORED_SESSION_BYTES));
        }
        self.files.insert(
            path.to_string(),
            SessionFile {
                bytes,
                written_at: now,
            },
        );
        Ok(())
    }

    /// Every workspace key directly under `directory` (the workspace root when it is
    /// empty), in path order: a file as `(key, Some(size))` and an immediate
    /// subdirectory as `(key, None)`, each named once.
    pub fn list_directory(&self, directory: &str) -> Vec<(String, Option<usize>)> {
        let prefix = if directory.is_empty() {
            String::new()
        } else {
            format!("{directory}/")
        };
        let mut entries: BTreeMap<String, Option<usize>> = BTreeMap::new();
        for (key, file) in &self.files {
            let Some(relative) = key.strip_prefix(prefix.as_str()) else {
                continue;
            };
            match relative.split_once('/') {
                // A deeper file names the subdirectory it is in, not itself.
                Some((child, _)) => {
                    entries.insert(format!("{prefix}{child}"), None);
                }
                None => {
                    entries.insert(key.clone(), Some(file.bytes.len()));
                }
            }
        }
        entries.into_iter().collect()
    }

    /// Keep `script` as part of this session's execution context, so the next run of
    /// its `executeCode` tool is evaluated on top of it.
    ///
    /// Called only for a script that ran to completion: one that ended on an exception
    /// defined nothing a later run can rely on. The oldest script gives way once
    /// either ceiling is passed, so the context a caller just built is the part that
    /// survives. `bytes_elsewhere` is what every OTHER session holds, so the context
    /// this session keeps holds the whole-store ceiling too: a script the store has no
    /// room for is not kept at all, exactly as one larger than the whole context is not.
    pub fn keep_context_script(&mut self, script: String, bytes_elsewhere: usize) {
        // The context this session may hold: its own ceiling, and no more than the room
        // the rest of the store leaves under the whole-store one.
        let own_room = MAX_STORED_SESSION_BYTES
            .saturating_sub(bytes_elsewhere)
            .saturating_sub(self.accounted_file_total())
            .min(MAX_CONTEXT_BYTES);
        if script.len() > own_room {
            // A script the context has no room for cannot be kept at all; the run itself
            // already happened.
            return;
        }
        self.context_scripts.push(script);
        while self.context_scripts.len() > MAX_CONTEXT_SCRIPTS || self.context_bytes() > own_room {
            if self.context_scripts.is_empty() {
                break;
            }
            self.context_scripts.remove(0);
        }
    }

    /// How many bytes this session's execution context holds across every script in it.
    pub fn context_bytes(&self) -> usize {
        self.context_scripts.iter().map(String::len).sum()
    }

    /// What this session is charged in the whole-store accounting
    /// ([`MAX_STORED_SESSION_BYTES`]): every workspace file's content, its key and the
    /// fixed per-entry overhead, plus every script of its execution context.
    pub fn accounted_bytes(&self) -> usize {
        self.accounted_file_total()
            .saturating_add(self.context_bytes())
    }

    /// The workspace half of [`Self::accounted_bytes`].
    fn accounted_file_total(&self) -> usize {
        self.files
            .iter()
            .map(|(key, file)| accounted_file_bytes(key, file.bytes.len()))
            .fold(0usize, usize::saturating_add)
    }
}

/// What one workspace file of `size` bytes stored under `key` is charged in the
/// whole-store accounting: its content, its key, and
/// [`SESSION_FILE_ENTRY_OVERHEAD_BYTES`].
fn accounted_file_bytes(key: &str, size: usize) -> usize {
    size.saturating_add(key.len())
        .saturating_add(SESSION_FILE_ENTRY_OVERHEAD_BYTES)
}

/// A resource this service's `ResourceNotFoundException` can name, with the SENTENCE AWS
/// words that message with.
///
/// The exception carries one `message` member (source: smithy_query query="shape"
/// shape="ResourceNotFoundException"), and the identifier that did not resolve is the
/// message's only variable part. Most of this service's resources are named by the
/// capitalised resource label, `": "`, then that identifier — so the label is the
/// resource's own name, not the input member's spelling — and one, the EVENT, is named by
/// a sentence of its own ([`Self::message`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotFoundResource {
    /// An AgentCore Memory container, addressed by `memoryId`.
    Memory,
    /// A code interpreter sandbox, addressed by `codeInterpreterIdentifier`.
    CodeInterpreter,
    /// A session: one of an actor's sessions in a memory container, or one of a code
    /// interpreter's sessions, both addressed by `sessionId`.
    Session,
    /// One event of a session, addressed by `eventId`, as a READ of that event reports it
    /// missing.
    EventRead,
    /// One event of a session as a DELETE of that event reports it missing: AgentCore
    /// words the two refusals differently, so the resource carries both sentences
    /// ([`NotFoundResource::message`]).
    EventDelete,
    /// A CUSTOM evaluator, addressed by `evaluatorId`: the only evaluator family a lookup
    /// reports missing, since a built-in or partner evaluator is one the service itself
    /// provides and an id of neither family names a custom evaluator the control plane
    /// created.
    CustomEvaluator,
    /// A browser, addressed by `browserIdentifier`: the tool the service provides or a
    /// browser the control plane created.
    Browser,
    /// A batch evaluation job, addressed by `batchEvaluationId`.
    BatchEvaluation,
    /// A payment session a payment manager owns, addressed by `paymentSessionId`.
    PaymentSession,
}

impl NotFoundResource {
    /// The label this resource is named by in a `ResourceNotFoundException` message.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Memory => "Memory",
            Self::CodeInterpreter => "Code Interpreter",
            Self::Session => "Session",
            Self::EventRead | Self::EventDelete => "Event",
            Self::CustomEvaluator => "Custom evaluator",
            Self::Browser => "Browser",
            Self::BatchEvaluation => "Batch Evaluation",
            Self::PaymentSession => "Payment session",
        }
    }

    /// The whole `ResourceNotFoundException` message for a lookup of this resource that
    /// missed, naming `identifier` — the identifier the request spelled for THIS resource,
    /// and the message's only variable part.
    ///
    /// An EVENT is worded by the operation that refuses: a READ states `Unable to find
    /// event with id <eventId>` and a DELETE states `Event not found with id: <eventId>`.
    /// Either way the sentence is fixed and the identifier is its only variable part,
    /// whatever step of the path made the event missing — an event of another session of
    /// the same actor included, which is not found for exactly the same reason as one that
    /// was never written. Every other resource is stated as its label, `": "`, then its
    /// identifier.
    #[must_use]
    pub fn message(self, identifier: &str) -> String {
        match self {
            Self::EventRead => format!("Unable to find event with id {identifier}"),
            Self::EventDelete => format!("{} not found with id: {identifier}", self.label()),
            _ => format!("{} not found: {identifier}", self.label()),
        }
    }
}

/// The `ResourceNotFoundException` message for a lookup that missed, as the resource that
/// did not resolve words one ([`NotFoundResource::message`]).
///
/// The identifier is the ONLY variable part, and it is the one that failed to resolve:
/// an identifier of an enclosing resource that *did* resolve is not part of the message.
#[must_use]
pub fn resource_not_found_message(resource: NotFoundResource, identifier: &str) -> String {
    resource.message(identifier)
}

/// The key a `memoryId` is stored under.
///
/// `MemoryId`'s pattern admits either the bare id or the
/// `arn:<partition>:bedrock-agentcore:<region>:<account>:memory/<id>` form, so both
/// spellings address one container; the bare id is the canonical key.
///
/// The rule itself is the SHARED one
/// ([`::service_common::agentcore_resources::memory_id`]) and this delegates to it: the
/// control plane registers a container under that key and this plane resolves one by it,
/// so the two cannot read a `memoryId` two different ways.
pub fn normalize_memory_id(memory_id: &str) -> String {
    ::service_common::agentcore_resources::memory_id(memory_id).to_string()
}

/// Mint an `eventId`.
///
/// `EventId`'s pattern is `^[0-9]+#[a-fA-F0-9]+$`: the decimal creation instant in
/// epoch milliseconds, then a hex discriminator taken from the engine-wide event
/// sequence so two events minted in one millisecond differ.
pub fn mint_event_id(seq: u64) -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    format!("{millis}#{seq:016x}")
}

/// Mint a `sessionId` for a `CreateEvent` that supplied none.
///
/// `SessionId`'s pattern is `^[a-zA-Z0-9][a-zA-Z0-9-_]*$` with length 1..100.
pub fn mint_session_id(seq: u64) -> String {
    format!("session-{seq:016x}")
}

/// Mint an id for a resource NAMED by the request that created it, in the form every
/// shape of this service whose pattern is `^[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10}$`
/// publishes (`ABTestId`, `BatchEvaluationId`): the name the request asked for (itself
/// `^[a-zA-Z][a-zA-Z0-9_]{0,47}$`, so every character of it is one the id admits), then a
/// ten-character discriminator — the low bits of the creation instant in epoch
/// milliseconds and of the engine-wide sequence, five lowercase hex digits each, which
/// keeps two resources created in one millisecond apart.
///
/// The ONE home of that id form, so two planes that publish the identical pattern cannot
/// spell it differently ([`mint_ab_test_id`], [`mint_batch_evaluation_id`]).
pub fn mint_named_resource_id(name: &str, created_at: SystemTime, seq: u64) -> String {
    let millis = created_at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_millis());
    format!("{name}-{:05x}{:05x}", millis & 0xf_ffff, seq & 0xf_ffff)
}

/// Mint a `memoryRecordId` for a record created at `created_at`.
///
/// `MemoryRecordId`'s pattern is `^mem-[a-zA-Z0-9-_]*$` with length 40..50, so the id is
/// the prefix the shape fixes followed by the record's creation instant in epoch
/// milliseconds and the engine-wide sequence, both in lowercase hex (44 characters in
/// all), which keeps two records minted inside the same millisecond apart. The instant
/// comes from the batch that is writing the record, so an id cannot disagree with the
/// `createdAt` reported beside it.
pub fn mint_memory_record_id(created_at: ::aws_smithy_types::DateTime, seq: u64) -> String {
    let millis = created_at
        .to_millis()
        .map_or(0, |millis| u64::try_from(millis).unwrap_or(0));
    format!("mem-{millis:016x}{seq:024x}")
}

/// Mint an id for a session shape whose pattern is `^[0-9a-zA-Z]{1,40}$` —
/// alphanumerics only, so the id is the creation instant in epoch milliseconds followed
/// by the engine-wide sequence, both in lowercase hex (26 characters), which keeps two
/// sessions started in one millisecond apart.
///
/// The ONE home of that id form: `CodeInterpreterSessionId` and `BrowserSessionId`
/// publish the identical pattern, so both tools planes mint through this rather than
/// each spelling the form again ([`mint_code_interpreter_session_id`],
/// [`mint_browser_session_id`]).
pub fn mint_alphanumeric_session_id(seq: u64) -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    format!("{millis:010x}{seq:016x}")
}

/// Mint a `CodeInterpreterSessionId`, in the form
/// [`mint_alphanumeric_session_id`] states for that shape's pattern.
pub fn mint_code_interpreter_session_id(seq: u64) -> String {
    mint_alphanumeric_session_id(seq)
}

/// Encode an opaque `nextToken` for `kind` resuming at `offset`.
///
/// The token is base64 (`aws_smithy_types::base64`, the codec the protocols already
/// use) over `"<kind>:<offset>"`: opaque to a caller, and tied to the list it came
/// from, so a token minted by one listing cannot silently resume another. It carries
/// the POSITION only, not the request that produced it, so a caller that changes the
/// other inputs between pages (a different `filter`, say) resumes at that offset into
/// the newly described list — the same contract AWS documents for a pagination token,
/// which is only meaningful for an otherwise identical request. An offset past the end
/// of the list is a page with nothing on it and no further token, which is what a
/// caller resuming a list that has since shrunk should see.
pub fn encode_next_token(kind: &str, offset: usize) -> String {
    ::aws_smithy_types::base64::encode(format!("{kind}:{offset}"))
}

/// Decode a `nextToken` minted by [`encode_next_token`] for `kind`, or `None` when
/// it is not one (which a handler answers with a `ValidationException`).
pub fn decode_next_token(kind: &str, token: &str) -> Option<usize> {
    let raw = ::aws_smithy_types::base64::decode(token).ok()?;
    let text = String::from_utf8(raw).ok()?;
    let offset = text.strip_prefix(kind)?.strip_prefix(':')?;
    offset.parse::<usize>().ok()
}

/// One page of `items`, starting at `offset`, plus the `nextToken` that resumes
/// after it (`None` on the last page).
///
/// `render` runs ONLY on the items of the page, so a response never materialises
/// more records than the page it returns, however many the store holds.
pub fn paginate_with<T, U>(
    items: &[T],
    kind: &str,
    offset: usize,
    max_results: usize,
    render: impl FnMut(&T) -> U,
) -> (Vec<U>, Option<String>) {
    let page: Vec<U> = items
        .iter()
        .skip(offset)
        .take(max_results)
        .map(render)
        .collect();
    let next_offset = offset.saturating_add(page.len());
    let next_token = (next_offset < items.len()).then(|| encode_next_token(kind, next_offset));
    (page, next_token)
}

/// The region this request was signed for, defaulting to [`DEFAULT_REGION`].
///
/// An ARN segment and an endpoint label admit `[a-z0-9-]+` only, so a scope region
/// outside that grammar is not stamped into either, and a label longer than
/// [`MAX_REGION_LABEL`] is no region this emulator reports back.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= MAX_REGION_LABEL
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The longest region label this emulator reads off a credential scope. Every published
/// AWS region label is far shorter, so the bound keeps a caller from choosing how long
/// the region segment of an ARN or an endpoint this service reports back is.
const MAX_REGION_LABEL: usize = 32;

/// The `iss` of a workload access token this service issues: the AgentCore endpoint of
/// the region the request was signed for, which is the authority that issued it.
pub fn workload_access_token_issuer(region: &str) -> String {
    format!("https://{ARN_SERVICE}.{region}.amazonaws.com")
}

/// The workload identity a `workloadName` addresses, in the token vault's `default`
/// directory — the same ARN the CONTROL plane mints for an identity of that name.
pub fn workload_identity_arn(region: &str, workload_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("workload-identity-directory/default/workload-identity/{workload_name}"),
    )
}

/// Mint a workload access token for `workload_name`, acting on behalf of `user_id` when
/// the issuing call named one, valid from `issued_at` until `expires_at`.
///
/// The token is an UNSECURED JWS in compact serialization (RFC 7515 §6, "Using the
/// Algorithm none"): `base64url(header) '.' base64url(claims) '.'`, with an empty
/// signature segment. AWS signs its workload access tokens with a key pair whose public
/// half the identity plane publishes; this emulator holds no such key pair, so it issues
/// the form RFC 7515 defines for a JWS that carries no signature rather than filling the
/// third segment with bytes that would only look like one.
///
/// Every claim is computed from the request and from this service's state: the issuing
/// authority for the region the request was signed for, the workload identity the
/// `workloadName` addresses as the audience, the user as the subject, the validity window
/// the issuance was recorded with, and a `jti` taken from the engine-wide sequence so two
/// tokens minted in one second are never the same token.
pub fn mint_workload_access_token(
    region: &str,
    workload_name: &str,
    user_id: Option<&str>,
    issued_at: SystemTime,
    expires_at: SystemTime,
    seq: u64,
) -> String {
    let claims = ::serde_json::json!({
        "iss": workload_access_token_issuer(region),
        "aud": workload_identity_arn(region, workload_name),
        "sub": user_id.unwrap_or(workload_name),
        "workloadName": workload_name,
        "iat": unix_seconds(issued_at),
        "exp": unix_seconds(expires_at),
        "jti": format!("{seq:016x}"),
    });
    let header = ::serde_json::json!({ "alg": UNSECURED_TOKEN_ALGORITHM, "typ": "JWT" });
    // The segment coding is the tree's ONE base64url encoder, beside the reader that
    // reads a minted token back, so the form this service issues cannot drift from the
    // form that reader accepts.
    format!(
        "{}.{}.",
        ::service_common::jwt::encode_base64url(header.to_string().as_bytes()),
        ::service_common::jwt::encode_base64url(claims.to_string().as_bytes())
    )
}

/// The `alg` of an unsecured JWS, which carries no signature (RFC 7515 §6).
const UNSECURED_TOKEN_ALGORITHM: &str = "none";

/// `time` as whole seconds since the epoch, which is how a token claim carries an instant
/// (RFC 7519 §2's NumericDate).
fn unix_seconds(time: SystemTime) -> i64 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .map(|since_epoch| i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState>)
}

/// The SHARED AgentCore resource inventory in the shared engine: call it with a handler's
/// `&*self.engine`.
///
/// A Memory container and a code interpreter are created by the CONTROL plane, a different
/// service, and this plane's own API creates neither: a write it serves names its parent in
/// the request path and nothing else. So the only way it can tell a parent this account
/// holds from one that was never created is the record that plane wrote, in ONE inventory
/// under one shared type
/// ([`::service_common::agentcore_resources::AgentCoreResourceInventory`]). The engine keys
/// state by TYPE, so a copy of its own inside
/// [`ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState`] would be invisible to the
/// plane that writes it.
///
/// LOCK ORDER: a handler takes this inventory FIRST, reads the one answer it needs and
/// releases it before taking [`service_state`] — the order every reader of the inventory
/// keeps, so two services cannot invert it.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn resource_inventory<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<
    ::engine::ServiceState<::service_common::agentcore_resources::AgentCoreResourceInventory>,
> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(
            ::engine::InMemoryEngine::state::<
                ::service_common::agentcore_resources::AgentCoreResourceInventory,
            >,
        )
}

/// Whether the account holds the memory container `memory_id` names, in either spelling
/// `MemoryId` admits — the ONE pre-condition every call that WRITES inside a container
/// checks before it writes, so no two of them disagree about whether a container exists.
///
/// `None` when the shared engine does not carry the inventory at all, which a handler
/// answers with a modelled server error rather than by writing.
pub fn holds_memory<Eng: Send + Sync + 'static>(engine: &Eng, memory_id: &str) -> Option<bool> {
    let inventory = resource_inventory(engine)?;
    let held = inventory.lock().holds_memory(memory_id);
    Some(held)
}

/// The pre-condition every call ADDRESSING something inside a memory container keeps,
/// whether it writes or only reports: a container is created through the CONTROL plane, so
/// its existence is resolved against the component that OWNS it ([`holds_memory`]) and
/// never against this plane's own index of what has been written into it — a container
/// nothing has been written into yet exists and holds nothing.
///
/// `Ok(())` when the account holds it. `Err(Some(message))` is the
/// `ResourceNotFoundException` message naming the container, with the identifier exactly as
/// the request spelled it; `Err(None)` says the shared engine does not carry the inventory
/// at all, which a handler answers with a modelled server error.
pub fn memory_container_resolved<Eng: Send + Sync + 'static>(
    engine: &Eng,
    memory_id: &str,
) -> Result<(), Option<String>> {
    match holds_memory(engine, memory_id) {
        None => Err(None),
        Some(false) => Err(Some(resource_not_found_message(
            NotFoundResource::Memory,
            memory_id,
        ))),
        Some(true) => Ok(()),
    }
}

/// Whether the account can reach the code interpreter `code_interpreter_identifier` names:
/// one the control plane created and has not deleted, or the tool the service provides to
/// every account. The companion of [`holds_memory`] for the tools plane, read the same way.
pub fn holds_code_interpreter<Eng: Send + Sync + 'static>(
    engine: &Eng,
    code_interpreter_identifier: &str,
) -> Option<bool> {
    let inventory = resource_inventory(engine)?;
    let held = inventory
        .lock()
        .holds_code_interpreter(code_interpreter_identifier);
    Some(held)
}
