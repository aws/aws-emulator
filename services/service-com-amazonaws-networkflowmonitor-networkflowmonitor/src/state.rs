// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-networkflowmonitor-networkflowmonitor service.
//!
//! The records here are the ONLY place a Network Flow Monitor scope, monitor or
//! top-contributors query exists: every implemented handler in `src/ops/` mints
//! into this store or reads out of it, so a create is observable by the next
//! get/list, a delete by the next reader, and a started query by the next status
//! poll.
//!
//! Two things this slice deliberately does NOT emulate, so a reader does not
//! mistake a modelled error for reachable behaviour: PRINCIPAL authorization (the
//! emulator holds no principals to evaluate, so no handler decides what a caller
//! may do) and throttling (`ThrottlingException`, likewise). The emulator verifies
//! request signatures in the framework, before any handler runs.
//!
//! `AccessDeniedException` is still reachable, for the one condition that does not
//! need a principal: an operation that models no `ResourceNotFoundException` cannot
//! report a reference to a scope or a monitor the store does not hold as an absence,
//! and a reference it cannot resolve it also cannot authorize, so that is the
//! modelled 403 ([`reference_denied`]).

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AccessDeniedException, ConflictException, CreateMonitorInput, CreateScopeInput, FilterParameter,
    InternalServerException, MonitorLocalResource, MonitorRemoteResource, ResourceNotFoundException,
    ServiceQuotaExceededException, TargetId, TargetResource, UpdateMonitorInput, ValidationException,
};

mod performance_monitors;
mod update_tokens;

pub use performance_monitors::*;
pub use update_tokens::*;

/// The Region an ARN carries when the request is unsigned, so no SigV4 credential
/// scope names one.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN service namespace for every resource this service mints
/// (`OPERATIONS.md`: "ARN namespace: `networkflowmonitor`").
pub const ARN_SERVICE: &str = "networkflowmonitor";

/// The ARN resource type of a scope.
pub const SCOPE_RESOURCE: &str = "scope";

/// The ARN resource type of a monitor. A PERFORMANCE monitor carries its own
/// ([`PERFORMANCE_MONITOR_RESOURCE`]).
pub const MONITOR_RESOURCE: &str = "monitor";

/// How long a freshly created resource stays in the non-terminal status the model
/// documents for it (`ScopeStatus` `IN_PROGRESS`, `MonitorStatus` `PENDING`: "The
/// monitor is in the process of being created") before the clock owes it the
/// terminal one (`SUCCEEDED`, `ACTIVE`).
///
/// AWS does not document how long either takes, so this span is chosen rather than
/// cited: long enough that a caller acting straight after the create still observes
/// the transient status, short enough that a client script does not wait. The due
/// instant is stored on the record ([`Scope::succeeded_at`], [`Monitor::active_at`])
/// and materialized by whichever call runs after it falls due — nothing ever sleeps.
pub const PROVISIONING_DELAY: Duration = Duration::from_millis(500);

/// The most items a `@paginated` list in this service returns in one page,
/// including when `maxResults` is absent. It is `MaxResults`' own `@range` maximum
/// (1..25), so no page can exceed what the model lets a caller ask for.
pub const MAX_PAGE_SIZE: i32 = 25;

/// The most tags one resource carries, from `TagMap`'s own `@length` maximum
/// (0..200, `smithy_query(query="shape",shape="TagMap")`). It bounds what
/// `TagResource` can MERGE onto a resource: the model refuses a single request
/// carrying more than 200 entries at the boundary, and this is what keeps repeated
/// requests from growing a resource's tag set past what `ListTagsForResource` is
/// allowed to answer with.
pub const MAX_TAGS: usize = 200;

/// What a `resourceArn` names in the modelled errors of the tagging operations:
/// they address a scope or a monitor through one member, so the refusal names the
/// taggable resource rather than either store.
pub const TAGGED_RESOURCE: &str = "resource";

/// The most scopes one account holds in one Region — Network Flow Monitor's
/// published quota for the `scope` resource, which is ONE.
///
/// A scope is "all the resources for a specific root account" (`ScopeId`'s own
/// documentation), and the service holds a single one of them per account and
/// Region: a `CreateScope` that would claim a second slot in the Region the
/// request is made in is refused with the modelled
/// `ServiceQuotaExceededException` (HTTP 402, [`quota_exceeded`]) rather than
/// minting another topology for the same account. Deleting the scope frees the
/// slot again, so the quota bounds what the store holds rather than how many
/// creates a caller may make.
pub const SCOPE_QUOTA: usize = 1;

/// The most scopes this emulator holds at once ACROSS Regions.
///
/// [`SCOPE_QUOTA`] bounds what one Region's slot holds, and the Region comes from
/// the request's own credential scope, so without this the store would still grow
/// one record per Region a caller names. It is the emulator's own ceiling rather
/// than a published quota — chosen well above the number of Regions a client
/// script uses — and a create past it is refused with the same modelled
/// `ServiceQuotaExceededException` (HTTP 402). Deleting a scope frees its slot.
pub const MAX_SCOPES: usize = 100;

/// The most monitors this emulator holds at once, refused with the same modelled
/// `ServiceQuotaExceededException` (HTTP 402). Deleting a monitor frees its slot
/// (and the idempotency record that named it), so the store's size is bounded by
/// the live resources: at most `MAX_MONITORS` records, each holding at most
/// [`MAX_MONITOR_RESOURCES`] entries per resource list and [`MAX_UPDATE_TOKENS`]
/// remembered client tokens. Unlike [`SCOPE_QUOTA`] this number is the emulator's
/// own ceiling rather than a published quota.
pub const MAX_MONITORS: usize = 100;

/// The most top-contributors queries this emulator holds at once, for the same
/// reason and with the same refusal as [`MAX_MONITORS`]: a `StartQuery*` past it is
/// refused with the modelled `ServiceQuotaExceededException` (HTTP 402), which all
/// three `StartQuery*` operations declare, rather than letting an unauthenticated
/// caller grow this process's memory one query record at a time. A query record is
/// fixed size (the window, the metric and the status it was started with), so the
/// store's query half is bounded by this number alone. AWS publishes its own query
/// quota and it is not this number; this is the emulator's own ceiling, chosen well
/// above what a client script needs.
pub const MAX_QUERIES: usize = 200;

/// How long a freshly started query stays `QUEUED` before the clock owes it
/// `RUNNING`, and how long it stays `RUNNING` before the clock owes it the terminal
/// `SUCCEEDED` (`QueryStatus`: QUEUED | RUNNING | SUCCEEDED | FAILED | CANCELED).
///
/// AWS does not document how long a top-contributors query takes, so both spans are
/// chosen rather than cited: long enough that a caller polling straight after
/// `StartQuery*` still observes `QUEUED`, short enough that a client script does not
/// wait. The due instants are stored on the record ([`Query::running_at`],
/// [`Query::succeeded_at`]) and materialized by whichever call runs after they fall
/// due — nothing ever sleeps.
pub const QUERY_RUNNING_DELAY: Duration = Duration::from_millis(250);

/// The second half of the span [`QUERY_RUNNING_DELAY`] documents: when a query
/// started at `t` is owed `SUCCEEDED`.
pub const QUERY_COMPLETION_DELAY: Duration = Duration::from_millis(500);

/// What a `queryId` names in the modelled errors of the query operations.
pub const QUERY_RESOURCE: &str = "query";

/// The HTTP status a successful TAG MUTATION of this service answers with.
///
/// `TagResource` and `UntagResource` leave their success code defaulted in the model
/// (`@http` carries no `code:`, i.e. `codeDefaulted: true`,
/// `smithy_query(query="operation",operation="TagResource,UntagResource")`), so the
/// Smithy default of 200 is not a statement about this service: the status of a
/// bodiless success is a BEHAVIOURAL fact, and the recorded AWS responses are what
/// state it — a successful tag write answers 204 No Content, because its output shape
/// (`TagResourceOutput`/`UntagResourceOutput`) declares no members and there is
/// nothing for a client to parse. A 200 there promises an entity the response does not
/// carry, so every client path gated on 204 takes the wrong branch.
///
/// This is NOT extended to the other operations of this model whose output structure
/// is also memberless (`DeleteScope`, `DeleteMonitor`, `StopQuery*`): their codes are
/// defaulted too, no recorded response in this corpus states their status, and the
/// model's 200 is all there is to go on until one does.
pub const NO_CONTENT_SUCCESS_STATUS: u16 = 204;

