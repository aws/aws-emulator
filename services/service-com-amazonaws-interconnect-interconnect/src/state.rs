// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-interconnect-interconnect service.
//!
//! Two resources live here:
//!
//! * **Environments** — AWS-managed and per-Region. The model binds no
//!   `CreateEnvironment` operation, so nothing stores them: the catalog a regional
//!   endpoint serves is the fixed function [`environment_catalog`] of the Region the
//!   request was signed for, and `ListEnvironments`/`GetEnvironment` — and every other
//!   operation that resolves an `environmentId` — read that one function for the Region
//!   they were called in. So a `GetEnvironment` is byte-identical to the entry
//!   `ListEnvironments` returned for that id, and an id of another Region's catalog
//!   resolves to nothing.
//! * **Connections** — caller-created. `CreateConnection` allocates one and every
//!   other connection operation reads or mutates that stored record. This service's
//!   endpoints are regional, so each record belongs to the Region its create was
//!   signed for and is resolved only for requests signed for that Region.
//!
//! Attach points are NOT held here: an attach point is a Direct Connect gateway of the
//! account, so `ListAttachPoints` resolves them against the service that owns them
//! ([`direct_connect`]) rather than reading a catalog of this service's own — and
//! `CreateConnection` resolves the attach point it is given through that same walk, so
//! what a Connection may be attached to is what the listing offers.
//!
//! Server-owned fields (`id`, `arn`, `sharedId`, `activationKey`, `ownerAccount`,
//! `provider`, `location`, `type`, `billingTier`, `state`) are computed here or in
//! the handler, never copied from the request.

pub mod direct_connect;

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::ops::types::{
    AttachPoint, Bandwidths, Connection, ConnectionSummary, CreateConnectionInput, Environment,
    Provider, RemoteAccountIdentifier,
};

/// The secret a listing's page cursors are authenticated under, which is what lets a
/// paginated read tell a `nextToken` it ISSUED from one a caller assembled.
pub use ::service_common::page_tokens::PageTokenKey;

/// The region an ARN carries when the request is unsigned, so no SigV4 credential
/// scope names one. Matches the region the emulator's own clients default to.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN namespace the model gives this service, which is the service segment of
/// every `Connection$arn` (`^arn:aws[a-z-]*:interconnect:…` per the member's pattern).
pub const ARN_NAMESPACE: &str = "interconnect";

/// The page size a list operation uses when the caller sends no `maxResults`.
///
/// `MaxResults` is modelled `@range(min: 1, max: 100)`, so the largest page a
/// caller can ask for is the natural default for one who asks for nothing.
pub const DEFAULT_PAGE_SIZE: usize = 100;

/// How long a connection stays in a transient, provisioning state (`deleting`
/// after `DeleteConnection`, `updating` after a bandwidth change) before the
/// clock settles it.
///
/// AWS documents no duration for either transition — only the states themselves —
/// so this is an emulator-chosen span inside the core's 250–750 ms window. It is
/// deliberately non-zero: a caller that reads straight back must still observe the
/// transient state and the refusals that go with it. The due instant is stored and
/// materialised on the next call; nothing ever sleeps on it.
pub const PROVISIONING_SPAN: Duration = Duration::from_millis(400);

/// How long ONE leg of a new connection's provisioning progression takes: the move
/// out of `requested`, and then the move out of `pending`.
///
/// AWS publishes no duration for either leg (the real move out of `requested` waits
/// on a human at the partner portal), so this is an emulator-chosen span inside the
/// core's 250–750 ms window, at its short end so a created connection reaches its
/// ready state promptly while a caller reading straight back still observes the
/// earlier state and the refusals that go with it. Each leg's due instant is stored
/// and materialised on the next call; nothing ever sleeps on it.
pub const ACTIVATION_SPAN: Duration = Duration::from_millis(250);


/// How long a `deleted` connection stays readable before the store drops it.
///
/// A deleted connection has to stay readable for a while — the terminal state is
/// what a caller polling after `DeleteConnection` must see — but not forever, or a
/// caller that creates and deletes in a loop grows the store without bound. Once
/// the retention has elapsed the record and its two reverse-index entries are
/// removed by the same clock-driven pass that settles states, so a delete really
/// does release the memory and the identifier then reads as absent.
pub const DELETED_RECORD_RETENTION: Duration = Duration::from_secs(300);

/// The most connections an account HOLDS at once.
///
/// AWS enforces a per-account connection quota but publishes no number, so the
/// limit itself is emulator-chosen; what matters is that there IS one, and that
/// exceeding it is answered with the quota error the service models
/// (`ServiceQuotaExceededException`) rather than by growing the store for the life
/// of the process. A quota counts live resources, so a record that has reached the
/// terminal `deleted` state does not consume it — deleting a connection frees the
/// budget at once — and such records leave the store entirely after
/// [`DELETED_RECORD_RETENTION`], which is what bounds the memory.
pub const MAX_CONNECTIONS: usize = 200;

/// The most records the store holds at once, live and terminal together.
///
/// The quota above counts only live connections, so without a second bound a
/// create-and-delete loop could hold arbitrarily many `deleted` records until
/// their retention elapsed. This cap keeps the store bounded by COUNT as well as
/// by the clock: once it is reached, the oldest terminal record is dropped to make
/// room (a live connection is never dropped — the quota refuses those instead).
pub const MAX_STORED_RECORDS: usize = MAX_CONNECTIONS * 2;

/// How many times `CreateConnection` re-rolls a random `ConnectionId` before giving
/// up. Bounded so the allocation loop cannot spin on the shared lock; with eight
/// hex characters against at most [`MAX_CONNECTIONS`] records, exhausting it means
/// the random source is broken, which is a server fault.
pub const ID_ALLOCATION_ATTEMPTS: usize = 16;

/// A state change the clock owes a connection: at `due`, its state becomes `state`.
#[derive(Debug, Clone)]
pub struct PendingTransition {
    /// The instant the change became (or becomes) due.
    pub due: Instant,
    /// The `ConnectionState` the connection settles into.
    pub state: String,
}

/// The transition the clock owes a connection that has just entered `state` at
/// `entered`, or `None` where `state` owes nothing.
///
/// This is the documented `ConnectionState` progression of a connection being
/// brought up — "requested: the initial state of a connection. The state will
/// remain here until the Connection is accepted on the Partner portal" ->
/// "pending: the connection has been accepted and is being provisioned between AWS
/// and the Partner" -> "available: the connection has been fully provisioned" — and
/// the CLOCK carries a connection along it so no connection is left sitting in a
/// non-terminal state for ever. Every other state is either terminal (`available`,
/// `deleted`), impaired by a circuit fault the emulator cannot have (`down`,
/// `failed`), or already owed a transition by the call that set it (`deleting`,
/// `updating`), so none of them is advanced from here.
///
/// The clock stands in for the TIME each leg takes and for nothing a caller decides:
/// it records no acceptance ([`StoredConnection::proposal_accepted`] is what records
/// one), so a proposal nobody has accepted stays acceptable however far along this
/// progression its connection has been carried, and [`apply_acceptance`] — the one
/// writer of an acceptance — re-enters the accepted state and takes the rest of the
/// progression from the instant the acceptance is made.
///
/// Derived from the state alone, so EVERY path that leaves a connection in a
/// provisioning state hands it on along the same progression — the create that
/// allocates it, and the clock leg or settled update that lands it there.
pub fn owed_progression(state: &str, entered: Instant) -> Option<PendingTransition> {
    let next = match state {
        "requested" => "pending",
        "pending" => "available",
        _ => return None,
    };
    Some(PendingTransition {
        due: entered + ACTIVATION_SPAN,
        state: next.to_string(),
    })
}

/// The parameters a connection's `activationKey` CAPTURED: what the initial
/// creation request asked for, which is what the proposal that key names offers.
///
/// Kept beside the record rather than read back off it, because the two are
/// different things once the connection has been modified: `Connection$bandwidth` is
/// "the specific selected bandwidth of this connection", which `UpdateConnection`
/// ("we support modifications to the connection's description and/or bandwidth")
/// reassigns, while the proposal's is "the bandwidth of the proposed Connection" —
/// the one the key "captures … from the initial creation request", chosen before
/// there was a connection to modify. So this is written ONCE, where the key is
/// minted, and never again; the same reason [`IdempotentCreate`] holds the request's
/// own members instead of reading the record back.
#[derive(Debug, Clone)]
pub struct CapturedProposal {
    /// The Environment "upon which the Connection would be placed if this proposal
    /// were accepted", as the creation request named it.
    pub environment_id: String,
    /// "The bandwidth of the proposed Connection", as the creation request asked
    /// for it.
    pub bandwidth: String,
}

impl CapturedProposal {
    /// What `input` asked for, taken straight off the creation request that minted
    /// the key — the one place a proposal's parameters enter the store.
    ///
    /// `environment` is the catalog entry that request resolved against, so the
    /// capture names the Environment the connection is actually placed upon rather
    /// than however the request spelled its identifier.
    pub fn of(input: &CreateConnectionInput, environment: &Environment) -> Self {
        Self {
            environment_id: environment.environment_id.clone(),
            bandwidth: input.bandwidth.clone(),
        }
    }
}

/// A persisted connection: the modelled resource plus the bookkeeping that is not
/// part of any response shape.
#[derive(Debug)]
pub struct StoredConnection {
    /// The resource exactly as `GetConnection` returns it.
    pub connection: Connection,
    /// The parameters this connection's `activationKey` captured, which are what
    /// `DescribeConnectionProposal` reports for that key: the proposal's Environment
    /// and bandwidth "cannot be modified", so they are read from here and no later
    /// write to the record changes them.
    pub proposal: CapturedProposal,
    /// The Region this connection belongs to: the one the `CreateConnection` that
    /// allocated it was signed for, which its ARN also carries. Only a request
    /// signed for this Region sees the record ([`visible_in`]).
    pub region: String,
    /// The remote-account identifier the creator supplied, which the partner of the
    /// Environment verifies the remote side by. Required at create time by an
    /// Environment that declares a `remoteIdentifierType`
    /// ([`remote_account_violation`]) and kept with
    /// the record because it is part of the connection that was requested; no
    /// response shape reports it back (`Connection` models no such member), so
    /// nothing reads it out again.
    pub remote_account: Option<RemoteAccountIdentifier>,
    /// The transition the clock owes this connection, if any.
    pub pending_transition: Option<PendingTransition>,
    /// Whether this connection's proposal HAS been accepted — the call that selects
    /// the Attach Point "to which the connection should be associated". Set by
    /// [`apply_acceptance`], which is the only writer of an acceptance, and by
    /// nothing else.
    ///
    /// Kept beside the record rather than read back off its `state`, because the two
    /// are different things here: the clock carries a connection along the
    /// provisioning progression on its own ([`owed_progression`]), standing in for
    /// the time each leg takes and not for anyone's acceptance, so a connection that
    /// has left `requested` has not necessarily been accepted by anybody. This is
    /// what a second acceptance of one proposal is judged against.
    pub proposal_accepted: bool,
    /// When this record leaves the store, set once it reaches `deleted`.
    pub evict_due: Option<Instant>,
}

/// What a `clientToken` was first used with, so a retry that carries the same
/// token but a DIFFERENT request is told so instead of being handed an unrelated
/// connection.
///
/// EVERY member of the create the caller owns is kept, because every one of them is
/// part of the request the token names — including the identifier of the remote
/// party, which is the one member a retry must not be able to swap silently. The
/// record is built from the input in [`Self::of`] and compared against a repeat in
/// [`Self::matches`], both taking the input shape itself, so the members the store
/// holds and the members a replay is judged on cannot drift apart.
///
/// The request-owned members are kept here rather than read back off the stored
/// connection, because `UpdateConnection` may since have changed the record's
/// description or bandwidth: idempotency is about the request that was made, not
/// about the resource's current shape. (`remoteAccount` is not on the resource at
/// all — `Connection` models no such member — so the request is the only place it
/// could be compared from.)
#[derive(Debug)]
pub struct IdempotentCreate {
    /// The connection that first call allocated.
    pub connection_id: String,
    /// `CreateConnection$environmentId` as it was sent.
    pub environment_id: String,
    /// `CreateConnection$bandwidth` as it was sent.
    pub bandwidth: String,
    /// `CreateConnection$attachPoint` as it was sent.
    pub attach_point: AttachPoint,
    /// `CreateConnection$description` as it was sent, absent included.
    pub description: Option<String>,
    /// `CreateConnection$remoteAccount` as it was sent, absent included.
    pub remote_account: Option<RemoteAccountIdentifier>,
    /// The tag set `CreateConnection$tags` asked for, in the one representation this
    /// service holds a tag set in ([`stored_tag_set`]) — so the comparison below
    /// judges the tag set the create ASKED FOR, which a `tags` map with no entries
    /// and no `tags` member at all are equally a request for, rather than which of
    /// the two spellings carried it.
    pub tags: StoredTagSet,
}