/// The most entries one monitor's `localResources` or `remoteResources` list
/// holds, and the most entries one request may name in any ONE of its resource
/// lists (`CreateMonitor`'s two, `UpdateMonitor`'s four).
///
/// Neither `MonitorLocalResources` nor `MonitorRemoteResources` carries a
/// `@length`, so nothing before a handler caps how long these lists grow, and an
/// entry's `identifier` is caller-chosen free text: without a ceiling here a
/// caller repeating `UpdateMonitor` with fresh identifiers grows one record — and
/// the work every later call does over it — without limit. A request that would
/// leave a list past this ceiling is refused with the modelled
/// `ValidationException` (HTTP 400, naming the member), which is the refusal both
/// operations model for a member they will not accept. AWS publishes its own
/// quota and it is not this number; this is the emulator's own ceiling, chosen
/// well above what a client script needs.
pub const MAX_MONITOR_RESOURCES: usize = 100;

/// The most targets one scope's `targets` list holds, and the most targets one
/// request may name in either of `UpdateScope`'s two lists.
///
/// The scope counterpart of [`MAX_MONITOR_RESOURCES`], for the same reason: a
/// target's identity includes its caller-chosen `accountId`, so without a ceiling
/// a caller repeating `UpdateScope` with fresh account ids would grow one stored
/// record — and the work every later call does over it — without limit.
/// `TargetResourceList` carries `@length(1, 99)` on `UpdateScope`'s own members,
/// so the boundary validator already caps ONE request's lists; this is what caps
/// what the STORE ends up holding across calls. A request that would leave a
/// scope past it is refused with the modelled `ValidationException` (HTTP 400,
/// naming the member), the refusal `UpdateScope` models for a member it will not
/// accept.
pub const MAX_SCOPE_TARGETS: usize = 100;

/// How many `clientToken`s of already-applied updates a monitor remembers, so a
/// replayed `@idempotent` update is recognized as a replay instead of applying
/// twice ([`Monitor::update_replayed`]).
///
/// AWS does not document how long a client token is remembered, so this window is
/// chosen rather than cited: deep enough that a retry — even behind a few other
/// updates to the same monitor — is still recognized, and bounded so that a caller
/// issuing updates forever cannot grow this process's memory without limit. The
/// oldest token is forgotten first.
pub const MAX_UPDATE_TOKENS: usize = 64;

/// `ScopeStatus`, the lifecycle status of a scope. The wire values are the model's
/// (SUCCEEDED | IN_PROGRESS | FAILED | DEACTIVATING | DEACTIVATED); only the ones
/// this slice can produce are modelled here, and the others arrive with the
/// operations that move a scope into them. `DeleteScope` is not one of them: it
/// removes the scope outright, so no reader sees a deactivation status for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeStatus {
    /// The scope is being set up: the status a create answers with.
    InProgress,
    /// The scope is ready, and metrics can be requested for its topology.
    Succeeded,
}

impl ScopeStatus {
    /// The value this status serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::InProgress => "IN_PROGRESS",
            Self::Succeeded => "SUCCEEDED",
        }
    }
}

/// `MonitorStatus`, the lifecycle status of a monitor. As with [`ScopeStatus`],
/// only the values this slice produces are modelled; `INACTIVE`, `ERROR` and
/// `DELETING` arrive with the operations that produce them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonitorStatus {
    /// "The monitor is in the process of being created" — the status a create
    /// answers with.
    Pending,
    /// "The monitor is active."
    Active,
}

impl MonitorStatus {
    /// The value this status serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Active => "ACTIVE",
        }
    }
}

/// What an ARN in [`ComAmazonawsNetworkflowmonitorNetworkflowmonitorState::arn_index`]
/// names: the store it lives in and its key there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceRef {
    /// A scope, by `scopeId`.
    Scope(String),
    /// A monitor, by `monitorName`.
    Monitor(String),
    /// A performance monitor, by `monitorName`. Its own variant because a monitor
    /// and a performance monitor of one name are two resources in two stores.
    PerformanceMonitor(String),
}

/// A Network Flow Monitor scope, as this emulator holds it.
#[derive(Debug, Clone)]
pub struct Scope {
    /// `scopeId`.
    pub scope_id: String,
    /// `scopeArn`.
    pub scope_arn: String,
    /// The Region this scope lives in: the one the create request was made in,
    /// which is also its ARN's Region segment. A scope is a per-account/Region
    /// resource, so this is what [`SCOPE_QUOTA`] is counted per
    /// ([`ComAmazonawsNetworkflowmonitorNetworkflowmonitorState::scopes_in_region`]).
    pub region: String,
    /// `status`.
    pub status: ScopeStatus,
    /// `targets` — exactly the Region/account pairs the create request carried.
    pub targets: Vec<TargetResource>,
    /// The tags the create request carried.
    pub tags: BTreeMap<String, String>,
    /// When the clock owes this scope its terminal status.
    pub succeeded_at: SystemTime,
    /// When this scope was created.
    pub created_at: SystemTime,
}

impl Scope {
    /// Materialize the status change the clock has already made due. Called by
    /// every handler before it reads or writes, so a reporting operation reports
    /// what a caller would see at this instant without changing anything a later
    /// caller would not already have seen.
    fn settle(&mut self, now: SystemTime) {
        if self.status == ScopeStatus::InProgress && now >= self.succeeded_at {
            self.status = ScopeStatus::Succeeded;
        }
    }

    /// Whether this record is the one `CreateScope` would mint for `input` — the
    /// comparison an idempotent replay makes before answering with it, so a
    /// `clientToken` reused with DIFFERENT parameters conflicts instead of quietly
    /// answering with the first call's scope ("Don't reuse the same client token
    /// for other API requests").
    pub fn matches_create(&self, input: &CreateScopeInput) -> bool {
        same_targets(&self.targets, &input.targets) && self.tags == tags_of(input.tags.as_ref())
    }
}

/// A Network Flow Monitor monitor, as this emulator holds it.
#[derive(Debug, Clone)]
pub struct Monitor {
    /// `monitorName` — unique within the store, and this monitor's key.
    pub monitor_name: String,
    /// `monitorArn`.
    pub monitor_arn: String,
    /// `monitorStatus`.
    pub monitor_status: MonitorStatus,
    /// The `scopeArn` this monitor was created under.
    pub scope_arn: String,
    /// `localResources`.
    pub local_resources: Vec<MonitorLocalResource>,
    /// `remoteResources` — empty when the create request carried none.
    pub remote_resources: Vec<MonitorRemoteResource>,
    /// The tags the create request carried.
    pub tags: BTreeMap<String, String>,
    /// `createdAt`.
    pub created_at: SystemTime,
    /// `modifiedAt`.
    pub modified_at: SystemTime,
    /// When the clock owes this monitor its terminal status.
    pub active_at: SystemTime,
    /// The `clientToken`s of the updates already applied to this monitor, each
    /// with the fingerprint of the change it carried ([`update_fingerprint`]),
    /// oldest first and at most [`MAX_UPDATE_TOKENS`] of them. A deque rather than
    /// a map because the eviction order is what bounds it; the tokens go away with
    /// the monitor.
    pub update_tokens: VecDeque<(String, String)>,
}

impl Monitor {
    /// The monitor counterpart of [`Scope::settle`]. Reaching `ACTIVE` is a change
    /// to the monitor, so `modifiedAt` moves to the instant that change fell due —
    /// the stored due instant, never "now", so two readers see one value.
    fn settle(&mut self, now: SystemTime) {
        if self.monitor_status == MonitorStatus::Pending && now >= self.active_at {
            self.monitor_status = MonitorStatus::Active;
            self.modified_at = self.active_at;
        }
    }

    /// What `token` means for this monitor, given the change `fingerprint` the
    /// request carries ([`update_fingerprint`]) — the one replay rule every update
    /// of this service follows ([`replay_of`]), so a retried update neither applies
    /// twice nor moves `modifiedAt` again.
    pub fn update_replay(&self, token: Option<&str>, fingerprint: &str) -> UpdateReplay {
        replay_of(&self.update_tokens, token, fingerprint)
    }

    /// Remember `token` as having applied the change `fingerprint` describes,
    /// forgetting the oldest once the window is full ([`remember`]).
    pub fn remember_update(&mut self, token: String, fingerprint: String) {
        remember(&mut self.update_tokens, token, fingerprint);
    }