/// Whether `stored` is a connection a request signed for `region` can see.
///
/// This service's endpoints are regional (`https://interconnect.{Region}.{dnsSuffix}`),
/// so a connection belongs to the Region it was created in: every read, update and
/// delete issued against another Region's endpoint sees nothing, exactly as it sees
/// nothing for an identifier that was never allocated. ONE predicate, so every
/// resolution path below scopes the store the same way.
pub fn visible_in(stored: &StoredConnection, region: &str) -> bool {
    stored.region == region
}

impl IdempotentCreate {
    /// What `connection_id`'s create carried, taken straight off the request that
    /// made it: the one place a caller-owned member enters the replay index.
    pub fn of(connection_id: String, input: &CreateConnectionInput) -> Self {
        Self {
            connection_id,
            environment_id: input.environment_id.clone(),
            bandwidth: input.bandwidth.clone(),
            attach_point: input.attach_point.clone(),
            description: input.description.clone(),
            remote_account: input.remote_account.clone(),
            tags: stored_tag_set(input.tags.as_ref()),
        }
    }

    /// Whether a repeated request is the SAME request this token created a
    /// connection for: every member of the create the caller owns compared, absence
    /// included, so a repeat differing in any one of them is a different request.
    ///
    /// The tag set is compared as the set the create ASKED FOR ([`stored_tag_set`]),
    /// which is what the connection the token names was given: two spellings of "no
    /// tags" ask for the same set, so neither is a different request from the other.
    pub fn matches(&self, input: &CreateConnectionInput) -> bool {
        self.environment_id == input.environment_id
            && self.bandwidth == input.bandwidth
            && attach_point_eq(&self.attach_point, &input.attach_point)
            && self.description == input.description
            && remote_account_eq(self.remote_account.as_ref(), input.remote_account.as_ref())
            && self.tags == stored_tag_set(input.tags.as_ref())
    }
}

/// Per-service state for com-amazonaws-interconnect-interconnect. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsInterconnectInterconnectState {
    /// The live connections, keyed by `id`, each record carrying the Region it
    /// belongs to: an operation reaches them through [`Self::connection_in`] /
    /// [`Self::connections_in`], so one Region's endpoint never answers for
    /// another's connections. A deleted connection is kept in the
    /// `deleted` state rather than dropped straight away, so a later
    /// `GetConnection` reports the terminal state instead of a 404 — and is
    /// evicted once [`DELETED_RECORD_RETENTION`] has elapsed. At most
    /// [`MAX_CONNECTIONS`] entries.
    pub connections: BTreeMap<String, StoredConnection>,
    /// `(Region, clientToken)` -> what that token created, so a retried
    /// `CreateConnection` replays the first outcome instead of allocating a second
    /// connection, and a retry carrying the same token with different parameters is
    /// refused. Keyed by the Region too ([`idempotency_key`]): a token is scoped to
    /// the regional endpoint that first carried it, so the same token sent to another
    /// Region is one that endpoint has never seen. Every
    /// entry names a connection in `connections`, and a dangling one is dropped on
    /// the next operation, so this map is bounded by [`MAX_STORED_RECORDS`] however
    /// many tokens a caller invents.
    pub idempotency_tokens: BTreeMap<(String, String), IdempotentCreate>,
    /// `activationKey` -> connection id: the reverse index
    /// `DescribeConnectionProposal` / `AcceptConnectionProposal` resolve a
    /// partner-portal key through. One entry per stored connection, dropped with it,
    /// so it is bounded by [`MAX_STORED_RECORDS`] too. The connection the entry names
    /// is resolved through [`Self::proposal_in`], so a key whose connection belongs to
    /// another Region, or has had a `DeleteConnection` made on it, resolves to nothing.
    pub activation_keys: BTreeMap<String, String>,
    /// The secret this store's page cursors are authenticated under, drawn from the
    /// system's randomness when the store is created ([`PageTokenKey`]): it is what lets
    /// a listing tell a `nextToken` it issued from one a caller built
    /// ([`decode_cursor`]). Held only in memory, never reported and never logged.
    pub page_token_key: PageTokenKey,
}

impl Default for ComAmazonawsInterconnectInterconnectState {
    fn default() -> Self {
        Self {
            connections: BTreeMap::new(),
            idempotency_tokens: BTreeMap::new(),
            activation_keys: BTreeMap::new(),
            // One key per store, from the system's randomness: see `PageTokenKey`.
            page_token_key: PageTokenKey::new(),
        }
    }
}

impl ComAmazonawsInterconnectInterconnectState {
    /// Apply every state change the clock has already made due.
    ///
    /// Called at the top of each connection operation, including the reporting
    /// ones: a newly created connection is accepted and then provisioned, a
    /// `deleting` connection becomes `deleted`, and an `updating` one settles back,
    /// at the instant stored when the call that started the change ran — whether or
    /// not anybody asked in between. It materialises only what was already owed, so
    /// it never moves a state a call did not start.
    ///
    /// A progression of several legs ([`owed_progression`]) is materialised leg by
    /// leg in ONE pass, each leg due a span after the one before it rather than
    /// after `now`: a caller that waits out the whole progression and then asks
    /// sees the state the clock owed it, not just the next leg of it. The pass
    /// terminates whatever the store holds — a leg's successor is due strictly
    /// later than the leg itself, and [`owed_progression`] runs forward to a state
    /// that owes nothing — so a single call does bounded work.
    pub fn materialise_due_transitions(&mut self, now: Instant) {
        let mut evicted: Vec<String> = Vec::new();
        for (id, stored) in &mut self.connections {
            while stored
                .pending_transition
                .as_ref()
                .is_some_and(|pending| pending.due <= now)
            {
                let Some(pending) = stored.pending_transition.take() else {
                    break;
                };
                stored.connection.state = pending.state;
                // Whatever the connection has just become may itself owe the next
                // leg, timed from the instant this one was due.
                stored.pending_transition = owed_progression(&stored.connection.state, pending.due);
                if stored.connection.state == DELETED_CONNECTION_STATE {
                    stored.evict_due = Some(pending.due + DELETED_RECORD_RETENTION);
                }
            }
            if stored.evict_due.is_some_and(|evict_due| evict_due <= now) {
                evicted.push(id.clone());
            }
        }
        // A record whose retention has run out leaves the store with both of its
        // reverse-index entries, so neither index outlives the connection it names.
        for id in evicted {
            self.remove_connection(&id);
        }
        self.drop_dangling_index_entries();
    }

    /// Remove one connection and every index entry that names it, so the three maps
    /// are never out of step.
    fn remove_connection(&mut self, id: &str) {
        self.connections.remove(id);
        self.activation_keys
            .retain(|_, connection_id| connection_id != id);
        self.idempotency_tokens
            .retain(|_, created| created.connection_id != id);
    }

    /// Drop any index entry whose connection is no longer stored.
    ///
    /// This is what BOUNDS the two side indexes: every entry in `activation_keys`
    /// and `idempotency_tokens` names a connection in `connections`, and that map
    /// holds at most [`MAX_STORED_RECORDS`] records, so neither index can grow past
    /// that however many `clientToken`s a caller invents. Cheap (a scan of at most
    /// a few hundred entries) and called on every connection operation, so the
    /// invariant does not depend on each removal site remembering it.
    pub fn drop_dangling_index_entries(&mut self) {
        let connections = &self.connections;
        self.activation_keys
            .retain(|_, connection_id| connections.contains_key(connection_id));
        self.idempotency_tokens
            .retain(|_, created| connections.contains_key(&created.connection_id));
    }

    /// The stored connection `identifier` names IN `region`, which is the only
    /// connection a request signed for that Region can reach by identifier.
    ///
    /// A connection belongs to the Region it was created in ([`visible_in`]), so an
    /// identifier allocated through another Region's endpoint resolves to nothing
    /// here — the same answer an identifier that was never allocated gets.
    pub fn connection_in(&self, region: &str, identifier: &str) -> Option<&StoredConnection> {
        self.connections
            .get(identifier)
            .filter(|stored| visible_in(stored, region))
    }

    /// The proposal the partner-portal `activation_key` names in `region` — the only
    /// connection a request whose sole handle is that key can reach.
    ///
    /// Two scopes, both the ones every other resolution here applies. The connection
    /// the `activationKey` -> connection-id index names is resolved through
    /// [`Self::connection_in`], so a key minted through another Region's endpoint
    /// resolves to nothing. And a connection a `DeleteConnection` has been made on
    /// resolves to nothing either: the record stays readable BY ITS IDENTIFIER so a
    /// caller polling the delete sees the teardown and then the terminal state, but
    /// the connectivity the proposal offered is going or gone, so the key names
    /// nothing left to describe or confirm — the same answer a key that was never
    /// issued gets.
    pub fn proposal_in(&self, region: &str, activation_key: &str) -> Option<&StoredConnection> {
        self.activation_keys
            .get(activation_key)
            .and_then(|connection_id| self.connection_in(region, connection_id))
            .filter(|stored| !is_deleted_or_deleting(&stored.connection.state))
    }

    /// [`Self::connection_in`] for the operations that MUTATE the record they
    /// resolve, scoping the store by the same predicate.
    pub fn connection_in_mut(
        &mut self,
        region: &str,
        identifier: &str,
    ) -> Option<&mut StoredConnection> {
        self.connections
            .get_mut(identifier)
            .filter(|stored| visible_in(stored, region))
    }

    /// Every connection a request signed for `region` sees, in `ConnectionId` order
    /// (`connections` is a `BTreeMap`, so iteration already is).
    pub fn connections_in<'a>(
        &'a self,
        region: &'a str,
    ) -> impl Iterator<Item = (&'a String, &'a StoredConnection)> {
        self.connections
            .iter()
            .filter(move |(_, stored)| visible_in(stored, region))
    }

    /// The stored connection an `AmazonResourceName` names field for field IN
    /// `region`, for the input that carries an ARN as a HANDLE on another resource
    /// (`AttachPoint$arn`) rather than as the address of the resource it acts on.
    ///
    /// Here the ARN identifies the resource in FULL — partition, Region and account
    /// included — so an ARN whose fields differ from the stored one names a
    /// resource this store does not hold, exactly as an unknown id does. And the
    /// store is scoped by the SAME predicate every other resolution here uses
    /// ([`visible_in`]): a connection belongs to the Region it was created in, so
    /// an ARN naming one created through another Region's endpoint resolves to
    /// nothing, whatever that ARN carries. A connection that has reached the
    /// terminal `deleted` state is gone too: the store keeps the record readable so
    /// a caller polling a delete sees the terminal state, but a deleted resource is
    /// no longer one this account holds.
    pub fn connection_in_by_arn(&self, region: &str, arn: &str) -> Option<&StoredConnection> {
        let id = connection_id_from_arn(arn)?;
        self.connections
            .get(id)
            .filter(|stored| visible_in(stored, region) && holds_arn(stored, arn))
    }

    /// The connection `arn` names as an ATTACH POINT in `region`, which is the only
    /// connection a new Connection is ever provisioned onto by ARN.
    ///
    /// Two filters, both of them rules this crate already applies elsewhere. The ARN
    /// resolves the resource the way a HANDLE on a connection resolves here
    /// ([`Self::connection_in_by_arn`]): in full, and inside the scope the request
    /// addressed. And the connectivity behind it has to BE there: an attach point is a
    /// handle whose whole subject is the connectivity the new Connection is provisioned
    /// onto, so the handle names something only while the connection behind it is
    /// providing that connectivity ([`provides_connectivity`]) — the readiness rule the
    /// union's gateway member is held to as well, where only a gateway that is ready for
    /// use is a valid attach point ([`direct_connect`]). That covers both ends of the
    /// connection's life: one still being brought up has no connectivity yet, and one a
    /// `DeleteConnection` has been made on has none any more — the filter
    /// [`Self::proposal_in`] applies to the other handle of that kind. The record itself
    /// stays readable by its identifier and by its ARN, which is how a caller polling the
    /// delete sees the teardown.
    pub fn attach_point_connection_in(
        &self,
        region: &str,
        arn: &str,
    ) -> Option<&StoredConnection> {
        self.connection_in_by_arn(region, arn)
            .filter(|stored| provides_connectivity(&stored.connection.state))
    }

    /// The stored connection an `AmazonResourceName` ADDRESSES in `region`, for the
    /// operations whose `arn` member IS the resource they act on (the tag operations).
    ///
    /// What identifies the resource is the `ConnectionId` the ARN's resource segment
    /// carries, resolved among the connections the called endpoint holds by the same
    /// predicate every other resolution here uses ([`visible_in`]). The ARN's Region
    /// segment is NOT part of that key: the member's pattern leaves it unconstrained
    /// (`[^:]+`), so it is a value the caller may spell any way and it addresses no
    /// scope of its own — the scope an ARN-addressed request is judged against is its
    /// partition and account ([`arn_scope_violation`]), and the Region it reaches is
    /// the one the request itself is signed for. A connection that has reached the
    /// terminal `deleted` state is gone: the record stays readable by its identifier
    /// so a caller polling a delete sees the terminal state, but a deleted resource is
    /// no longer one this account holds.
    pub fn connection_in_for_arn(&self, region: &str, arn: &str) -> Option<&StoredConnection> {
        let id = connection_id_from_arn(arn)?;
        self.connection_in(region, id).filter(|stored| exists_as_resource(stored))
    }

    /// [`Self::connection_in_for_arn`] for the operations that MUTATE the tag set they
    /// resolve, resolving the same resource under the same rule.
    pub fn connection_in_for_arn_mut(
        &mut self,
        region: &str,
        arn: &str,
    ) -> Option<&mut StoredConnection> {
        let id = connection_id_from_arn(arn)?;
        self.connection_in_mut(region, id)
            .filter(|stored| exists_as_resource(stored))
    }

    /// Make room for one more record: while the store is at [`MAX_STORED_RECORDS`],
    /// drop the terminal record whose retention expires first (the oldest one to
    /// have been deleted), together with its reverse-index entries. A live
    /// connection is never dropped, so a caller holding the quota is refused rather
    /// than quietly losing a connection.
    pub fn make_room_for_one(&mut self) {
        while self.connections.len() >= MAX_STORED_RECORDS {
            let oldest = self
                .connections
                .iter()
                .filter_map(|(id, stored)| stored.evict_due.map(|due| (due, id.clone())))
                .min()
                .map(|(_, id)| id);
            let Some(id) = oldest else {
                return;
            };
            self.remove_connection(&id);
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsInterconnectInterconnectState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsInterconnectInterconnectState>)
}

/// The region this request was signed for, which is the region every ARN the
/// request mints carries. Falls back to [`DEFAULT_REGION`] for an unsigned request.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// The key a `clientToken` is held under in [`ComAmazonawsInterconnectInterconnectState::idempotency_tokens`]:
/// the Region the request addressed, then the token itself.
pub fn idempotency_key(region: &str, token: &str) -> (String, String) {
    (region.to_string(), token.to_string())
}

/// Two `Provider` values name the same partner on the same side of the union.
///
/// The union has no derived equality (its variants are the two partner kinds), and
/// `ListConnections`/`ListEnvironments` both filter on it.
pub fn provider_eq(left: &Provider, right: &Provider) -> bool {
    match (left, right) {
        (Provider::cloudServiceProvider(a), Provider::cloudServiceProvider(b))
        | (Provider::lastMileProvider(a), Provider::lastMileProvider(b)) => a == b,
        _ => false,
    }
}

/// The `@length` the model gives the target of BOTH `Provider` members
/// (`CloudServiceProvider` and `LastMileProvider` are each `@length(max: 32)`): a
/// ceiling, and no floor at all, so a partner name is held to its length and to nothing
/// else — neither member carries a `@pattern`.
pub const PROVIDER_NAME_MAX_LENGTH: usize = 32;

/// Why the value a `Provider` carries is not one the member it arrived in may hold — or
/// `None` when it is inside that member's own modelled `@length`.
///
/// Hand-checked for the reason the crate's other two unions are
/// ([`attach_point_violation`], [`remote_account_violation`]): the boundary validator
/// holds a top-level member to its target's constraints but not a member one level down,
/// inside a union. So every reader of a `Provider` judges the value here first, and a
/// value outside the member's constraints is a fault in the REQUEST, reported as invalid
/// input, rather than compared against stored records and answered as a page the caller
/// cannot tell from "nothing matched".
///
/// The wording is the crate's own sentence for a value whose LENGTH its member cannot
/// hold (the one [`remote_account_violation`] words), naming the member and the length
/// rather than echoing the value back.
pub fn provider_violation(provider: &Provider) -> Option<String> {
    let (member, name) = match provider {
        Provider::cloudServiceProvider(name) => ("cloudServiceProvider", name),
        Provider::lastMileProvider(name) => ("lastMileProvider", name),
    };
    // `@length` on a string counts characters, not bytes, so the name is measured the
    // way the model measures it.
    let length = name.chars().count();
    (length > PROVIDER_NAME_MAX_LENGTH).then(|| {
        format!(
            "{REFUSAL_ENVELOPE}provider.{member} must be at most {PROVIDER_NAME_MAX_LENGTH} characters long, and one of length {length} is not"
        )
    })
}

/// Two `AttachPoint` values identify the same attach point, which is what the
/// `ListConnections` `attachPoint` filter compares.
pub fn attach_point_eq(left: &AttachPoint, right: &AttachPoint) -> bool {
    match (left, right) {
        (AttachPoint::directConnectGateway(a), AttachPoint::directConnectGateway(b))
        | (AttachPoint::Arn(a), AttachPoint::Arn(b)) => a == b,
        _ => false,
    }
}

/// Two optional `RemoteAccountIdentifier` values carry the same identifying
/// information, absence included — which is what a replayed `clientToken` compares
/// the remote party on.
///
/// The union has no derived equality (a variant per kind of identifier the service
/// accepts), so the comparison is per variant, the way [`attach_point_eq`] and
/// [`provider_eq`] compare theirs.
pub fn remote_account_eq(
    left: Option<&RemoteAccountIdentifier>,
    right: Option<&RemoteAccountIdentifier>,
) -> bool {
    match (left, right) {
        (None, None) => true,
        (
            Some(RemoteAccountIdentifier::Identifier(a)),
            Some(RemoteAccountIdentifier::Identifier(b)),
        ) => a == b,
        _ => false,
    }
}

/// How the service words an `attachPoint` that resolves to nothing a connection can
/// be attached to on the named Environment.
///
/// An attach point has to be one the caller has access to and that is valid for the
/// Environment — the set `ListAttachPoints` reports for it — so the message names the
/// identifier the request carried and the environment it was offered for, and is
/// built from the request rather than from the store.
pub fn attach_point_not_valid_message(identifier: &str, environment_id: &str) -> String {
    format!(
        "{REFUSAL_ENVELOPE}{identifier} is not an attach point valid for environment \
         {environment_id}"
    )
}

/// The `EnvironmentState` on which an Environment takes no new Connection at all.
///
/// The state's own documentation draws the line: `available` is "the environment is
/// available and new `Connection` objects can be requested" and `limited` is "the
/// environment is available, but overall capacity is limited", while this one is "the
/// environment is currently unavailable".
pub const UNAVAILABLE_ENVIRONMENT_STATE: &str = "unavailable";

/// Whether ANY attach point is valid for the Environment — which is whether a
/// Connection can be placed on it at all.
///
/// `ListAttachPoints` reports "all Attach Points the caller has access to that are
/// valid for the specified Environment", and an attach point is where a Connection on
/// that Environment "will logically connect" (`CreateConnection$attachPoint`): an
/// Environment that can carry no Connection therefore has no valid attach point,
/// whatever the account holds. Two members of the Environment record say it carries
/// none — its `state` ([`UNAVAILABLE_ENVIRONMENT_STATE`]), and a `bandwidths.available`
/// offering nothing, which leaves no value the required `CreateConnection$bandwidth`
/// could carry ([`bandwidth_availability_violation`]) — so this is the ONE predicate
/// both the listing and the per-request attach-point judgement scope themselves by, and
/// what the listing offers for an Environment cannot drift from what a create onto it
/// accepts.
///
/// `environment` is an `Option` for the same reason
/// [`bandwidth_availability_violation`]'s is: a caller that reaches the catalog by id
/// may find no record there, and an Environment the catalog does not hold carries no
/// Connection either.
pub fn environment_offers_attach_points(environment: Option<&Environment>) -> bool {
    environment.is_some_and(|environment| {
        environment.state != UNAVAILABLE_ENVIRONMENT_STATE
            && environment
                .bandwidths
                .available
                .as_deref()
                .is_some_and(|available| !available.is_empty())
    })
}

/// The `@length` the model gives `AttachPoint$arn`, whose pattern is an Interconnect
/// connection ARN: the shortest such ARN is 59 characters and the longest 150.
pub const ATTACH_POINT_ARN_LENGTH: (usize, usize) = (59, 150);

/// The `@length` the model gives `AttachPoint$directConnectGateway`: a gateway is
/// identified by the hyphenated UUID its pattern spells out, which is exactly this many
/// characters.
pub const ATTACH_POINT_GATEWAY_LENGTH: usize = 36;

/// The two prefixes `ConnectionId`'s `^(mcc|lmcc)-[a-z0-9]{8}$` pattern allows — the two
/// [`connection_id_prefix`] mints, one per kind of partner.
pub const CONNECTION_ID_PREFIXES: [&str; 2] = ["mcc", "lmcc"];

/// How many characters follow a `ConnectionId`'s prefix: the `[a-z0-9]{8}` its pattern
/// ends with, which is what [`new_connection_id`] allocates.
pub const CONNECTION_ID_SUFFIX_LENGTH: usize = 8;