    /// Apply an `UpdateMonitor` request's additions and removals to this monitor's
    /// resource lists, answering whether the stored record actually changed (which
    /// is what decides whether `modifiedAt` moves).
    ///
    /// Each list is edited as a SET keyed by an entry's identity (`type` +
    /// `identifier`): the removals land first and then the additions, so
    /// repeating either is a no-op rather than a duplicate or an error. A request
    /// naming one resource in BOTH an add and a remove list never reaches here —
    /// it is refused as a contradiction before the monitor is read.
    ///
    /// A request that would leave a stored list past [`MAX_MONITOR_RESOURCES`] is
    /// refused: the answer is the member to name in a `ValidationException` and
    /// the reason, and NOTHING is applied, so a refused update leaves the record
    /// exactly as it was. The request's own list lengths are refused earlier, by
    /// [`request_overflow`], before this monitor is touched or the request is
    /// fingerprinted.
    pub fn apply_update(
        &mut self,
        input: &UpdateMonitorInput,
    ) -> Result<bool, (&'static str, String)> {
        let local_to_add = input.local_resources_to_add.as_deref().unwrap_or_default();
        let local_to_remove = input
            .local_resources_to_remove
            .as_deref()
            .unwrap_or_default();
        let remote_to_add = input.remote_resources_to_add.as_deref().unwrap_or_default();
        let remote_to_remove = input
            .remote_resources_to_remove
            .as_deref()
            .unwrap_or_default();

        let local = merged_resources(&self.local_resources, local_to_remove, local_to_add);
        if local.len() > MAX_MONITOR_RESOURCES {
            return Err(("localResourcesToAdd", too_many_resources(local.len())));
        }
        let remote = merged_resources(&self.remote_resources, remote_to_remove, remote_to_add);
        if remote.len() > MAX_MONITOR_RESOURCES {
            return Err(("remoteResourcesToAdd", too_many_resources(remote.len())));
        }

        // "The last date and time that the monitor was modified": the record
        // changed only if a list really ended up different, so a request whose
        // removals and additions cancel out is a no-op.
        let changed = !same_resources(&local, &self.local_resources)
            || !same_resources(&remote, &self.remote_resources);
        self.local_resources = local;
        self.remote_resources = remote;
        Ok(changed)
    }

    /// The monitor counterpart of [`Scope::matches_create`], including the scope:
    /// a token reused under a different scope is a mismatch, not a replay. The
    /// request's lists are compared in the form a create STORES them (deduped), so
    /// a replay of a request carrying a duplicate entry still matches its monitor.
    pub fn matches_create(&self, input: &CreateMonitorInput) -> bool {
        self.monitor_name == input.monitor_name
            && self.scope_arn == input.scope_arn
            && same_resources(
                &self.local_resources,
                &deduped_resources(&input.local_resources),
            )
            && same_resources(
                &self.remote_resources,
                &deduped_resources(input.remote_resources.as_deref().unwrap_or_default()),
            )
            && self.tags == tags_of(input.tags.as_ref())
    }
}

/// Which family of top-contributors query a record is, and so which operations
/// reach it: the model binds three independent query lifecycles over three
/// different URI families, each with its own `StartQuery*`, `StopQuery*`,
/// `GetQueryStatus*` and `GetQueryResults*`. A `queryId` minted by one is not a
/// query of another, so the kind is part of a lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryKind {
    /// `POST /monitors/{monitorName}/topContributorsQueries`.
    MonitorTopContributors,
    /// `POST /workloadInsights/{scopeId}/topContributorsQueries`.
    WorkloadInsightsTopContributors,
    /// `POST /workloadInsights/{scopeId}/topContributorsDataQueries`.
    WorkloadInsightsTopContributorsData,
    /// `POST /performance-monitors/{monitorName}/queries`.
    PerformanceMonitor,
}

/// `QueryStatus`, the lifecycle status of a query. The wire values are the model's
/// (QUEUED | RUNNING | SUCCEEDED | FAILED | CANCELED); `FAILED` is not modelled
/// here because nothing in this emulator makes a query fail — a query whose window
/// holds no observed network flow still succeeds, with no rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryStatus {
    /// The query has been accepted and not started yet: the status a start
    /// answers with.
    Queued,
    /// The query is running.
    Running,
    /// The query finished and its results can be read.
    Succeeded,
    /// The query was stopped by `StopQuery*` before it finished.
    Canceled,
}

impl QueryStatus {
    /// The value this status serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Queued => "QUEUED",
            Self::Running => "RUNNING",
            Self::Succeeded => "SUCCEEDED",
            Self::Canceled => "CANCELED",
        }
    }

    /// Whether this status is terminal: the query will not move again on its own.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Canceled)
    }
}

/// What a query was started WITH: the members of its own `StartQuery*` request that
/// its results are reported from.
///
/// The two query planes of this service name different ones — the three
/// top-contributors families name a metric and a destination category, and the
/// performance-monitor family names a query type and its filters — so a record
/// holds whichever its own plane carries rather than an empty member standing in
/// for the other's.
#[derive(Debug, Clone)]
pub enum QueryTerms {
    /// A top-contributors query: the `metricName` whose unit its rows are reported
    /// in, and the `destinationCategory` it was started for.
    TopContributors {
        /// `metricName`.
        metric_name: String,
        /// `destinationCategory`.
        destination_category: String,
    },
    /// A performance-monitor query: the `queryType` whose schema its results are
    /// reported under, and the `filterParameters` the request carried.
    Paths {
        /// `queryType`.
        query_type: String,
        /// `filterParameters`, in request order and empty when the request carried
        /// none.
        filters: Vec<FilterParameter>,
    },
}

impl QueryTerms {
    /// The `metricName` this query was started with, or `None` for a plane that
    /// names no metric: what decides the `unit` a top-contributors result set is
    /// reported in ([`metric_unit`]).
    pub fn metric_name(&self) -> Option<&str> {
        match self {
            Self::TopContributors { metric_name, .. } => Some(metric_name.as_str()),
            Self::Paths { .. } => None,
        }
    }

    /// The `queryType` this query was started with, or `None` for a plane that
    /// names no query type.
    pub fn query_type(&self) -> Option<&str> {
        match self {
            Self::TopContributors { .. } => None,
            Self::Paths { query_type, .. } => Some(query_type.as_str()),
        }
    }
}

/// A query of either of this service's query planes, as this emulator holds it.
///
/// The record is everything the `StartQuery*` request asked for plus the lifecycle
/// the clock drives, so `GetQueryStatus*` and `GetQueryResults*` answer from what
/// was started rather than from their own arguments.
#[derive(Debug, Clone)]
pub struct Query {
    /// `queryId` — unique within the store, and this query's key.
    pub query_id: String,
    /// Which query lifecycle this record belongs to.
    pub kind: QueryKind,
    /// The resource the query was started for: the `monitorName` of a monitor or
    /// performance-monitor query, the `scopeId` of a workload-insights one. A query
    /// is only reachable under the resource it was started for.
    pub owner: String,
    /// What the query was started with, as its own plane names it.
    pub terms: QueryTerms,
    /// `startTime` — the beginning of the window the query covers, kept exactly as
    /// the request carried it.
    pub start_time: ::aws_smithy_types::DateTime,
    /// `endTime` — the end of that window.
    pub end_time: ::aws_smithy_types::DateTime,
    /// `limit`, when the request carried one.
    pub limit: Option<i32>,
    /// `status`.
    pub status: QueryStatus,
    /// When the clock owes this query `RUNNING`.
    pub running_at: SystemTime,
    /// When the clock owes this query `SUCCEEDED`.
    pub succeeded_at: SystemTime,
}

impl Query {
    /// The query counterpart of [`Scope::settle`]: materialize the status changes
    /// the clock has already made due. A terminal status never moves, so a query
    /// `StopQuery*` canceled stays canceled however long the completion instant it
    /// would otherwise have reached is past.
    fn settle(&mut self, now: SystemTime) {
        if self.status == QueryStatus::Queued && now >= self.running_at {
            self.status = QueryStatus::Running;
        }
        if self.status == QueryStatus::Running && now >= self.succeeded_at {
            self.status = QueryStatus::Succeeded;
        }
    }

    /// What `StopQuery*` does to this query: cancel it while it is still moving,
    /// and nothing at all once it has reached a terminal status.
    ///
    /// The stop is modelled as an idempotent fire-and-forget — `@idempotent`, an
    /// EMPTY output shape, and no status of its own to report — so it asks only
    /// that the query not go on running, which is already true of a query that
    /// finished or was already canceled. Such a query KEEPS the outcome it
    /// reached, so a later `GetQueryStatus*` still reports it, and the call
    /// succeeds: refusing the retry of a stop that already landed, or the
    /// defensive stop a cleanup path makes, would fail a request the service has
    /// nothing left to do for.
    pub fn stop(&mut self) {
        if !self.status.is_terminal() {
            self.status = QueryStatus::Canceled;
        }
    }
}

/// The `MetricUnit` a metric's values are reported in, from the metric's own
/// meaning: a byte count for data transferred, a duration for a round trip, and a
/// plain count for the two event metrics.
///
/// The values are `MetricUnit`'s own (`smithy_query(query="shape",
/// shape="MetricUnit")`: Seconds | … | Bytes | … | Count | …), and the metrics are
/// the union of `MonitorMetric` (ROUND_TRIP_TIME | TIMEOUTS | RETRANSMISSIONS |
/// DATA_TRANSFERRED) and `WorkloadInsightsMetric` (TIMEOUTS | RETRANSMISSIONS |
/// DATA_TRANSFERRED), so one function serves both query families. A metric the
/// model does not declare cannot reach here — the boundary validator refuses it
/// against `MonitorMetric`/`WorkloadInsightsMetric` before a handler runs — so an
/// unknown name is reported as `None`, the unit the model has for "no unit".
pub fn metric_unit(metric_name: &str) -> &'static str {
    match metric_name {
        "DATA_TRANSFERRED" => "Bytes",
        "ROUND_TRIP_TIME" => "Milliseconds",
        "TIMEOUTS" | "RETRANSMISSIONS" => "Count",
        _ => "None",
    }
}

/// A create request's `tags` as a record holds them: ordered, and empty when the
/// member is absent.
pub fn tags_of(tags: Option<&HashMap<String, String>>) -> BTreeMap<String, String> {
    tags.map(|tags| {
        tags.iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    })
    .unwrap_or_default()
}

/// The stored tags as the wire carries them: present only when the resource has
/// any, so a resource created without tags reports none rather than an empty map.
pub fn tags_for_output(tags: &BTreeMap<String, String>) -> Option<HashMap<String, String>> {
    if tags.is_empty() {
        None
    } else {
        Some(
            tags.iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        )
    }
}

/// The stored tags as a TAG LISTING carries them: always a map, and empty when
/// the resource has none.
///
/// `tags` is the whole of `ListTagsForResource`'s output ("Returns all the tags for
/// a resource"), so an untagged resource answers `{}` — the listing says "this
/// resource has no tags" rather than leaving the one member it has unset. The
/// create/read shapes use [`tags_for_output`] instead, where the record's other
/// members already say what was read.
pub fn tags_as_map(tags: &BTreeMap<String, String>) -> HashMap<String, String> {
    tags.iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// The account id a `TargetId` union carries. `accountId` is its only member
/// today ("currently, a target is always an Amazon Web Services account"), so an
/// id that is not one compares equal to nothing.
pub fn target_account_id(id: &TargetId) -> &str {
    match id {
        TargetId::accountId(account_id) => account_id.as_str(),
    }
}

/// The account that owns every resource this emulator holds — the account segment
/// of every ARN it mints. A scope is "all the resources for a specific root
/// account", and that root account here is the emulator's own.
pub fn owner_account_id() -> &'static str {
    ::engine::emulator_account_id()
}

/// An entry of a list that an update edits as a SET: the scope's `targets` and a
/// monitor's `localResources` / `remoteResources`.
///
/// `MonitorLocalResource` and `MonitorRemoteResource` are generated wire shapes
/// that derive no `PartialEq` and share no trait, so this is how ONE comparison
/// serves both lists (and any further resource list the service grows). It is
/// `pub` only because the handlers' entry points ([`deduped_resources`]) are
/// generic over it; nothing outside this module implements it.
pub trait MonitorResource {
    /// The entry's `type` member.
    fn resource_type(&self) -> &str;
    /// The entry's `identifier` member.
    fn identifier(&self) -> &str;
}

impl MonitorResource for MonitorLocalResource {
    fn resource_type(&self) -> &str {
        self.r#type.as_str()
    }

    fn identifier(&self) -> &str {
        self.identifier.as_str()
    }
}

impl MonitorResource for MonitorRemoteResource {
    fn resource_type(&self) -> &str {
        self.r#type.as_str()
    }

    fn identifier(&self) -> &str {
        self.identifier.as_str()
    }
}

/// The first entry an update names in BOTH its add list and its remove list, or
/// `None` when the two lists are disjoint.
///
/// An update that names one resource in both says two contradictory things about
/// that resource, which is what makes the REQUEST bad rather than an edit with a
/// winner, so every add/remove pair is checked through here before anything is
/// applied.
pub fn overlapping_resources<'a, T: MonitorResource>(
    to_add: &'a [T],
    to_remove: &[T],
) -> Option<&'a T> {
    let removed: BTreeSet<(String, String)> = to_remove.iter().map(identity).collect();
    to_add
        .iter()
        .find(|entry| removed.contains(&identity(*entry)))
}

/// The list `stored` becomes when a request's removals and additions are applied:
/// every entry `to_remove` does not name, in its stored order, followed by every
/// entry of `to_add` the result does not already hold. Entries are compared by
/// identity ([`MonitorResource`]) through a `BTreeSet` of identities, so one call
/// costs O((n + m) log n) rather than a scan per entry.
///
/// One function serves the local and the remote list — and any further monitor
/// resource list the service grows — for the same reason [`same_resources`] does.
/// It never grows the result past what it was asked to merge, so the caller bounds
/// the stored list by checking the length it answers with.
fn merged_resources<T: MonitorResource + Clone>(
    stored: &[T],
    to_remove: &[T],
    to_add: &[T],
) -> Vec<T> {
    let removed: BTreeSet<(String, String)> = to_remove.iter().map(identity).collect();
    let mut merged: Vec<T> = stored
        .iter()
        .filter(|entry| !removed.contains(&identity(*entry)))
        .cloned()
        .collect();
    let mut present: BTreeSet<(String, String)> = merged.iter().map(identity).collect();
    for addition in to_add {
        if present.insert(identity(addition)) {
            merged.push(addition.clone());
        }
    }
    merged
}

/// The reason text of the modelled 400 for a resource list this service will not
/// hold, built from the request so a caller can fix the request from it alone.
/// `CreateMonitor` refuses its own two lists with it, `UpdateMonitor` its four.
pub fn too_many_resources(named: usize) -> String {
    format!(
        "{named} resources, more than the {MAX_MONITOR_RESOURCES} one monitor list \
         holds in this emulator"
    )
}

/// The scope counterpart of [`too_many_resources`]: the reason text of the
/// modelled 400 for a `targets` list this service will not hold, built from the
/// request so a caller can fix the request from it alone.
pub fn too_many_targets(named: usize) -> String {
    format!(
        "{named} targets, more than the {MAX_SCOPE_TARGETS} one scope holds in \
         this emulator"
    )
}

/// Which member of an `UpdateMonitor` request this service will not accept on the
/// request's own terms, and why — or `None` when every one of its four resource
/// lists is within [`MAX_MONITOR_RESOURCES`].
///
/// This is the FIRST thing the handler checks, before it reads the monitor,
/// fingerprints the request or merges anything: no list a monitor may hold can
/// name more entries than a monitor holds, whether it adds them or removes them,
/// so every later step — the work the merge does, and the digest the remembered
/// `clientToken` keeps — is derived from a request this ceiling already bounds.
pub fn request_overflow(input: &UpdateMonitorInput) -> Option<(&'static str, String)> {
    [
        (
            "localResourcesToAdd",
            input.local_resources_to_add.as_deref().unwrap_or_default().len(),
        ),
        (
            "localResourcesToRemove",
            input
                .local_resources_to_remove
                .as_deref()
                .unwrap_or_default()
                .len(),
        ),
        (
            "remoteResourcesToAdd",
            input
                .remote_resources_to_add
                .as_deref()
                .unwrap_or_default()
                .len(),
        ),
        (
            "remoteResourcesToRemove",
            input
                .remote_resources_to_remove
                .as_deref()
                .unwrap_or_default()
                .len(),
        ),
    ]
    .into_iter()
    .find(|(_, named)| *named > MAX_MONITOR_RESOURCES)
    .map(|(member, named)| (member, too_many_resources(named)))
}