/// Whether `id` is spelled the way `ConnectionId`'s `^(mcc|lmcc)-[a-z0-9]{8}$` pattern
/// spells one: one of the two prefixes, a hyphen, then eight lowercase letters or digits.
fn is_connection_id_text(id: &str) -> bool {
    let Some((prefix, suffix)) = id.split_once('-') else {
        return false;
    };
    CONNECTION_ID_PREFIXES.contains(&prefix)
        && suffix.chars().count() == CONNECTION_ID_SUFFIX_LENGTH
        && suffix
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// Whether `arn` is spelled the way `AttachPoint$arn`'s pattern
/// (`^arn:aws[a-z-]*:interconnect:[^:]+:[0-9]{12}:connection/(mcc|lmcc)-[a-z0-9]{8}$`)
/// and `@length` spell an Interconnect connection ARN: the six fields
/// [`parse_connection_arn`] reads, inside the modelled length, carrying a resource field
/// that is a `ConnectionId`.
///
/// The field walk is [`parse_connection_arn`]'s, so what counts as a connection ARN here
/// is what every other ARN-addressed path of this service resolves; only the two
/// constraints that walk deliberately leaves open — the overall length and the id's own
/// spelling — are added on top.
pub fn is_connection_arn_text(arn: &str) -> bool {
    let (min, max) = ATTACH_POINT_ARN_LENGTH;
    // `@length` on a string counts characters, not bytes, so the ARN is measured the
    // way the model measures it.
    (min..=max).contains(&arn.chars().count())
        && parse_connection_arn(arn).is_some_and(|parsed| is_connection_id_text(parsed.id))
}

/// Whether `identifier` is spelled the way `AttachPoint$directConnectGateway`'s pattern
/// (`^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`)
/// spells a Direct Connect gateway id: five hex groups of those widths, joined by
/// hyphens, in either case.
pub fn is_gateway_identifier_text(identifier: &str) -> bool {
    /// The widths of the five groups the member's pattern spells out, which together
    /// with the four hyphens make up [`ATTACH_POINT_GATEWAY_LENGTH`].
    const GROUP_WIDTHS: [usize; 5] = [8, 4, 4, 4, 12];
    let mut groups = identifier.split('-');
    GROUP_WIDTHS.iter().all(|width| {
        groups.next().is_some_and(|group| {
            group.chars().count() == *width && group.chars().all(|c| c.is_ascii_hexdigit())
        })
    }) && groups.next().is_none()
}

/// How the service words a `directConnectGateway` that is not a Direct Connect gateway
/// identifier at all, which is a fault in the request rather than a missing resource —
/// the gateway counterpart of [`not_a_connection_arn_message`].
pub fn not_a_gateway_identifier_message(identifier: &str) -> String {
    format!("{REFUSAL_ENVELOPE}{identifier} is not the ID of a DirectConnect Gateway")
}

/// Why the value an `AttachPoint` carries is not one the member it arrived in may hold —
/// or `None` when it is inside that member's own modelled `@length` and `@pattern`.
///
/// The union's members are constrained in the model and NOTHING before the handler holds
/// them to it: the boundary validator holds a top-level member to its target's
/// constraints but not a member one level down, inside a union — which is why the sibling
/// union is hand-checked the same way ([`remote_account_violation`]). So every reader of
/// an `AttachPoint` judges the value here first, and a value outside the member's
/// constraints is a fault in the REQUEST, reported as invalid input, rather than an
/// identifier resolved against state and then reported as a resource the account does not
/// hold: a caller can tell "I sent something that is not an attach-point identifier" from
/// "this account holds no such attach point".
///
/// Each member is worded by the sentence this crate already owns for a value that is not
/// an identifier of its kind at all ([`not_a_connection_arn_message`] for the ARN,
/// [`not_a_gateway_identifier_message`] for the gateway), so the same malformed value is
/// refused the same way whichever operation carried it.
pub fn attach_point_violation(attach_point: &AttachPoint) -> Option<String> {
    match attach_point {
        AttachPoint::Arn(arn) => {
            (!is_connection_arn_text(arn)).then(|| not_a_connection_arn_message(arn))
        }
        AttachPoint::directConnectGateway(identifier) => (!is_gateway_identifier_text(identifier))
            .then(|| not_a_gateway_identifier_message(identifier)),
    }
}

/// Judge the `attach_point` a request selected for a connection on `environment_id`
/// against this service's own store, answering `Ok(())` when the connection may be
/// provisioned onto it.
///
/// The ONE rule both writers of a connection's attach point apply (the create that
/// allocates the first attach point and the acceptance that associates one), so what a
/// Connection may be attached to does not depend on which operation carried it. Two
/// things are judged, in this order:
///
/// * the value the union carries, against the member's OWN modelled constraints
///   ([`attach_point_violation`]) — a value outside them is invalid input
///   ([`direct_connect::AttachPointRefusal::invalid_attach_point`]), never an identifier
///   resolved against state, so neither member reaches a store lookup or a cross-service
///   walk as something that is not an identifier of its kind;
/// * then, for the `arn` member only, the resource it names: the ARN has to name a
///   connection this store holds as an attach point
///   ([`ComAmazonawsInterconnectInterconnectState::attach_point_connection_in`]), and one
///   that does not is the not-found refusal
///   [`direct_connect::AttachPointRefusal::not_an_attach_point`] — the same refusal,
///   worded the same way, an identifier the account holds nothing for at all gets, so
///   what refuses the request never reports what the store does hold for that ARN.
///
/// The union's two members are therefore judged through ONE refusal channel: this answers
/// both members' constraints and the `arn` member's resource against the store under the
/// caller's lock, and [`direct_connect::judge_attach_point`] answers the
/// `directConnectGateway` resource — a resource of the service that owns those gateways —
/// off that lock, under the same readiness rule. Each answers `Ok(())` for what the other
/// owns, and each scopes itself to the Environment by the same predicate
/// ([`environment_offers_attach_points`]), so neither member names an attach point valid
/// for an Environment the other would refuse.
/// Judge the VALUE an `AttachPoint` carries against its own member's modelled
/// constraints ([`attach_point_violation`]), through the refusal channel the calling
/// operation declares: the half of [`judge_attach_point_in_store`] that needs no store
/// and no Environment at all.
///
/// Separated out because a value outside the member's constraints is a fault in the
/// REQUEST, which every handler that carries this union judges BEFORE it resolves any
/// resource the request names — so no verdict about a resource (an identifier that
/// resolves to nothing, a proposal already accepted) is ever reported for a request that
/// is not a well-formed one to begin with. The judgement itself stays in one place, so
/// the same malformed value is refused the same way and worded the same way whichever
/// operation carried it.
pub fn judge_attach_point_value<E: direct_connect::AttachPointRefusal>(
    attach_point: &AttachPoint,
) -> Result<(), E> {
    match attach_point_violation(attach_point) {
        Some(message) => Err(E::invalid_attach_point(message)),
        None => Ok(()),
    }
}

pub fn judge_attach_point_in_store<E: direct_connect::AttachPointRefusal>(
    store: &ComAmazonawsInterconnectInterconnectState,
    region: &str,
    attach_point: &AttachPoint,
    environment_id: &str,
) -> Result<(), E> {
    judge_attach_point_value::<E>(attach_point)?;
    let AttachPoint::Arn(arn) = attach_point else {
        return Ok(());
    };
    if environment_offers_attach_points(environment_in_region(region, environment_id).as_ref())
        && store.attach_point_connection_in(region, arn).is_some()
    {
        return Ok(());
    }
    Err(E::not_an_attach_point(attach_point_not_valid_message(
        arn,
        environment_id,
    )))
}

/// The `@length` the model gives `RemoteOwnerAccount`, the target of the ONLY member
/// `RemoteAccountIdentifier` offers — the whole of what the model says the identifier
/// may be, together with its pattern ([`is_remote_identifier_text`]).
pub const REMOTE_IDENTIFIER_LENGTH: (usize, usize) = (1, 255);

/// Whether `identifier` is built from the characters the member's pattern
/// (`^[-a-zA-Z0-9_@\.]+$`) allows: ASCII letters and digits, `-`, `_`, `@` and `.`.
///
/// Checked here because the boundary validator holds a top-level member to its
/// target's constraints but not a member one level down, inside a union.
fn is_remote_identifier_text(identifier: &str) -> bool {
    identifier
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '@' | '.'))
}

/// Why `supplied` is not the `remoteAccount` `environment` asks a create for — or
/// `None` when it is.
///
/// `Environment$remoteIdentifierType` is "the type of identifying information that
/// should be supplied to the `remoteAccount` parameter of a `CreateConnection` call
/// for this specific Environment": it tells the caller WHICH identity the partner of
/// that Environment will verify on its portal, so an Environment that declares one
/// needs identifying information on every create against it, and one that declares
/// none needs nothing and takes whatever the caller sends.
///
/// `remoteAccount` is OPTIONAL in the model precisely because whether it is needed is
/// the Environment's to say, so ABSENCE is refused on an Environment that declares a
/// type: that Environment's partner would have nothing to verify the remote side
/// with, and the connection could never be confirmed on its portal.
///
/// PRESENCE and the member's OWN modelled rules are all that is judged, the latter
/// through [`remote_identifier_violation`], which is also what a create judges on the
/// request alone. The union
/// offers a single generic `identifier` — "a generic bit of identifying information.
/// Can be used in place of any of the more specific types" — so what it may carry is
/// its `@length` and its `@pattern` and nothing else: the identity behind it is
/// verified later, at the partner portal, so no text shape of a kind is required of it
/// here and an opaque partner-issued identity is accepted whatever kind the
/// Environment declares.
pub fn remote_account_violation(
    environment: &Environment,
    supplied: Option<&RemoteAccountIdentifier>,
) -> Option<String> {
    if supplied.is_none() {
        return environment
            .remote_identifier_type
            .as_ref()
            .map(|_| format!("{REFUSAL_ENVELOPE}remoteAccount.identifier is required"));
    }
    remote_identifier_violation(supplied)
}

/// Why the identifier a `remoteAccount` CARRIES is not one the member may hold — or
/// `None` when it is inside the member's own modelled `@length` and `@pattern`, and for
/// a request that carries no `remoteAccount` at all.
///
/// The member-constraint half of [`remote_account_violation`], separated out because it
/// is a fault in the REQUEST and needs no Environment: a create judges it before it
/// resolves the Environment the request names, so an unresolvable `environmentId` cannot
/// mask it, while PRESENCE — the half that is the Environment's to say — stays with the
/// Environment.
///
/// Checked by hand because the boundary validator holds a top-level member to its
/// target's constraints but not a member one level down, inside a union (the same gap
/// [`attach_point_violation`] fills for the sibling union).
pub fn remote_identifier_violation(
    supplied: Option<&RemoteAccountIdentifier>,
) -> Option<String> {
    let Some(RemoteAccountIdentifier::Identifier(identifier)) = supplied else {
        return None;
    };
    // `@length` on a string counts characters, not bytes, so the identifier is
    // measured the way the model measures it.
    let length = identifier.chars().count();
    let (min, max) = REMOTE_IDENTIFIER_LENGTH;
    if !(min..=max).contains(&length) {
        return Some(format!(
            "{REFUSAL_ENVELOPE}remoteAccount.identifier must be between {min} and {max} characters long, and one of length {length} is not"
        ));
    }
    (!is_remote_identifier_text(identifier)).then(|| {
        format!(
            "{REFUSAL_ENVELOPE}remoteAccount.identifier must be made up of letters, digits and the characters -, _, @ and ., and this one is not"
        )
    })
}

/// Why `bandwidth` is not one a connection on the Environment `environment_id` names
/// may hold — or `None` when the Environment currently offers it.
///
/// `Environment$bandwidths.available` is "the list of currently available bandwidths",
/// so a bandwidth outside it cannot be allocated on that Environment even when the
/// Environment `supported`s it (which is exactly what `limited` means). Both of this
/// service's bandwidth writers (the create that allocates the first one and the update
/// that allocates a new one on the SAME Environment) judge their input here, so one
/// rule decides which bandwidths a stored connection can hold however it was reached,
/// including what an Environment offering no list at all offers: nothing.
///
/// `environment` is an `Option` because a writer that reaches the catalog by id may
/// find no record there, and `environment_id` is taken separately for exactly that
/// case — the refusal names the Environment the request resolved against whether or
/// not the catalog still holds it. The sentence carries this service's
/// [`REFUSAL_ENVELOPE`], like every other client-fault refusal it words itself.
pub fn bandwidth_availability_violation(
    environment: Option<&Environment>,
    environment_id: &str,
    bandwidth: &str,
) -> Option<String> {
    let available = environment
        .and_then(|environment| environment.bandwidths.available.as_deref())
        .unwrap_or_default();
    (!available.iter().any(|offered| offered == bandwidth)).then(|| {
        format!(
            "{REFUSAL_ENVELOPE}bandwidth '{bandwidth}' is not available on environment '{environment_id}'; available bandwidths are [{}]",
            available.join(", ")
        )
    })
}

/// The `ConnectionId` prefix an Environment's provider kind dictates.
///
/// The model's `Provider` members say it: a `cloudServiceProvider` environment
/// produces "Multicloud connections" (`mcc`), a `lastMileProvider` environment
/// "LastMile connections" (`lmcc`) — the two alternatives `ConnectionId`'s
/// `^(mcc|lmcc)-[a-z0-9]{8}$` pattern allows.
pub fn connection_id_prefix(provider: &Provider) -> &'static str {
    match provider {
        Provider::cloudServiceProvider(_) => "mcc",
        Provider::lastMileProvider(_) => "lmcc",
    }
}

/// A fresh `ConnectionId`: the provider-dictated prefix plus eight hex characters
/// (`[0-9a-f]`, taken from a v4 UUID's digits), which is inside the
/// `^(mcc|lmcc)-[a-z0-9]{8}$` alphabet the member's pattern allows.
pub fn new_connection_id(prefix: &str) -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    format!("{prefix}-{}", &hex[..CONNECTION_ID_SUFFIX_LENGTH])
}

/// A fresh `ConnectionSharedId`: the hyphenated v4 UUID the member's pattern
/// (`^[0-9a-fA-F]{8}-…-[0-9a-fA-F]{12}$`, length exactly 36) describes.
pub fn new_shared_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// A fresh `ActivationKey` — the token the creator brings to the partner portal. 32
/// uppercase hex characters, well inside the modelled 1..2048 length.
///
/// Those characters are all in the base64 alphabet and the count is a whole number of
/// base64 groups, so a minted key is base64 text ([`is_base64_activation_key`]) and the
/// two operations that take the member read it rather than refusing it.
pub fn new_activation_key() -> String {
    uuid::Uuid::new_v4()
        .simple()
        .to_string()
        .to_ascii_uppercase()
}

/// The bounds the model puts on `BillingTier` (`@range(min: 0, max: 8)`), which is
/// every value the member may ever carry.
pub const BILLING_TIER_RANGE: (i32, i32) = (0, 8);

/// Project a stored `Connection` into the `ConnectionSummary` the list operation
/// returns, so both views report one record rather than two.
///
/// `billingTier` is carried across like any other stored member: the tier a
/// connection is assigned belongs to the record, so the summary view reports the
/// same value the full view does rather than deciding one of its own.
pub fn summary_of(connection: &Connection) -> ConnectionSummary {
    ConnectionSummary {
        id: connection.id.clone(),
        arn: connection.arn.clone(),
        description: connection.description.clone(),
        bandwidth: connection.bandwidth.clone(),
        attach_point: connection.attach_point.clone(),
        environment_id: connection.environment_id.clone(),
        provider: connection.provider.clone(),
        location: connection.location.clone(),
        r#type: connection.r#type.clone(),
        state: connection.state.clone(),
        shared_id: connection.shared_id.clone(),
        billing_tier: connection.billing_tier,
    }
}