/// The list a create stores for the list a request carried: the same entries in
/// the same order, with any entry naming a resource an earlier one already names
/// dropped. A monitor's resource lists are sets keyed by `type` + `identifier`
/// everywhere else, so they are sets from the moment the record is minted.
pub fn deduped_resources<T: MonitorResource + Clone>(list: &[T]) -> Vec<T> {
    merged_resources(&[], &[], list)
}

/// What an `UpdateMonitor` request asks for, as ONE fixed-size value a later
/// replay can be compared against: the SHA-256 digest, hex encoded, of the
/// identities of its four resource lists in request order.
///
/// A digest rather than the serialized lists themselves, because this value is
/// retained in the store next to the `clientToken` that applied it
/// ([`Monitor::update_tokens`]): the digest is 64 characters whatever the request
/// carried, so what one remembered token costs the store does not depend on the
/// size of the request that minted it. Each piece going into the digest is
/// length-prefixed, so no identifier's own punctuation can make two different
/// requests fingerprint alike.
pub fn update_fingerprint(input: &UpdateMonitorInput) -> String {
    use ::sha2::Digest;

    let mut hasher = ::sha2::Sha256::new();
    hash_list(
        &mut hasher,
        input.local_resources_to_add.as_deref().unwrap_or_default(),
    );
    hash_list(
        &mut hasher,
        input
            .local_resources_to_remove
            .as_deref()
            .unwrap_or_default(),
    );
    hash_list(
        &mut hasher,
        input.remote_resources_to_add.as_deref().unwrap_or_default(),
    );
    hash_list(
        &mut hasher,
        input
            .remote_resources_to_remove
            .as_deref()
            .unwrap_or_default(),
    );
    ::hex::encode(hasher.finalize())
}

/// Feed one resource list's length-prefixed identities into `hasher`.
fn hash_list<T: MonitorResource>(hasher: &mut ::sha2::Sha256, list: &[T]) {
    use ::sha2::Digest;

    hasher.update(list.len().to_le_bytes());
    for entry in list {
        let (resource_type, identifier) = identity_ref(entry);
        hasher.update(resource_type.len().to_le_bytes());
        hasher.update(resource_type.as_bytes());
        hasher.update(identifier.len().to_le_bytes());
        hasher.update(identifier.as_bytes());
    }
}

/// A resource list entry's identity: the two members that say WHICH resource it
/// is, owned so it can be compared against a list that is being rebuilt.
fn identity<T: MonitorResource>(entry: &T) -> (String, String) {
    let (resource_type, identifier) = identity_ref(entry);
    (resource_type.to_string(), identifier.to_string())
}

/// The same identity, borrowed: what the comparison paths use, so comparing two
/// lists allocates nothing.
fn identity_ref<T: MonitorResource>(entry: &T) -> (&str, &str) {
    (entry.resource_type(), entry.identifier())
}

/// Whether two resource lists name the same resources in the same order, for the
/// same reason as [`same_targets`].
fn same_resources<T: MonitorResource>(stored: &[T], requested: &[T]) -> bool {
    stored.len() == requested.len()
        && stored
            .iter()
            .zip(requested)
            .all(|(left, right)| identity_ref(left) == identity_ref(right))
}

/// Whether two `targets` lists name the same Region/account pairs in the same
/// order. `TargetResource` is a generated wire shape and derives no `PartialEq`,
/// so the comparison goes through each target's identity.
fn same_targets(stored: &[TargetResource], requested: &[TargetResource]) -> bool {
    stored.len() == requested.len()
        && stored
            .iter()
            .zip(requested)
            .all(|(left, right)| target_identity(left) == target_identity(right))
}

/// A target's identity: the Region, the target type and the account it names —
/// the three members that say WHICH Region-account pair a target is, so a scope's
/// `targets` is edited as a set keyed by them, the way a monitor's resource lists
/// are keyed by [`identity`].
fn target_identity(target: &TargetResource) -> (String, String, String) {
    (
        target.region.clone(),
        target.target_identifier.target_type.clone(),
        target_account_id(&target.target_identifier.target_id).to_string(),
    )
}

/// The first target a request names in BOTH `resourcesToAdd` and
/// `resourcesToDelete`, or `None` when the two lists are disjoint: the target
/// counterpart of [`overlapping_resources`], for the same reason.
pub fn overlapping_targets<'a>(
    to_add: &'a [TargetResource],
    to_delete: &[TargetResource],
) -> Option<&'a TargetResource> {
    let deleted: BTreeSet<(String, String, String)> =
        to_delete.iter().map(target_identity).collect();
    to_add
        .iter()
        .find(|target| deleted.contains(&target_identity(target)))
}

/// The targets of `to_add` that `stored` ALREADY holds, in the order the request
/// named them, keyed by [`target_identity`] the way [`merged_targets`] keys them.
///
/// Empty when every addition is new. An update that asks to add a Region/account
/// pair the scope already holds is asking for something already true, so the
/// handler refuses the request with these pairs named rather than folding them
/// away.
pub fn targets_already_present<'a>(
    stored: &[TargetResource],
    to_add: &'a [TargetResource],
) -> Vec<&'a TargetResource> {
    let present: BTreeSet<(String, String, String)> = stored.iter().map(target_identity).collect();
    to_add
        .iter()
        .filter(|target| present.contains(&target_identity(target)))
        .collect()
}

/// The `targets` list a scope ends with once an update's deletions and then its
/// additions are applied: the target counterpart of [`merged_resources`], as a set
/// keyed by [`target_identity`], so a replay of one update lands on the same
/// targets and a target named twice is held once.
pub fn merged_targets(
    stored: &[TargetResource],
    to_delete: &[TargetResource],
    to_add: &[TargetResource],
) -> Vec<TargetResource> {
    let deleted: BTreeSet<(String, String, String)> =
        to_delete.iter().map(target_identity).collect();
    let mut merged: Vec<TargetResource> = stored
        .iter()
        .filter(|target| !deleted.contains(&target_identity(target)))
        .cloned()
        .collect();
    let mut present: BTreeSet<(String, String, String)> =
        merged.iter().map(target_identity).collect();
    for addition in to_add {
        if present.insert(target_identity(addition)) {
            merged.push(addition.clone());
        }
    }
    merged
}

/// Per-service state for com-amazonaws-networkflowmonitor-networkflowmonitor. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsNetworkflowmonitorNetworkflowmonitorState {
    /// Every scope, keyed by `scopeId` — a `BTreeMap`, so every list page is in a
    /// stable, resumable order.
    pub scopes: BTreeMap<String, Scope>,
    /// Every monitor, keyed by `monitorName`, for the same reason. The name is the
    /// key because the API addresses a monitor by it (`GET /monitors/{monitorName}`),
    /// which is also what makes a second create of one name a conflict.
    pub monitors: BTreeMap<String, Monitor>,
    /// Every performance monitor, keyed by `monitorName` — its own store, because a
    /// performance monitor is its own resource with its own collection (`GET
    /// /performance-monitors/{monitorName}`) and its own name space.
    pub performance_monitors: BTreeMap<String, PerformanceMonitor>,
    /// `clientToken` -> `scopeId`, so an `@idempotent` `CreateScope` replayed with
    /// the same token answers with the scope that token already minted.
    pub scope_idempotency: HashMap<String, String>,
    /// `clientToken` -> `monitorName`, for the same reason. Kept separate from
    /// `scope_idempotency` because the two creates are different operations: one
    /// token reused across both must not make `CreateMonitor` answer with a scope.
    pub monitor_idempotency: HashMap<String, String>,
    /// `clientToken` -> `monitorName` of a PERFORMANCE monitor, for the same reason
    /// and kept separate for the same one: one token reused across the two creates
    /// must not make either answer with the other's resource.
    pub performance_monitor_idempotency: HashMap<String, String>,
    /// Every minted ARN -> the record it names, so an operation handed a
    /// `scopeArn` or a `resourceArn` resolves it without scanning the store. This
    /// is how `CreateMonitor` refuses a `scopeArn` that is not a stored scope.
    pub arn_index: HashMap<String, ResourceRef>,
    /// Every top-contributors query, keyed by `queryId` — a `BTreeMap`, so a walk
    /// over the store is in a stable order. One map holds all three query families
    /// because a `queryId` is unique across the store and each record names its own
    /// [`QueryKind`]; splitting them would mean three lookups for one id.
    pub queries: BTreeMap<String, Query>,
    /// Source of the sequence number behind every minted `scopeId`. Monotonic, so
    /// no two scopes in one store ever share an id.
    pub next_sequence: u64,
}