/// The envelope this service puts on the client-facing refusals it words itself.
///
/// Every recorded Interconnect rejection message starts with it ("Unable to process
/// request: This environment model with id … could not be found", "Unable to process
/// request: Connection does not exist for …"), so the refusals below are built from
/// this one constant rather than each spelling the prefix out.
pub const REFUSAL_ENVELOPE: &str = "Unable to process request: ";

/// How the service words an `environmentId` (or `GetEnvironment$id`) that resolves to
/// no Environment.
///
/// An unresolvable environment is a CLIENT fault for this service — the caller named
/// a model that does not exist — so it is reported as `InterconnectClientException`
/// (HTTP 400), not as the connection-scoped `ResourceNotFoundException` (404), and the
/// message is built from the id the request carried.
pub fn environment_not_found_message(environment_id: &str) -> String {
    format!("{REFUSAL_ENVELOPE}This environment model with id {environment_id} could not be found")
}

/// How the service words a create against an Environment that is not taking new
/// connections, naming the Environment and the state it is in.
///
/// The sibling of [`environment_not_found_message`] — the Environment resolved, but
/// its own state says no connection can be requested across it — so it is worded the
/// same way: a CLIENT fault carrying this service's [`REFUSAL_ENVELOPE`].
pub fn environment_unavailable_message(environment_id: &str, state: &str) -> String {
    format!(
        "{REFUSAL_ENVELOPE}environment '{environment_id}' is {state} and cannot take new connections"
    )
}

/// How `CreateConnection` words a `clientToken` that already created a connection but
/// arrived with different parameters, naming the token and the connection it created.
///
/// A fault in the REQUEST — the token names a connection other than the one this call
/// describes — so it carries this service's [`REFUSAL_ENVELOPE`] like every other
/// client-fault refusal this service words.
pub fn client_token_reused_message(client_token: &str, connection_id: &str) -> String {
    format!(
        "{REFUSAL_ENVELOPE}clientToken '{client_token}' was already used to create connection '{connection_id}' with different parameters"
    )
}

/// How `CreateConnection` words the account's connection quota ([`MAX_CONNECTIONS`])
/// being full.
///
/// Entirely FIXED text but for the limit itself, which is read from the one constant
/// the create counts against, so the number the caller is told is the number that
/// refused it. It carries this service's [`REFUSAL_ENVELOPE`] like the service's other
/// client-facing refusals.
pub fn connection_quota_exceeded_message() -> String {
    format!("{REFUSAL_ENVELOPE}the account already holds the maximum of {MAX_CONNECTIONS} connections")
}

/// How a connection READ (`GetConnection`, `DeleteConnection`) words an identifier
/// that names no stored connection: the retrieval could not be served.
pub fn connection_not_retrievable_message(identifier: &str) -> String {
    format!("{REFUSAL_ENVELOPE}Unable to retrieve connection {identifier}, as it does not exist")
}

/// How an ARN-ADDRESSED operation words an `arn` that resolves to no resource of this
/// account.
///
/// A distinct construction from [`connection_not_retrievable_message`], which is what
/// an identifier-addressed read words: this sentence is entirely FIXED — it names
/// neither the ARN, nor the connection id, nor the resource type — so a refusal on an
/// ARN the caller supplied reproduces nothing of it, and it says in one breath both
/// ways a resource can be absent (never created, or deleted since).
pub fn arn_resource_absent_message() -> String {
    format!("{REFUSAL_ENVELOPE}Resource does not exist or is deleted")
}

/// How `DescribeConnectionProposal` words an `activationKey` that resolves to no
/// stored proposal.
///
/// `ActivationKey` is `@sensitive`, so — unlike the identifier-bearing refusals
/// above — this message names the MEMBER and not the value the request carried: a
/// key echoed into an error body would be a secret in a log.
pub fn proposal_not_retrievable_message() -> String {
    format!(
        "{REFUSAL_ENVELOPE}Unable to retrieve a connection proposal for the supplied activationKey, as it does not exist"
    )
}

/// The coding an `ActivationKey`'s text is written in.
///
/// An activation key is not an opaque handle: it is the partner portal's ENCODED
/// capture of what a creation request chose — "This key captures the desired parameters
/// from the initial creation request" (`DescribeConnectionProposal$activationKey`) — so
/// text that is not base64 carries no capture to read and names no proposal of any
/// account.
///
/// Read with the standard base64 alphabet, the way a general-purpose base64 reader
/// reads it: `=` padding is accepted but not required, and the bits a final partial
/// group leaves over are not held to canonical zeroes, so every text some base64
/// WRITER could have produced is accepted. What is refused is text that encodes
/// nothing at all: a character outside the alphabet, padding in the middle of the text,
/// or a final group of one character, which carries fewer bits than one byte needs.
const ACTIVATION_KEY_ENCODING: ::base64::engine::general_purpose::GeneralPurpose =
    ::base64::engine::general_purpose::GeneralPurpose::new(
        &::base64::alphabet::STANDARD,
        ::base64::engine::general_purpose::GeneralPurposeConfig::new()
            .with_decode_padding_mode(::base64::engine::DecodePaddingMode::Indifferent)
            .with_decode_allow_trailing_bits(true),
    );

/// Whether `activation_key` is base64 text at all ([`ACTIVATION_KEY_ENCODING`]).
///
/// A property of the VALUE the request carried and of nothing else, so the answer does
/// not depend on which proposals this store holds.
pub fn is_base64_activation_key(activation_key: &str) -> bool {
    ::base64::Engine::decode(&ACTIVATION_KEY_ENCODING, activation_key).is_ok()
}

/// Why `activation_key` is not an activation key at all — or `None` when its text is
/// base64, and so could name a proposal.
///
/// The ONE rule both operations that take the member apply to it
/// (`DescribeConnectionProposal`, `AcceptConnectionProposal`), judged on the REQUEST
/// alone and therefore BEFORE the key is resolved against this store: the encoding is
/// what makes the value an activation key, so a value that is not one is a fault in the
/// input rather than a resource the account does not hold, and a caller can tell "the
/// text I pasted is not a key" from "that proposal does not exist".
pub fn activation_key_encoding_violation(activation_key: &str) -> Option<String> {
    (!is_base64_activation_key(activation_key)).then(activation_key_not_base64_message)
}

/// How the service words an `activationKey` whose text is not base64.
///
/// Entirely FIXED text, with no variable part: `ActivationKey` is `@sensitive`, so the
/// refusal reproduces nothing of the value the request carried — not its text, not its
/// length and not where its first bad character was.
pub fn activation_key_not_base64_message() -> String {
    format!("{REFUSAL_ENVELOPE}The provided activation key is not valid base64")
}

/// How `UpdateConnection` words an identifier that names no stored connection.
///
/// A distinct construction from [`connection_not_retrievable_message`]: the update
/// path states that the connection does not exist for the identifier it was asked to
/// modify, which is the wording this operation is recorded answering.
pub fn connection_absent_message(identifier: &str) -> String {
    format!("{REFUSAL_ENVELOPE}Connection does not exist for {identifier}")
}

/// The resource-type segment every `AmazonResourceName` of this service carries:
/// the model's pattern is `^arn:aws[a-z-]*:interconnect:[^:]+:[0-9]{12}:connection/…$`,
/// so a connection is the only resource kind an ARN here can name (which is what the
/// tagging operations mean by "currently this supports Connection resources").
pub const ARN_RESOURCE_TYPE: &str = "connection";

/// How many digits an ARN's account-id field has (`[0-9]{12}` in that pattern).
const ARN_ACCOUNT_ID_DIGITS: usize = 12;

/// The partition every ARN this emulator mints carries.
///
/// `emulator_arn` builds `arn:aws:<service>:<region>:<account>:<resource>`, so `aws`
/// is the partition the requests this emulator serves address, while the member's
/// `aws[a-z-]*` pattern lets a caller SEND any partition.
pub const ARN_PARTITION: &str = "aws";

/// Whether `field` is the account field of an `AmazonResourceName`: the twelve digits
/// an AWS account id has, which is what this service's ARN pattern writes there.
fn is_arn_account_field(field: &str) -> bool {
    field.len() == ARN_ACCOUNT_ID_DIGITS && field.chars().all(|c| c.is_ascii_digit())
}

/// Every field an Interconnect connection ARN carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectionArn<'a> {
    /// The partition field (`aws`, `aws-cn`, …).
    pub partition: &'a str,
    /// The Region field.
    pub region: &'a str,
    /// The twelve-digit account field.
    pub account: &'a str,
    /// The `ConnectionId` the resource field names.
    pub id: &'a str,
}

/// The fields of an `AmazonResourceName`, for the operations that identify a
/// connection by its ARN (`TagResource`, `UntagResource`, `ListTagsForResource`)
/// rather than by its id.
///
/// Parsed from the ARN's six colon-separated fields — `arn`, a partition the
/// pattern writes `aws[a-z-]*`, this service's [`ARN_NAMESPACE`], a region, a
/// twelve-digit account and `connection/<id>` — so anything that is not an
/// Interconnect connection ARN resolves to `None` rather than to some other
/// service's resource. The id itself is NOT re-checked against
/// `^(mcc|lmcc)-[a-z0-9]{8}$` here: an id outside it simply matches no stored
/// connection.
pub fn parse_connection_arn(arn: &str) -> Option<ConnectionArn<'_>> {
    let (qualifier, id) = arn.rsplit_once('/')?;
    let mut fields = qualifier.split(':');
    if fields.next() != Some("arn") {
        return None;
    }
    let partition = fields.next().filter(|partition| {
        partition
            .strip_prefix("aws")
            .is_some_and(|rest| rest.chars().all(|c| c.is_ascii_lowercase() || c == '-'))
    })?;
    if fields.next() != Some(ARN_NAMESPACE) {
        return None;
    }
    let region = fields.next().filter(|region| !region.is_empty())?;
    let account = fields.next().filter(|account| is_arn_account_field(account))?;
    if fields.next() != Some(ARN_RESOURCE_TYPE) || fields.next().is_some() || id.is_empty() {
        return None;
    }
    Some(ConnectionArn {
        partition,
        region,
        account,
        id,
    })
}

/// The `ConnectionId` an `AmazonResourceName` names — [`parse_connection_arn`]'s
/// resource field, for the callers that need only the id.
pub fn connection_id_from_arn(arn: &str) -> Option<&str> {
    parse_connection_arn(arn).map(|parsed| parsed.id)
}

/// How the service words an `arn` that is not an Interconnect connection ARN at
/// all, which is a fault in the request rather than a missing resource.
pub fn not_a_connection_arn_message(arn: &str) -> String {
    format!("{REFUSAL_ENVELOPE}{arn} is not the ARN of an Interconnect {ARN_RESOURCE_TYPE}")
}

/// How the ACCOUNT field of an `AmazonResourceName` is named in the refusal of an ARN
/// that does not address the caller's scope: the twelve-digit account id
/// ([`is_arn_account_field`]) is an "account ID".
const ARN_ACCOUNT_FIELD_LABEL: &str = "account ID";

/// How the PARTITION field is named in that same refusal.
const ARN_PARTITION_FIELD_LABEL: &str = "partition";

/// Why `parsed` does not address the scope the request it arrived on is for — or
/// `None` when its account and partition are both that scope's.
///
/// The scope an ARN addresses is its ACCOUNT and its PARTITION: an ARN carrying
/// either of another scope names a resource the caller is not talking to at all, which
/// is a fault in the REQUEST rather than a resource this scope is missing. The Region
/// segment is NOT part of that judgement — the member's pattern leaves it
/// unconstrained (`[^:]+`), so any spelling of it is a well-formed value of the member
/// and the resource it names is resolved like any other
/// ([`ComAmazonawsInterconnectInterconnectState::connection_in_for_arn`]).
///
/// The refusal is worded the way an ARN-scope mismatch is worded service-wide: the ARN
/// FIELD that differs, the value the ARN carried for it, the value the CALLER is (its
/// own account as `GetCallerIdentity` reports it), and `request_shape` — the name of
/// the input shape the ARN arrived in (`TagResourceRequest`,
/// `ListTagsForResourceRequest`, `UntagResourceRequest`), which is what says WHICH
/// request carried the offending ARN. It carries neither this service's
/// [`REFUSAL_ENVELOPE`] nor the ARN text, so a refusal reproduces nothing of the ARN
/// the caller supplied. The ACCOUNT is judged first, so an ARN of another account is
/// always answered with the account mismatch even when its partition differs too.
pub fn arn_scope_violation(parsed: &ConnectionArn<'_>, request_shape: &str) -> Option<String> {
    let account = ::emulator_server::emulator_account_id();
    let (field, carried, addressed) = [
        (ARN_ACCOUNT_FIELD_LABEL, parsed.account, account),
        (ARN_PARTITION_FIELD_LABEL, parsed.partition, ARN_PARTITION),
    ]
    .into_iter()
    .find(|(_, carried, addressed)| carried != addressed)?;
    Some(format!(
        "ARN {field} ({carried}) does not match caller {field} ({addressed}) for {request_shape}"
    ))
}