impl ComAmazonawsNetworkflowmonitorNetworkflowmonitorState {
    /// Materialize every status change the clock has made due, as of `now`.
    pub fn settle(&mut self, now: SystemTime) {
        for scope in self.scopes.values_mut() {
            scope.settle(now);
        }
        for monitor in self.monitors.values_mut() {
            monitor.settle(now);
        }
        for monitor in self.performance_monitors.values_mut() {
            monitor.settle(now);
        }
        for query in self.queries.values_mut() {
            query.settle(now);
        }
    }

    /// How many scopes this account holds in `region` — what [`SCOPE_QUOTA`] is
    /// counted against, so a create claims a slot in the Region its request was
    /// made in and leaves every other Region's slot alone.
    pub fn scopes_in_region(&self, region: &str) -> usize {
        self.scopes
            .values()
            .filter(|scope| scope.region == region)
            .count()
    }

    /// The next sequence number: unique within this store for the life of the
    /// process.
    pub fn next_sequence(&mut self) -> u64 {
        self.next_sequence += 1;
        self.next_sequence
    }

    /// The scope an ARN names, or `None` when the ARN names nothing in this store
    /// (or names a monitor rather than a scope).
    pub fn scope_by_arn(&self, arn: &str) -> Option<&Scope> {
        match self.arn_index.get(arn) {
            Some(ResourceRef::Scope(scope_id)) => self.scopes.get(scope_id),
            _ => None,
        }
    }

    /// The monitor an ARN names, or `None` when the ARN names nothing in this store
    /// (or names a resource of this service that is not a monitor).
    pub fn monitor_by_arn(&self, arn: &str) -> Option<&Monitor> {
        match self.arn_index.get(arn) {
            Some(ResourceRef::Monitor(monitor_name)) => self.monitors.get(monitor_name),
            _ => None,
        }
    }

    /// Whether any monitor this store holds was created under the scope `scope_arn`
    /// names.
    ///
    /// A monitor names its scope by a `@required` ARN (`CreateMonitor.scopeArn`), so
    /// such a monitor is a live reference to that scope: this is what a destructive
    /// operation on the scope consults before it removes anything, so a scope a monitor
    /// still depends on is not taken out from under it ([`scope_in_use`]).
    pub fn scope_is_referenced(&self, scope_arn: &str) -> bool {
        self.monitors
            .values()
            .any(|monitor| monitor.scope_arn == scope_arn)
    }

    /// The tags of the resource an ARN names, or `None` when the ARN names nothing
    /// in this store.
    ///
    /// The tagging operations address EVERY taggable resource of this service
    /// through one `resourceArn` member, so they resolve it here rather than each
    /// naming the stores: whatever [`arn_index`](Self::arn_index) holds is
    /// taggable, and a resource type added later is reachable without touching
    /// them.
    pub fn tags_by_arn(&self, arn: &str) -> Option<&BTreeMap<String, String>> {
        match self.arn_index.get(arn)? {
            ResourceRef::Scope(scope_id) => self.scopes.get(scope_id).map(|scope| &scope.tags),
            ResourceRef::Monitor(name) => self.monitors.get(name).map(|monitor| &monitor.tags),
            ResourceRef::PerformanceMonitor(name) => self
                .performance_monitors
                .get(name)
                .map(|monitor| &monitor.tags),
        }
    }

    /// The same lookup for a writer. The reference is cloned out of the index
    /// first, so the borrow of the index ends before the record is reached.
    pub fn tags_by_arn_mut(&mut self, arn: &str) -> Option<&mut BTreeMap<String, String>> {
        match self.arn_index.get(arn)?.clone() {
            ResourceRef::Scope(scope_id) => {
                self.scopes.get_mut(&scope_id).map(|scope| &mut scope.tags)
            }
            ResourceRef::Monitor(name) => self
                .monitors
                .get_mut(&name)
                .map(|monitor| &mut monitor.tags),
            ResourceRef::PerformanceMonitor(name) => self
                .performance_monitors
                .get_mut(&name)
                .map(|monitor| &mut monitor.tags),
        }
    }

    /// The query `query_id` names in the `kind` lifecycle of `owner`, or `None` when
    /// no such query is in the store.
    ///
    /// All three parts have to match: a `queryId` is only reachable through the
    /// operation family that minted it and under the monitor or scope it was started
    /// for, so one query's id is never an id of another's lifecycle.
    pub fn query(&self, kind: QueryKind, owner: &str, query_id: &str) -> Option<&Query> {
        self.queries
            .get(query_id)
            .filter(|query| query.kind == kind && query.owner == owner)
    }

    /// The same lookup for a writer.
    pub fn query_mut(
        &mut self,
        kind: QueryKind,
        owner: &str,
        query_id: &str,
    ) -> Option<&mut Query> {
        self.queries
            .get_mut(query_id)
            .filter(|query| query.kind == kind && query.owner == owner)
    }

    /// Mint a query for a `StartQuery*` request and store it, answering the
    /// `queryId` the caller polls it with.
    ///
    /// `started_at` is the instant the call ran, which is what the two due instants
    /// are measured from, so the lifecycle a later reader materializes is the one
    /// this call started rather than one measured from whenever that reader ran.
    #[allow(clippy::too_many_arguments)]
    pub fn start_query(
        &mut self,
        kind: QueryKind,
        owner: &str,
        terms: QueryTerms,
        start_time: ::aws_smithy_types::DateTime,
        end_time: ::aws_smithy_types::DateTime,
        limit: Option<i32>,
        started_at: SystemTime,
    ) -> String {
        let id = query_id(self.next_sequence());
        self.queries.insert(
            id.clone(),
            Query {
                query_id: id.clone(),
                kind,
                owner: owner.to_string(),
                terms,
                start_time,
                end_time,
                limit,
                status: QueryStatus::Queued,
                running_at: started_at + QUERY_RUNNING_DELAY,
                succeeded_at: started_at + QUERY_COMPLETION_DELAY,
            },
        );
        id
    }

    /// Forget every query started under `owner` in the `kind` lifecycle.
    ///
    /// A query is only reachable under the resource it was started for, so the
    /// queries of a resource that is being deleted are reachable by nobody
    /// afterwards: dropping them with the resource is what keeps the store bounded
    /// by what is still live, the way a deleted resource's ARN and idempotency
    /// records go with it. It changes no answer — every lookup of one of those
    /// queries already fails on the missing owner.
    pub fn drop_queries(&mut self, kind: QueryKind, owner: &str) {
        self.queries
            .retain(|_, query| !(query.kind == kind && query.owner == owner));
    }
}

/// A `scopeId` for sequence number `sequence`.
///
/// `ScopeId` carries no pattern, and the real service's ids are UUIDs ("an
/// internally-generated identifier"), so this is UUID-shaped — 8-4-4-4-12
/// lowercase hex with RFC 4122's version (4) and variant (8) nibbles — with the
/// store's own monotonic sequence in the node field. That keeps every id unique
/// and makes id order creation order, so a list page resumes deterministically.
pub fn scope_id(sequence: u64) -> String {
    format!("00000000-0000-4000-8000-{sequence:012x}")
}

/// Whether `value` has the FORM of a `scopeId`: 8-4-4-4-12 groups of hex digits, the
/// UUID shape the real service's "internally-generated identifier"s take and the one
/// [`scope_id`] mints, so no id this store holds is ever refused by this check.
///
/// `ScopeId` carries no `@pattern`, so nothing before a handler runs checks the form —
/// but the form is still part of the request's SHAPE: a value that cannot be an
/// identifier this service generates names no scope under any state of the store, and
/// is refused as a bad request ([`invalid_scope_id`]) rather than looked up. Hex is
/// accepted in either case, so a client that upper-cases a UUID it was handed is not
/// told its request is malformed.
pub fn is_scope_id(value: &str) -> bool {
    let mut groups = value.split('-');
    let well_formed = [8usize, 4, 4, 4, 12].iter().all(|len| {
        groups.next().is_some_and(|group| {
            group.len() == *len && group.chars().all(|c| c.is_ascii_hexdigit())
        })
    });
    well_formed && groups.next().is_none()
}

/// A `queryId` for sequence number `sequence`.
///
/// `queryId` is a plain `String` in the model with no pattern, and the real
/// service's are "internally-generated identifier[s]", so this is UUID-shaped for
/// the same reason [`scope_id`] is — with RFC 4122's version (4) nibble and a
/// DIFFERENT variant nibble (9 rather than 8), so a minted query id is never equal
/// to a minted scope id even though both draw on the one store sequence.
pub fn query_id(sequence: u64) -> String {
    format!("00000000-0000-4000-9000-{sequence:012x}")
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsNetworkflowmonitorNetworkflowmonitorState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsNetworkflowmonitorNetworkflowmonitorState>)
}

/// The Region an ARN this request mints belongs in: the one the request's SigV4
/// credential scope names, else [`DEFAULT_REGION`].
///
/// A scope region that could not be an ARN region segment is not used, so a
/// crafted `Authorization` header can never produce a stored ARN a client cannot
/// parse.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= 63
                && region
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of one of this service's resources, through the one ARN builder the
/// framework vends (so the account segment is the canonical emulator account).
pub fn resource_arn(region: &str, resource_type: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("{resource_type}/{id}"))
}

/// What a refusal worded here states around its sentence: NOTHING.
///
/// A client reads a refused call as the sentence the service worded followed by four facts
/// its own call knows — the SERVICE that answered, the HTTP STATUS, the ID OF THE REQUEST and
/// how many attempts reaching it took. That envelope is the SDK's rendering of the exception
/// it raises, not part of the answer: the wire carries the sentence alone, and the request id
/// travels in the header this protocol names a request in
/// ([`crate::service::sdk_envelope`]).
///
/// `sentence` as a client is answered it: this service's own wording for the condition, and
/// nothing after it. Every CLIENT-ERROR refusal of this service is worded through here, so one
/// condition reads one sentence whichever operation refused and whichever class it refused
/// under.
pub fn refusal_message(sentence: impl Into<String>) -> String {
    sentence.into()
}

/// The modelled 500 for a request that could not reach this service's store.
pub fn state_unavailable() -> InternalServerException {
    InternalServerException {
        message: Some("the Network Flow Monitor state store is unavailable".to_string()),
    }
}

/// The modelled 404 for an id that names nothing in the store and names no
/// resource this service mints an ARN for.
///
/// The ARN-qualified forms below are for the resources this service DOES mint an
/// ARN for — a monitor ([`monitor_not_found`]) and a scope ([`scope_not_found`]) —
/// and a `queryId` has its own renderer ([`query_not_found`]). This generic form is
/// what is left: the tagging operations' `resourceArn`, which the caller already
/// addressed BY its ARN, so the id this message carries is that ARN.
pub fn not_found(resource_type: &str, id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(refusal_message(format!(
            "{} [{id}] doesn't exist",
            capitalized(resource_type)
        ))),
    }
}

/// `word` with its first character upper-cased, so the ARN resource type doubles
/// as the label a message opens with (`monitor` -> `Monitor`).
fn capitalized(word: &str) -> String {
    let mut characters = word.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
        None => String::new(),
    }
}

/// The modelled 404 for a `monitorName` this store holds no monitor under.
///
/// The message is BUILT FROM THE REQUEST and identifies the missing monitor by the
/// ARN it would carry — [`resource_arn`] over the request's Region, the same
/// builder `CreateMonitor` mints `monitorArn` with — so the partition, Region,
/// account and resource type of what is missing are all readable from the refusal
/// alone, and a caller holding a batch of names learns WHICH resource was absent.
///
/// The fixed words around the ARN are this service's own wording for a missing
/// MONITOR. A missing scope is worded differently ([`scope_not_found`]), which is
/// why the two are separate renderers rather than one template parameterized by a
/// resource type.
pub fn monitor_not_found(region: &str, monitor_name: &str) -> ResourceNotFoundException {
    missing_monitor(region, MONITOR_RESOURCE, monitor_name)
}

/// The sentence a monitor of either kind is reported missing with, over the ARN its
/// own resource type gives it: one wording for the condition, one ARN builder for
/// the resource ([`performance_monitor_not_found`] is the other caller).
pub fn missing_monitor(
    region: &str,
    resource_type: &str,
    monitor_name: &str,
) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(refusal_message(format!(
            "Monitor [{}] doesn't exist",
            resource_arn(region, resource_type, monitor_name)
        ))),
    }
}

/// The modelled 404 for a `scopeId` this store holds no scope under: the scope
/// counterpart of [`monitor_not_found`], ARN-qualified in the same way and from the
/// same builder, in this service's own wording for a missing SCOPE.
pub fn scope_not_found(region: &str, scope_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(refusal_message(format!(
            "There's no scope with ARN: [{}]",
            resource_arn(region, SCOPE_RESOURCE, scope_id)
        ))),
    }
}

/// How this service names a `queryId` that names no query it holds: a fixed prefix
/// and the request's own id in braces.
///
/// A query carries no ARN of its own — it is an identifier inside a monitor's or a
/// scope's own lifecycle — so the refusal identifies it by the id the REQUEST
/// carried, the way the renderers above identify a monitor or a scope by the ARN
/// built from the request. One sentence serves every operation that can be handed
/// an id naming no query, whichever error class that operation models for it.
fn no_such_query(query_id: &str) -> String {
    format!("There's no query with id {{{query_id}}}")
}

/// The modelled 404 for a `queryId` the store holds no query under, on the
/// `GetQueryResults*` readers — the operations whose model declares
/// `ResourceNotFoundException`.
pub fn query_not_found(query_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(refusal_message(no_such_query(query_id))),
    }
}

/// The same condition on an operation whose model declares NO
/// `ResourceNotFoundException` (`GetQueryStatus*`): the class is that operation's
/// modelled 400, and the message is the one sentence this service words a missing
/// query with.
pub fn unknown_query(query_id: &str) -> ValidationException {
    ValidationException {
        message: Some(refusal_message(no_such_query(query_id))),
    }
}

/// The modelled 400 for an update that names one item BOTH in an add list and in
/// the matching remove list.
///
/// The text is a constant for the condition, echoing nothing of the request: the
/// request carried no bad value — every entry of it is admissible on its own — it
/// said two contradictory things about one item, which is the whole of what the
/// caller has to be told. Every update of this service refuses the contradiction
/// with it, so the pair of lists it was found in does not change the wording.
pub fn add_remove_overlap() -> ValidationException {
    ValidationException {
        message: Some(refusal_message(
            "invalid input -> (detected overlap between add/remove resources)",
        )),
    }
}

/// The modelled 409 for a create of a name the store already holds.
///
/// The text is a constant for the condition, carrying no value from the request:
/// the caller already named the resource, and the condition is the same whichever
/// one it was.
pub fn already_exists(resource_type: &str) -> ConflictException {
    ConflictException {
        message: Some(refusal_message(format!(
            "Cannot create a {resource_type} which already exists"
        ))),
    }
}

/// The modelled 409 for an `@idempotencyToken` replayed with parameters that do
/// not match the resource it already minted.
pub fn idempotency_conflict(resource_type: &str, id: &str) -> ConflictException {
    ConflictException {
        message: Some(refusal_message(format!(
            "the clientToken of this request already created {resource_type} {id}, \
             with different parameters"
        ))),
    }
}

/// The modelled 402 for a create that would pass a quota, in the fixed wording the
/// real service answers an exceeded quota with.
///
/// The variable parts are the limit that was reached, the resource type it is a
/// limit on and the account the limit applies to — the caller's, which is the one
/// account this emulator holds resources for ([`owner_account_id`]) — so a client
/// that parses or logs the refusal reads the limit, what it limits and whose it is.
/// Every quota refusal of this service renders through here, whichever resource's
/// limit was reached.
pub fn quota_exceeded(resource_type: &str, limit: usize) -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: Some(refusal_message(format!(
            "Maximum limit of [{limit}] for resource [{resource_type}] exceeded for account [{}].",
            owner_account_id()
        ))),
    }
}