/// The shortest and longest a tag KEY may be: `TagKey` is modelled
/// `@length(min: 1, max: 128)`, so an empty key and one past 128 are both outside
/// what a `TagMap` can carry.
pub const TAG_KEY_LENGTH: (usize, usize) = (1, 128);

/// The shortest and longest a tag VALUE may be: `TagValue` is modelled
/// `@length(min: 0, max: 256)`, so a value may be empty but not longer than 256.
pub const TAG_VALUE_LENGTH: (usize, usize) = (0, 256);

/// The tag-key namespace a caller does not own.
///
/// AWS assigns its own system tags keys that begin with this prefix and reserves it:
/// a tagging call may not create, change or remove a key inside it, so the tags a
/// request can touch are only the caller's own. `TagKey` carries no `@pattern`, so
/// the reservation is a service-level rule rather than a modelled constraint, and it
/// is matched without regard to case the way the reservation is stated.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` lies in the reserved [`RESERVED_TAG_KEY_PREFIX`] namespace.
///
/// The head is taken with `get` rather than by slicing, so a key whose opening bytes
/// are part of a multi-byte character answers `false` instead of panicking.
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// Why `keys` are not keys this caller may write or remove, worded for the caller —
/// or `None` when none of them is reserved.
///
/// Every tagging path judges its keys here — tag-on-create, the tagging operation and
/// the untagging one — so the reservation holds however a key reached the request, and
/// a refused call writes nothing. The lowest reserved key decides the message, so the
/// same request is always refused for the same key, and that key and the reserved
/// prefix are what the message names.
pub fn reserved_tag_key_violation<'a>(keys: impl IntoIterator<Item = &'a str>) -> Option<String> {
    keys.into_iter()
        .filter(|key| is_reserved_tag_key(key))
        .min()
        .map(|key| {
            format!(
                "{REFUSAL_ENVELOPE}the tag key {key} is in the {RESERVED_TAG_KEY_PREFIX} namespace that AWS reserves for its own tags, which a caller cannot create, change or remove"
            )
        })
}

/// Why a `TagMap` the request carries does not satisfy the lengths the model gives
/// `TagKey` and `TagValue`, worded for the caller — or `None` when every entry is
/// inside them. The entry-count half of the rule lives in [`tag_write_violation`],
/// which is what the handlers call.
///
/// Entries are walked in key order so the same request is always refused for the
/// same entry, and the message names the member and the bound it broke rather than
/// quoting a value back at the caller.
fn tag_length_violation(tags: &std::collections::HashMap<String, String>) -> Option<String> {
    let mut entries: Vec<(&String, &String)> = tags.iter().collect();
    entries.sort_by(|left, right| left.0.cmp(right.0));
    for (key, value) in entries {
        // `@length` on a string counts characters, not bytes, so a non-ASCII key is
        // measured the way the model measures it.
        let key_length = key.chars().count();
        let (key_min, key_max) = TAG_KEY_LENGTH;
        if !(key_min..=key_max).contains(&key_length) {
            return Some(format!(
                "{REFUSAL_ENVELOPE}a tag key must be between {key_min} and {key_max} characters long, and one of length {key_length} is not"
            ));
        }
        let value_length = value.chars().count();
        let (value_min, value_max) = TAG_VALUE_LENGTH;
        if !(value_min..=value_max).contains(&value_length) {
            return Some(format!(
                "{REFUSAL_ENVELOPE}the tag value for key {key} must be between {value_min} and {value_max} characters long, and one of length {value_length} is not"
            ));
        }
    }
    None
}

/// The most tags ONE resource holds at once.
///
/// A tag map is written from the request, so without a bound on its entry count a
/// caller could keep adding fresh keys to the same connection until the store filled
/// memory — the record-count bounds above ([`MAX_CONNECTIONS`],
/// [`MAX_STORED_RECORDS`]) limit how many resources exist, not how large one gets.
/// The model puts no `@length` on `TagMap`, so the number itself is the one AWS
/// publishes for a resource's tag set; what matters is that there IS one and that
/// exceeding it is refused, with nothing written, rather than growing the store.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// How many tags a resource currently holding `existing` would hold once `incoming`
/// is merged into it.
///
/// Counted off the set the ONE merge every tag writer here applies ([`merge_tags`])
/// would leave, so the bound is judged against exactly what would be stored: a key
/// already present takes the new value instead of adding an entry, and a request
/// carrying no tag leaves the count where it was.
fn merged_tag_count(
    existing: Option<&std::collections::HashMap<String, String>>,
    incoming: &std::collections::HashMap<String, String>,
) -> usize {
    let mut merged = existing.cloned();
    merge_tags(&mut merged, Some(incoming));
    merged.map(|tags| tags.len()).unwrap_or(0)
}

/// Why writing `incoming` onto a resource that currently holds `existing` is not a
/// tag set this service stores, worded for the caller — or `None` when the write is
/// within every bound.
///
/// Both of this service's tag writers (tag-on-create and the tagging operation) judge
/// their input here, so one rule decides what a stored tag set can be however it was
/// reached: every entry inside the lengths the model gives `TagKey` and `TagValue`,
/// no key in the reserved [`RESERVED_TAG_KEY_PREFIX`] namespace, and at most
/// [`MAX_TAGS_PER_RESOURCE`] entries on the resource once the write is
/// merged. A request outside either bound is a fault in the INPUT — the 400 the
/// service models for one, never a missing resource and never a 501 — and is refused
/// before anything is written, so a refused write leaves the resource exactly as it
/// was. The length check is needed because the boundary validator holds a LIST's
/// members to their target's constraints but not a MAP's keys.
pub fn tag_write_violation(
    existing: Option<&std::collections::HashMap<String, String>>,
    incoming: &std::collections::HashMap<String, String>,
) -> Option<String> {
    if let Some(message) = tag_length_violation(incoming) {
        return Some(message);
    }
    if let Some(message) = reserved_tag_key_violation(incoming.keys().map(String::as_str)) {
        return Some(message);
    }
    let merged = merged_tag_count(existing, incoming);
    if merged > MAX_TAGS_PER_RESOURCE {
        return Some(format!(
            "{REFUSAL_ENVELOPE}a {ARN_RESOURCE_TYPE} holds at most {MAX_TAGS_PER_RESOURCE} tags, and this request would leave it with {merged}"
        ));
    }
    None
}

/// A resource's tag set as this service HOLDS it: the entries it carries, or `None`
/// for a resource carrying none.
pub type StoredTagSet = Option<std::collections::HashMap<String, String>>;

/// How a tag set a request supplied is HELD: a set with entries is stored as it is,
/// and a set with NO entries is stored as `None`.
///
/// "No tags" therefore has exactly ONE representation in the store — `None`, which is
/// how a resource created without a `tags` member is held and what [`untag_tags`]
/// leaves once the last key goes — so a tag map a request carries with no entries in
/// it is the same input as no `tags` member at all, and every later read reports one
/// wire form for that one state however it was reached. Every writer of a tag set
/// normalises here, so no path can store the second, empty-map form.
pub fn stored_tag_set(
    supplied: Option<&std::collections::HashMap<String, String>>,
) -> StoredTagSet {
    supplied.filter(|tags| !tags.is_empty()).cloned()
}

/// Merge `supplied` into the tag set a resource `held`: each key the request carries
/// is written over whatever the resource held for it, and every key the request does
/// not name keeps the value it had.
///
/// Merge, never replace — so a request carrying no tag at all adds nothing — and the
/// result is left in the one representation [`stored_tag_set`] defines, so a merge
/// that writes nothing cannot turn "no tags" into an empty map.
pub fn merge_tags(
    held: &mut StoredTagSet,
    supplied: Option<&std::collections::HashMap<String, String>>,
) {
    let Some(supplied) = stored_tag_set(supplied) else {
        return;
    };
    held.get_or_insert_with(std::collections::HashMap::new)
        .extend(supplied);
}

/// Remove `keys` from the tag set a resource `held`: only the listed keys go, and the
/// rest of the map is untouched.
///
/// The result is left in the one representation [`stored_tag_set`] defines, so a
/// resource whose last key has just been removed holds "no tags" the same way one
/// created without any does.
pub fn untag_tags(held: &mut StoredTagSet, keys: &[String]) {
    if let Some(tags) = held.as_mut() {
        for key in keys {
            tags.remove(key);
        }
        if tags.is_empty() {
            *held = None;
        }
    }
}

/// The terminal `ConnectionState` a completed `DeleteConnection` settles into.
pub const DELETED_CONNECTION_STATE: &str = "deleted";

/// The transient `ConnectionState` a `DeleteConnection` hands the connection to the
/// clock in ("deleting: The connection is being deleted").
pub const DELETING_CONNECTION_STATE: &str = "deleting";

/// Whether a `DeleteConnection` has been made on a connection in `state`: it is
/// either being torn down or has been.
///
/// Either way "the network connectivity provided by the Connection will cease to
/// exist", so a handle whose whole subject is that connectivity names nothing from
/// the moment the delete is called — while the record itself stays readable by its
/// identifier, which is how a caller polling the delete sees the teardown.
pub fn is_deleted_or_deleting(state: &str) -> bool {
    state == DELETING_CONNECTION_STATE || state == DELETED_CONNECTION_STATE
}

/// Whether `stored` is still a resource of this account: a connection that has reached
/// the terminal `deleted` state is gone, while the record itself stays readable by its
/// identifier so a caller polling a delete sees that terminal state.
fn exists_as_resource(stored: &StoredConnection) -> bool {
    stored.connection.state != DELETED_CONNECTION_STATE
}

/// Whether `stored` is the live resource `arn` names: the stored ARN matches field
/// for field, and the connection has not reached the terminal deleted state.
fn holds_arn(stored: &StoredConnection, arn: &str) -> bool {
    stored.connection.arn == arn && exists_as_resource(stored)
}

/// The `ConnectionState` a new connection is returned in ("requested: The initial
/// state of a connection. The state will remain here until the Connection is accepted
/// on the Partner portal"), which is also the first leg of the progression the clock
/// carries it along.
///
/// It is not what says whether an acceptance can still be made: the clock carries a
/// connection out of this state by itself, so whether the proposal has been accepted
/// is [`StoredConnection::proposal_accepted`]'s to say and which states have no
/// acceptance left to make is [`acceptance_possible_in`]'s.
pub const AWAITING_ACCEPTANCE_CONNECTION_STATE: &str = "requested";

/// The `ConnectionState` an accepted proposal's connection enters ("pending: The
/// connection has been accepted and is being provisioned between AWS and the
/// Partner"), which is the first leg of the progression the clock then owes it.
pub const ACCEPTED_CONNECTION_STATE: &str = "pending";

/// The one `ConnectionState` in which a connection IS the connectivity something can be
/// attached to.
///
/// The model documents the value as "available: The connection has been fully
/// provisioned between AWS and the Partner". Every other modelled state is a circuit
/// that is not there to attach anything to: one still waiting at the partner portal
/// ("requested: … the state will remain here until the Connection is accepted on the
/// Partner portal"), one still being provisioned (`pending`), one being re-provisioned
/// (`updating`), one that is not passing traffic (`down`), one that never came up
/// ("failed: the connection has failed to be created") and one being torn down or
/// already torn down (`deleting`, `deleted`).
pub const CONNECTION_READY_STATE: &str = "available";

/// Whether a connection in `state` provides connectivity a new Connection can be
/// provisioned onto.
///
/// The ONE readiness rule the whole `AttachPoint` union is held to, stated for this
/// member's resource: an attach point is a handle whose subject is connectivity, so it
/// names something only while the resource behind it is in the state its own service
/// documents as ready for use — [`CONNECTION_READY_STATE`] for a connection of this
/// service, and the equivalent gateway state for the union's other member
/// ([`direct_connect`]).
pub fn provides_connectivity(state: &str) -> bool {
    state == CONNECTION_READY_STATE
}