/// The modelled 400 for a member this service refuses, naming the member and the
/// rule it broke — the text is built from the request, so a caller can fix the
/// request from it alone.
pub fn field_validation(member: &str, message: impl Into<String>) -> ValidationException {
    ValidationException {
        message: Some(refusal_message(format!("{member}: {}", message.into()))),
    }
}

/// The modelled 400 for a `nextToken` this service did not issue.
///
/// A constant for the condition rather than a rendering of the rejected token:
/// every paginated list in this service refuses an unissued cursor with the same
/// text, so a paginating client can recognise "restart the scan" without parsing
/// anything out of the message.
pub fn invalid_next_token() -> ValidationException {
    ValidationException {
        message: Some(refusal_message("NextToken provided isn't valid.")),
    }
}

/// The wording a `scopeId` that is not of the generated form is refused with.
///
/// The status (400) and the code (`ValidationException`) are the contract; the text is
/// the fixed sentence the real service answers this condition with, recorded rather
/// than built from the request, which is why it is a constant and echoes nothing of
/// the `scopeId` that failed.
pub const INVALID_SCOPE_ID: &str = "The provided Scope ARN is not valid";

/// The modelled 400 for a `scopeId` whose FORM is not one this service generates
/// ([`is_scope_id`]).
///
/// Raised BEFORE the store is consulted: a malformed identifier is a fault in the
/// request itself (the client's own bug, never retryable), and reporting it as the
/// absence of a resource (404) would tell a reconciling client to retry or recreate.
pub fn invalid_scope_id() -> ValidationException {
    ValidationException {
        message: Some(refusal_message(INVALID_SCOPE_ID)),
    }
}

/// The wording a delete of a scope some monitor still names is refused with.
///
/// The sentence is wholly fixed — it has no variable parts: the caller already named
/// the scope it addressed, and what it has to be told is that the scope still has
/// monitors associated with it, which is the same whichever scope and however many
/// monitors.
pub const SCOPE_IN_USE: &str = "Scope can't be deleted as there's monitors associated with it";

/// The modelled 400 for a destructive request on a scope that a monitor still
/// references ([`ComAmazonawsNetworkflowmonitorNetworkflowmonitorState::scope_is_referenced`]).
///
/// A monitor holds a `@required` `scopeArn`, so removing the scope while monitors
/// name it would leave each of them pointing at an ARN this store no longer resolves.
/// The request is therefore refused BEFORE anything is written, in the real service's
/// own wording for the condition, and the caller deletes the monitors first.
pub fn scope_in_use() -> ValidationException {
    ValidationException {
        message: Some(refusal_message(SCOPE_IN_USE)),
    }
}

/// The modelled 403 for a reference to a scope or a monitor the store does not hold,
/// on an operation whose model declares NO `ResourceNotFoundException`.
///
/// Those operations cannot report an unresolvable reference as an absence, and a
/// reference the service cannot resolve it also cannot authorize — a scope and a
/// monitor each carry their own ARN, and so their own resource-level permission — so
/// the refusal is the modelled `AccessDeniedException` (403) rather than a 400, which
/// belongs to a request this service would refuse whatever the store held. The text is
/// built from the resource TYPE the request named, in the wording the real service
/// answers this condition with.
pub fn reference_denied(resource_type: &str) -> AccessDeniedException {
    AccessDeniedException {
        message: Some(refusal_message(format!(
            "{resource_type} supplied doesn't exist"
        ))),
    }
}

/// `Some(the modelled 400)` when `query` has no result page to report, `None` when
/// it does.
///
/// A query's results exist once that query has SUCCEEDED: one still `QUEUED` or
/// `RUNNING` has not produced them yet, and one `StopQuery*` left `CANCELED` never
/// will. Answering a successful empty page for either would make a query that never
/// finished indistinguishable from one that did, so the read is refused with the
/// modelled `ValidationException` — which, with the 404 for an id that names no
/// query, is the whole of what the results readers model for a client.
pub fn results_unavailable(query: &Query) -> Option<ValidationException> {
    if query.status == QueryStatus::Succeeded {
        return None;
    }
    Some(ValidationException {
        message: Some(refusal_message(format!(
            "query {} is {}; results are reported once a query has SUCCEEDED",
            query.query_id,
            query.status.as_wire()
        ))),
    })
}

/// The message the modelled 400 for an inverted query window carries.
///
/// A `StartQuery*` request whose `endTime` is earlier than its `startTime` names no
/// window at all, and the refusal is the same sentence whatever the two instants
/// were: it states the relation that failed and echoes nothing of the request. This
/// is the wording the real service answers that condition with, which is why it is
/// a constant rather than built per request.
pub const END_BEFORE_START: &str = "EndTime cannot be before StartTime";

/// `Some(the modelled 400)` when `end_time` is earlier than `start_time`, `None`
/// when the two bound a window a query can cover.
///
/// The three `StartQuery*` operations all take the same `@required` `startTime` /
/// `endTime` pair, so one check serves all three: a window is well formed when its
/// end is not before its start, and an empty window (the two equal) is well formed.
/// The comparison is on the instant each value names — whole seconds and then the
/// sub-second part — so two values that differ only below the second still order
/// correctly.
pub fn inverted_window(
    start_time: ::aws_smithy_types::DateTime,
    end_time: ::aws_smithy_types::DateTime,
) -> Option<ValidationException> {
    let instant = |at: ::aws_smithy_types::DateTime| (at.secs(), at.subsec_nanos());
    if instant(end_time) < instant(start_time) {
        return Some(ValidationException {
            message: Some(refusal_message(END_BEFORE_START)),
        });
    }
    None
}

/// The listing a page token belongs to, so a token minted by one list is not
/// accepted by another: a `ListMonitors` token carries this tag.
pub const MONITORS_PAGE_TAG: &str = "monitors/1:";

/// The same, for `ListScopes`: its own tag, so a token one listing issued never
/// resumes the other's walk.
pub const SCOPES_PAGE_TAG: &str = "scopes/1:";

/// The `nextToken` that resumes the `tag` listing at `key`.
///
/// `nextToken` is a plain `String` in the model with no documented form, so it is
/// this service's to choose: the hex encoding (the `hex` crate, rather than a
/// hand-rolled codec) of the listing's tag followed by the key the next page
/// starts at. The encoding is not a secret and is trivially reversible — it only
/// keeps a client from treating the token as a resource name and building one
/// itself; the TAG is what makes a token this listing did not mint unusable. The
/// service can always resume from a token it issued, including after the resource
/// the previous page stopped at was deleted, because the cursor is compared as a
/// lower bound rather than looked up.
pub fn page_token(tag: &str, key: &str) -> String {
    ::hex::encode(format!("{tag}{key}"))
}

/// The key a `nextToken` resumes the `tag` listing at, or `None` when the token is
/// not one this listing issued.
///
/// The tag is what makes that check real: a token has to decode to this listing's
/// own prefix, so neither a hex string a caller invented nor a token another
/// listing minted resumes a walk from an arbitrary lower bound.
pub fn decode_page_token(tag: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    decoded
        .strip_prefix(tag)
        .filter(|key| !key.is_empty())
        .map(ToString::to_string)
}

/// One page of a `@paginated` list over key-ordered `(key, item)` pairs: at most
/// `max_results` items (never more than [`MAX_PAGE_SIZE`]) starting at `after`,
/// and the token that resumes after them.
///
/// An absent `max_results` asks for everything, which this service answers with at
/// most [`MAX_PAGE_SIZE`] items plus a resume token: no single call returns the
/// whole store. The returned token is `None` on the last page, so paging through
/// yields every item exactly once.
pub fn page<'a, T, I>(
    items: I,
    tag: &str,
    after: Option<&str>,
    max_results: Option<i32>,
) -> (Vec<&'a T>, Option<String>)
where
    I: Iterator<Item = (&'a String, &'a T)>,
{
    let cursor = after.unwrap_or_default().to_string();
    let mut remaining = items.skip_while(move |(key, _)| key.as_str() < cursor.as_str());
    // A `maxResults` outside 1..25 is refused by the boundary validator before a
    // handler runs; the clamp is what makes an ABSENT one a bounded page.
    let limit = max_results.map_or(MAX_PAGE_SIZE, |value| value.clamp(1, MAX_PAGE_SIZE)) as usize;
    let page: Vec<&T> = remaining
        .by_ref()
        .take(limit)
        .map(|(_, item)| item)
        .collect();
    let next = remaining.next().map(|(key, _)| page_token(tag, key));
    (page, next)
}