/// The `ConnectionState` a connection whose bandwidth is being re-provisioned
/// reports ("updating: the connection is being updated"), which is the transient
/// state `UpdateConnection`'s own change hands the connection to the clock in.
pub const UPDATING_CONNECTION_STATE: &str = "updating";

/// The `ConnectionState` values a connection that has been carried past the state it
/// was created in, and whose circuit has not since been torn down or failed, reports:
/// the accepted state itself, the fully provisioned one, the one a provisioned circuit
/// that is not passing traffic reports, and the one an in-flight change reports.
pub const ACCEPTED_CONNECTION_STATES: [&str; 4] = [
    ACCEPTED_CONNECTION_STATE,
    CONNECTION_READY_STATE,
    "down",
    UPDATING_CONNECTION_STATE,
];

/// Whether `state` is one a connection reports once it has been carried past the state
/// it was created in.
pub fn is_accepted_state(state: &str) -> bool {
    ACCEPTED_CONNECTION_STATES.contains(&state)
}

/// The terminal `ConnectionState` a connection that never came up reports ("failed:
/// The connection has failed to be created").
pub const FAILED_CONNECTION_STATE: &str = "failed";

/// Whether a connection in `state` still has an acceptance to make on it.
///
/// A connection a `DeleteConnection` has been made on is losing, or has lost, the very
/// connectivity an acceptance brings up, and one whose creation failed never had it, so
/// on those states there is nothing left to accept. Every other modelled state is a
/// connection that exists and that an acceptance can therefore be made on — whether it
/// already HAS been is [`StoredConnection::proposal_accepted`]'s to say, not the
/// state's, because the clock carries a connection along the provisioning progression
/// by itself.
pub fn acceptance_possible_in(state: &str) -> bool {
    !is_deleted_or_deleting(state) && state != FAILED_CONNECTION_STATE
}

/// Accept a connection's proposal: the ONE writer of everything an acceptance decides.
///
/// "Upon accepting the proposal a connection will be made between the AWS network as
/// accessed via the selected Attach Point and the network previously selected network
/// on the partner's portal" — so the selected attach point is what the connection is
/// associated with from now on, a `description` the request supplies distinguishes the
/// resulting Connection (one it omits leaves the proposal's own alone), and the tags it
/// carries are MERGED onto the ones the connection holds, the way every tag writer of
/// this service writes them. The Environment and the bandwidth were chosen on the
/// partner's portal and "cannot be modified", so nothing here touches them, nor any
/// other server-owned member of the record.
///
/// Making the connection onto the selected attach point is provisioning work, so the
/// connection enters the accepted state ("pending: the connection has been accepted and
/// is being provisioned between AWS and the Partner") and is handed the rest of the
/// documented progression ([`owed_progression`]) from `now` — whatever state the clock
/// had carried it to beforehand, exactly as a re-provisioning bandwidth change hands a
/// connection back to the clock. The due instant is stored and materialised by
/// whichever call comes next; nothing ever waits for it.
pub fn apply_acceptance(
    stored: &mut StoredConnection,
    attach_point: &AttachPoint,
    description: Option<&String>,
    tags: Option<&std::collections::HashMap<String, String>>,
    now: Instant,
) {
    stored.connection.attach_point = attach_point.clone();
    if let Some(description) = description {
        stored.connection.description = description.clone();
    }
    // The tags the acceptance carries are written by the one rule every writer of a
    // tag set here follows ([`merge_tags`]): merge, never replace, and "no tags" keeps
    // its single representation in the store when the request carries none.
    merge_tags(&mut stored.connection.tags, tags);
    stored.proposal_accepted = true;
    stored.connection.state = ACCEPTED_CONNECTION_STATE.to_string();
    stored.pending_transition = owed_progression(ACCEPTED_CONNECTION_STATE, now);
}

/// Apply a new bandwidth to a connection: the ONE writer of what a re-provisioning
/// change does to the record.
///
/// "updating: the connection is being updated" — re-provisioning the circuit is not
/// instantaneous, so the connection reports that transient state and the clock settles
/// it back into the state it was IN when the change was applied, which is the state the
/// change found it in and not the one the clock had still to carry it to. A bandwidth
/// change re-provisions a circuit and decides nothing else: only an acceptance enters
/// the accepted state ("pending: the connection has been accepted…"), so a connection
/// whose proposal is still awaiting one comes back awaiting it, and whatever the
/// restored state owes ([`owed_progression`]) is re-armed from the instant it is
/// restored. The due instant is stored and materialised by whichever call comes next;
/// nothing ever waits for it.
pub fn apply_bandwidth_change(stored: &mut StoredConnection, bandwidth: String, now: Instant) {
    let came_from = stored.connection.state.clone();
    stored.connection.bandwidth = bandwidth;
    stored.connection.state = UPDATING_CONNECTION_STATE.to_string();
    stored.pending_transition = Some(PendingTransition {
        due: now + PROVISIONING_SPAN,
        state: came_from,
    });
}

/// Whether accepting `connection`'s proposal with `attach_point`, `description` and
/// `tags` would leave the connection exactly as it already is.
///
/// What makes a repeated acceptance the SAME acceptance: the attach point it asks the
/// connection to be associated with is the one the connection holds, and every member
/// it supplies is already the connection's. One that asks for anything else is a
/// different acceptance of a proposal that has already been accepted, so it is
/// refused instead of re-associating a connection that is already provisioning.
pub fn acceptance_already_applied(
    connection: &Connection,
    attach_point: &AttachPoint,
    description: Option<&String>,
    tags: Option<&std::collections::HashMap<String, String>>,
) -> bool {
    attach_point_eq(&connection.attach_point, attach_point)
        && description.is_none_or(|supplied| *supplied == connection.description)
        && tags.is_none_or(|supplied| {
            supplied.iter().all(|(key, value)| {
                connection
                    .tags
                    .as_ref()
                    .and_then(|held| held.get(key))
                    .is_some_and(|held| held == value)
            })
        })
}

/// How the service words an acceptance of a proposal whose connection has already
/// been accepted and asks for something that connection does not hold.
///
/// `ActivationKey` is `@sensitive`, so the message names the MEMBER the request
/// carried the key in and the connection the acceptance already made, never the key.
pub fn proposal_already_accepted_message(connection_id: &str) -> String {
    format!(
        "{REFUSAL_ENVELOPE}The connection proposal for the supplied activationKey has already \
         been accepted as connection {connection_id}, whose attach point cannot be changed"
    )
}

/// How the service words an acceptance of a proposal whose connection is no longer
/// one an acceptance can be made on — it is being deleted, deleted, or failed.
///
/// Built from the store rather than from the request: the state the connection is in
/// is what refuses the call, so the message names it, and the sensitive
/// `activationKey` is named only as a member.
pub fn proposal_not_acceptable_message(connection_id: &str, state: &str) -> String {
    format!(
        "{REFUSAL_ENVELOPE}The connection proposal for the supplied activationKey names connection \
         {connection_id}, which is {state}, so it can no longer be accepted"
    )
}

/// The `ConnectionState` values an `UpdateConnection` is permitted FROM.
///
/// The service states the rule in its own refusal ("only connections in 'requested'
/// or 'available' state can be updated"), so it is an allowlist: every other modelled
/// state — `pending`, `down`, `deleting`, `deleted`, `failed`, `updating` — is a
/// connection whose circuit is mid-flight or gone, and an update that asks it for a
/// CHANGE is refused. A request that asks for no change is judged by
/// [`update_permitted_from`], which is where the whole rule lives.
pub const UPDATABLE_CONNECTION_STATES: [&str; 2] = ["requested", "available"];

/// Whether a connection in `state` may be updated at all.
pub fn is_updatable_state(state: &str) -> bool {
    UPDATABLE_CONNECTION_STATES.contains(&state)
}

/// Whether applying `description` and `bandwidth` to `connection` would leave it
/// exactly as it already is.
///
/// What makes a repeated update the SAME update: every member it supplies already
/// holds the value it asks for, so the call has nothing left to change. One that asks
/// for anything else is a different update. The comparison is against the STORED
/// record rather than against the request that was first made, because an update
/// allocates no resource of its own — the record is the whole outcome of the call that
/// applied it, so a request the record already satisfies is that call's own answer.
pub fn update_already_applied(
    connection: &Connection,
    description: Option<&String>,
    bandwidth: Option<&String>,
) -> bool {
    description.is_none_or(|supplied| *supplied == connection.description)
        && bandwidth.is_none_or(|supplied| *supplied == connection.bandwidth)
}

/// Whether a connection in `state` may take an update that `asks_for_change`.
///
/// A request that asks for a CHANGE is held to the states the service permits an
/// update from ([`UPDATABLE_CONNECTION_STATES`]): a circuit that is mid-flight,
/// impaired or gone has nothing a description or bandwidth change can be applied to.
///
/// A request that asks for NO change — every member it supplies already holds the
/// value it asks for ([`update_already_applied`]) — changes nothing, so there is
/// nothing for that precondition to protect: it is answered with the record the call
/// that applied it already produced, which is what the `@idempotent` trait asks for
/// when a client or an SDK repeats a request that has already succeeded. In
/// particular the transient state this operation's own change hands the connection to
/// the clock in is one such state, so the repeat of a bandwidth change is a no-op
/// rather than a refusal. A connection that is being deleted, deleted or failed is
/// gone, and an update reports nothing on it at all.
pub fn update_permitted_from(state: &str, asks_for_change: bool) -> bool {
    if asks_for_change {
        return is_updatable_state(state);
    }
    is_updatable_state(state) || is_accepted_state(state)
}

/// How `UpdateConnection` words a refusal on a connection whose state does not permit
/// an update: the identifier from the request plus the permitted-state rule itself,
/// rendered from [`UPDATABLE_CONNECTION_STATES`] so the message cannot drift from the
/// guard that produced it.
pub fn connection_not_updatable_message(identifier: &str) -> String {
    let permitted: Vec<String> = UPDATABLE_CONNECTION_STATES
        .iter()
        .map(|state| format!("'{state}'"))
        .collect();
    format!(
        "{REFUSAL_ENVELOPE}Cannot update connection {identifier}: only connections in {} state can be updated",
        permitted.join(" or ")
    )
}

/// The tag `ListEnvironments` stamps on the cursors it issues.
pub const ENVIRONMENT_CURSOR_TAG: &str = "env:";
/// The tag `ListConnections` stamps on the cursors it issues.
pub const CONNECTION_CURSOR_TAG: &str = "conn:";
/// The tag `ListAttachPoints` stamps on the cursors it issues.
pub const ATTACH_POINT_CURSOR_TAG: &str = "ap:";

/// Separator between the fields of the payload a cursor authenticates: the scope of the
/// listing that issued it, then the key the page it ended ran to.
///
/// A unit separator appears in none of those fields — a Region id, an `environmentId`, a
/// `ConnectionId` and a gateway identifier are all written in identifier characters — so
/// the payload is read back field for field exactly as it was minted, and a key carrying
/// one is refused rather than split.
const CURSOR_FIELD_SEPARATOR: char = '\u{1f}';

/// The scope a `ListConnections` cursor is issued under: this operation's tag and the
/// regional endpoint that served the page.
///
/// A cursor names the last key of ONE walked sequence, and the sequence this listing
/// walks is the connections of the Region the request was signed for, so a cursor issued
/// by another Region's endpoint names a position in a sequence this one never walked.
pub fn connection_cursor_scope(region: &str) -> String {
    format!("{CONNECTION_CURSOR_TAG}{region}")
}

/// The scope a `ListEnvironments` cursor is issued under: this operation's tag and the
/// regional endpoint that served the page, whose catalog is the sequence it walked.
pub fn environment_cursor_scope(region: &str) -> String {
    format!("{ENVIRONMENT_CURSOR_TAG}{region}")
}

/// The scope a `ListAttachPoints` cursor is issued under: this operation's tag, the
/// regional endpoint that served the page, and the Environment the listing was scoped
/// by — the attach points of one Environment are the sequence it walked, so a cursor
/// from a listing of another Environment is not one this listing issued.
pub fn attach_point_cursor_scope(region: &str, environment_id: &str) -> String {
    format!("{ATTACH_POINT_CURSOR_TAG}{region}{CURSOR_FIELD_SEPARATOR}{environment_id}")
}

/// Render the key a page ended on as the opaque `nextToken` that listing hands back: the
/// key and its `scope` behind the authenticator THIS store holds the secret for
/// ([`PageTokenKey::mint`]), in hex.
///
/// The position travels inside the token — nothing is kept on the side to look up — so
/// what stops a caller naming a position this listing never offered is that it cannot
/// produce the authenticator over it.
pub fn encode_cursor(token_key: &PageTokenKey, scope: &str, last_key: &str) -> String {
    ::hex::encode(token_key.mint(&format!(
        "{scope}{CURSOR_FIELD_SEPARATOR}{last_key}"
    )))
}

/// Read the key out of a page cursor the listing of `scope` issued.
///
/// `None` for every other value, which the handler refuses with the modelled validation
/// error rather than silently answering with an empty page or restarting the walk: a
/// token that is not hex, one too short to carry an authenticator, one whose
/// authenticator THIS store's secret does not verify (a hand-made token included,
/// however exactly it has an issued token's shape), one minted under another scope — a
/// sibling listing's, another Region's endpoint's, another Environment's — and one whose
/// key is empty or carries the field separator.
///
/// A token naming a key the sequence no longer holds IS one this listing issued, and
/// resumes after that key: a record deleted between two pages must not break the page
/// after it.
pub fn decode_cursor(token_key: &PageTokenKey, token: &str, scope: &str) -> Option<String> {
    let payload = token_key.cursor_of(&::hex::decode(token).ok()?)?;
    let key = payload.strip_prefix(&format!("{scope}{CURSOR_FIELD_SEPARATOR}"))?;
    if key.is_empty() || key.contains(CURSOR_FIELD_SEPARATOR) {
        return None;
    }
    Some(key.to_string())
}

/// How a list operation words a `nextToken` it did not issue, naming the operation
/// whose cursors it would have had to be one of.
///
/// The ONE wording every paginated operation of this service refuses an unknown cursor
/// with ([`decode_cursor`] answering `None`), so the three listings cannot word the
/// same fault differently. It is a fault in the request's own value, so it carries this
/// service's [`REFUSAL_ENVELOPE`], and it reproduces nothing of the token the caller
/// sent.
pub fn cursor_not_issued_message(operation: &str) -> String {
    format!("{REFUSAL_ENVELOPE}nextToken is not a pagination token issued by {operation}")
}

/// Cut one page out of an ordered, already-filtered sequence of keyed items.
///
/// `items` must be in ascending key order (both stores are `BTreeMap`s, so
/// iteration already is). The cursor is the key of the last item handed out, and
/// the next page starts at the first key strictly greater than it — so a page is
/// never repeated or skipped even if the record a token names was deleted or
/// evicted by then. The returned token is `None` exactly when the page ran to the
/// end of the sequence; when there is one, it is minted under `scope` and this
/// store's secret ([`encode_cursor`]), so the next call can tell a cursor this
/// listing issued from one it did not.
pub fn paginate<T>(
    items: Vec<(String, T)>,
    cursor: Option<&str>,
    max_results: Option<i32>,
    token_key: &PageTokenKey,
    scope: &str,
) -> (Vec<T>, Option<String>) {
    let start = match cursor {
        Some(key_after) => items
            .iter()
            .position(|(key, _)| key.as_str() > key_after)
            .unwrap_or(items.len()),
        None => 0,
    };
    // Clamped at both ends on the server: `MaxResults` is modelled
    // `@range(min: 1, max: 100)`, and the page a caller gets is bounded here too
    // rather than only by the boundary validator.
    let limit = max_results.map_or(DEFAULT_PAGE_SIZE, |max| {
        max.clamp(1, DEFAULT_PAGE_SIZE as i32) as usize
    });

    let mut rest = items.into_iter().skip(start);
    let mut page = Vec::new();
    let mut last_key = None;
    for (key, item) in rest.by_ref().take(limit) {
        last_key = Some(key);
        page.push(item);
    }
    let token = if rest.next().is_some() {
        last_key.map(|key| encode_cursor(token_key, scope, &key))
    } else {
        None
    };
    (page, token)
}

/// The code the interconnection facility of `region` is named by, which is the city it
/// sits in: `pdx` (Portland) for the Oregon Region, `iad` (Ashburn) for Northern
/// Virginia.
///
/// These are the city codes AWS names the facilities of a Region by — the same codes
/// its Direct Connect locations and edge sites there carry — and an Environment's id
/// embeds the one of the facility that publishes it (the catalog of the Oregon Region
/// publishes `mce-aws-gcp-pdx`). A Region the table does not name is named by its Region
/// id instead, so an id is tied to exactly ONE Region either way.
const REGION_FACILITY_CODES: [(&str, &str); 29] = [
    ("af-south-1", "cpt"),
    ("ap-east-1", "hkg"),
    ("ap-northeast-1", "nrt"),
    ("ap-northeast-2", "icn"),
    ("ap-northeast-3", "kix"),
    ("ap-south-1", "bom"),
    ("ap-south-2", "hyd"),
    ("ap-southeast-1", "sin"),
    ("ap-southeast-2", "syd"),
    ("ap-southeast-3", "cgk"),
    ("ap-southeast-4", "mel"),
    ("ca-central-1", "yul"),
    ("ca-west-1", "yyc"),
    ("eu-central-1", "fra"),
    ("eu-central-2", "zrh"),
    ("eu-north-1", "arn"),
    ("eu-south-1", "mxp"),
    ("eu-south-2", "zaz"),
    ("eu-west-1", "dub"),
    ("eu-west-2", "lhr"),
    ("eu-west-3", "cdg"),
    ("il-central-1", "tlv"),
    ("me-central-1", "dxb"),
    ("me-south-1", "bah"),
    ("sa-east-1", "gru"),
    ("us-east-1", "iad"),
    ("us-east-2", "cmh"),
    ("us-west-1", "sfo"),
    ("us-west-2", "pdx"),
];

/// The facility code of `region`, or the Region id itself when the table names no code
/// for it.
fn region_facility_code(region: &str) -> &str {
    REGION_FACILITY_CODES
        .iter()
        .find(|(named, _)| *named == region)
        .map_or(region, |(_, code)| *code)
}

/// The `environmentId` the Environment of the `partner` offering carries on the
/// regional endpoint of `region`.
///
/// The published form is `mce-aws-<partner>-<facility>`: a Multicloud Environment
/// between AWS and that partner, at the facility the connections it produces land at.
/// The model's own AcceptConnectionProposal / CreateConnection examples spell an
/// `environmentId` that way (`mce-aws-acme-1`), and so does the catalog the service
/// publishes.
///
/// The facility — and so the Region — is part of the identifier because an Environment
/// belongs to one: the same partner offering reached through two regional endpoints is
/// two Environments, each producing connections that land in its own Region, so no
/// single id can name both. Well inside the `@length(min: 1, max: 64)` the model gives
/// `EnvironmentId` for every Region id AWS publishes.
fn environment_id_in(partner: &str, region: &str) -> String {
    format!("mce-aws-{partner}-{}", region_facility_code(region))
}

/// The partner the service publishes its cloud-service-provider offering with, spelled
/// as the catalog spells it: the partner's own lower-case token, which is what
/// `Provider$cloudServiceProvider` carries and what the `environmentId` embeds.
const MULTICLOUD_PARTNER: &str = "gcp";

/// The offering's `location`: "the provider specific location on the remote side" of
/// the connection, in the PARTNER's own region naming rather than an AWS Region id.
///
/// It belongs to the OFFERING, not to the Region the offering is asked for in: the
/// remote side of this interconnection is one partner region, and the same offering
/// published at a second facility is still terminated there, so it reads the same on
/// every endpoint. It is deliberately NOT derived per Region the way the facility of the
/// `environmentId` is: which partner region an offering terminates in is published by
/// the service, and deriving a different one per Region would be inventing partner
/// geography the service does not publish — the very thing this catalog must not do.
const MULTICLOUD_REMOTE_LOCATION: &str = "us-west1";

/// The offering's `ProductType`. `ProductType` is an unconstrained string, and
/// `"Multicloud"` is the `type` the model's own AcceptConnectionProposal /
/// CreateConnection examples show for a `cloudServiceProvider` environment.
const MULTICLOUD_PRODUCT_TYPE: &str = "Multicloud";

/// The offering's `EnvironmentState`: `limited`, documented as "available, but overall
/// capacity is limited", which is what an offering whose `available` bandwidths are a
/// strict subset of its `supported` ones is in.
const LIMITED_ENVIRONMENT_STATE: &str = "limited";

/// The `RemoteAccountIdentifierType` the offering declares — "the type of identifying
/// information that should be supplied to the `remoteAccount` parameter of a
/// `CreateConnection` call for this specific Environment" — one of the two values
/// `RemoteAccountIdentifierType` models (`account`, `email`).
const MULTICLOUD_REMOTE_IDENTIFIER_TYPE: &str = "account";

/// The bandwidths the offering CURRENTLY offers ("the list of currently available
/// bandwidths"), in the order the catalog publishes them.
const MULTICLOUD_AVAILABLE_BANDWIDTHS: [&str; 7] = [
    "1Gbps", "2Gbps", "5Gbps", "10Gbps", "20Gbps", "50Gbps", "500Mbps",
];

/// The bandwidths the offering supports ("the list of bandwidths supported by this
/// environment"), in the order the catalog publishes them: the available ones plus the
/// size it supports without currently offering it, which is what `limited` means.
const MULTICLOUD_SUPPORTED_BANDWIDTHS: [&str; 8] = [
    "500Mbps", "1Gbps", "2Gbps", "5Gbps", "10Gbps", "20Gbps", "50Gbps", "100Gbps",
];

/// The Environment catalog the regional endpoint of `region` serves.
///
/// `ListEnvironments` lists "all of the environments that can produce connections that
/// will land in the called AWS region", so the catalog is a function OF that Region and
/// every entry in it belongs to that Region alone. Every operation that resolves an
/// `environmentId` resolves it here, for the Region it was called in
/// ([`environment_in_region`]), so what a Region's listing offers is exactly what a
/// create through that Region's endpoint accepts, and the Region a connection's ARN
/// carries is always the Region its Environment produces connections in.
///
/// Nothing is stored: Environments are AWS-managed (the model binds no
/// `CreateEnvironment`), so a catalog derived per call is the same catalog on every
/// call, whoever asks and whenever.
///
/// AWS manages Environments itself and the model publishes no catalog contents, so
/// WHICH partner offering a Region's endpoint presents is the SERVICE's to say, not
/// this emulator's to invent: the entry below is the offering the service publishes,
/// with the partner token, the remote-side location naming and the two bandwidth lists
/// it publishes them with, and without the members it leaves out (`activationPageUrl`).
/// The catalog is the same for every caller in a Region, which is what makes the one
/// published offering of its facility the whole of it.
pub fn environment_catalog(region: &str) -> BTreeMap<String, Environment> {
    let catalog = vec![Environment {
        provider: Provider::cloudServiceProvider(MULTICLOUD_PARTNER.to_string()),
        location: MULTICLOUD_REMOTE_LOCATION.to_string(),
        environment_id: environment_id_in(MULTICLOUD_PARTNER, region),
        state: LIMITED_ENVIRONMENT_STATE.to_string(),
        bandwidths: Bandwidths {
            available: Some(
                MULTICLOUD_AVAILABLE_BANDWIDTHS
                    .iter()
                    .map(|bandwidth| (*bandwidth).to_string())
                    .collect(),
            ),
            supported: Some(
                MULTICLOUD_SUPPORTED_BANDWIDTHS
                    .iter()
                    .map(|bandwidth| (*bandwidth).to_string())
                    .collect(),
            ),
        },
        r#type: MULTICLOUD_PRODUCT_TYPE.to_string(),
        activation_page_url: None,
        remote_identifier_type: Some(MULTICLOUD_REMOTE_IDENTIFIER_TYPE.to_string()),
    }];
    catalog
        .into_iter()
        .map(|environment| (environment.environment_id.clone(), environment))
        .collect()
}

/// The Environment `environment_id` names in the catalog of `region`.
///
/// `None` when that Region's catalog holds no such Environment — the answer every
/// environment-resolving operation gives for an id out of ANOTHER Region's catalog,
/// which is the same answer an id no catalog holds at all gets: an Environment produces
/// connections that land in its own Region, so it is not one this endpoint can place a
/// connection on.
pub fn environment_in_region(region: &str, environment_id: &str) -> Option<Environment> {
    environment_catalog(region).remove(environment_id)
}

/// The `AttachPointType` every descriptor carries.
///
/// `AttachPointType` is an enum whose ONLY modelled value is
/// `DirectConnectGateway`, so it is the only type a descriptor can report even
/// though `AttachPointDescriptor$type`'s prose also mentions an ARN form.
pub const ATTACH_POINT_TYPE_DIRECT_CONNECT_GATEWAY: &str = "DirectConnectGateway";
