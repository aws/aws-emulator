// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-neptunedata-amazonneptunedataplane service.
//!
//! The records below live here, and every implemented operation computes its answer
//! from them:
//!
//! * [`FastResetRecord`] — the fast-reset token the engine is willing to accept, and what
//!   the resets it has performed left behind. `ExecuteFastReset` mints the token on
//!   `initiateDatabaseReset` and consumes it on `performDatabaseReset`.
//! * [`EngineStatusRecord`] — the one DB instance `GetEngineStatus` reports on. It is
//!   built ONCE, when this state is first reached (the instant this emulator's engine
//!   "started"), so every later read answers the same `startTime` instead of a fresh
//!   clock reading. What the CLUSTER this plane answers for settles — its engine version
//!   and the features a create turned on — is layered over that record from the shared
//!   directory the Neptune control plane records a provisioned cluster in (see
//!   [`engine_status`]).
//! * [`GremlinQueryRecord`] — one per Gremlin query the engine has accepted, keyed by
//!   query id. `ListGremlinQueries` and `GetGremlinQueryStatus` report on these and
//!   `CancelGremlinQuery` mutates them.
//! * [`LoaderJobRecord`] — one per bulk-load job the engine has admitted, keyed by load
//!   id, carrying what that load READ ([`LoadRun`]). `ListLoaderJobs` reports the load ids
//!   of the active ones and `GetLoaderJobStatus` reports one job's own run.
//! * [`IndexRebuildRecord`] — one per index rebuild the storage layer has admitted, keyed
//!   by rebuild id (see [`index_rebuild`]). `StartIndexRebuild` admits one,
//!   `GetIndexRebuildStatus` reports how far it has got, and `CancelIndexRebuild` stops one
//!   the storage layer is still busy with.
//! * [`PropertyGraph`] — the graph's own data: the nodes and relationships the
//!   openCypher endpoints read and write (see [`open_cypher`]). `ExecuteFastReset`
//!   empties it, because that is what a database reset leaves behind.
//! * [`StatisticsRecord`] — the DFE statistics the property-graph statistics endpoints
//!   report (see [`statistics`]): the generation mode `ManagePropertygraphStatistics` set
//!   and the latest run it performed, counted from [`PropertyGraph`] itself.
//!   `DeletePropertygraphStatistics` removes the record, so `GetPropertygraphStatistics`
//!   has nothing to answer from and refuses. A run also counts the whole GRAPH SUMMARY
//!   (see [`graph_summary`]), which is what `GetPropertygraphSummary` reports: the summary
//!   a client reads is the one the latest run computed, as of the instant it ran.
//!
//! `ExecuteGremlinExplainQuery` is what admits a query record for an explain: it reads the
//! submitted traversal and plans it (see [`gremlin_plan`]), which is work the engine really
//! did, so the query it finished is recorded like any other and the status family above
//! reports it. `ExecuteGremlinQuery` EVALUATES the traversal it reads against
//! [`PropertyGraph`] (see [`gremlin_eval`]) and records the query it answered the same way;
//! a traversal naming a step that evaluation does not cover yet is answered HTTP 501 rather
//! than with a result nothing computed, and `ExecuteGremlinProfileQuery` — which reports
//! what an evaluation MEASURED — still answers 501 for the same reason.
//!
//! The loader registry is written by the loader endpoints themselves: `StartLoaderJob`
//! reads the request, refuses one the loader could not act on (an `iamRoleArn` whose
//! credentials it cannot resolve), and otherwise READS the objects its source names out of
//! this emulator's own Amazon S3, writes what they held into [`PropertyGraph`] and records
//! the job the load ran under (see [`loader`]). `GetLoaderJobStatus` reports the
//! measurements of that run, `CancelLoaderJob` stops a load the engine still owes an
//! outcome, and `ListLoaderJobs` reports exactly the jobs the registry holds as active.
//!
//! `ExecuteOpenCypherQuery` is the one operation that both reads and writes the graph: it
//! parses the submitted query with a real openCypher parser and evaluates it against
//! [`PropertyGraph`], so what a query answers is what this store holds.
//!
//! `ExecuteOpenCypherExplainQuery` reports the PLAN for a query (see
//! [`open_cypher::explain_query`]) and changes nothing: it is a read, so the query it
//! explains is not recorded in the openCypher registry, and the modes that report what an
//! evaluation measured evaluate a query that writes against a copy of the graph which is
//! dropped when the report is written.

use std::collections::BTreeMap;
use std::time::SystemTime;

use crate::ops::types::{
    BadRequestException, FailureByQueryException, GremlinQueryStatus, IllegalArgumentException,
    InternalFailureException, InvalidParameterException, MissingParameterException, QueryEvalStats,
    ServerShutdownException,
};
mod engine_status;
mod graph_summary;
mod gremlin_eval;
mod gremlin_plan;
mod index_rebuild;
mod loader;
mod ml;
mod open_cypher;
mod parse_refusal;
mod property_graph;
mod statistics;

pub use engine_status::*;
pub use graph_summary::*;
pub use gremlin_eval::*;
pub use gremlin_plan::*;
pub use index_rebuild::*;
pub use loader::*;
pub use ml::*;
pub use open_cypher::*;
pub use parse_refusal::*;
pub use property_graph::*;
pub use statistics::*;

/// `CancelGremlinQueryOutput$status` for a cancellation the engine accepted. It reports
/// the status of the cancellation request itself, not of the query.
pub const CANCELLATION_ACCEPTED: &str = "200 OK";

/// How many FINISHED queries the registry keeps. A query the engine has finished is kept
/// so the status family can still report the id its submitter was given, but only the
/// most recent ones: the registry serves the queries in flight, and a stream of finished
/// ones must not grow it without end.
pub const FINISHED_QUERY_HISTORY: usize = 128;

/// `ListLoaderJobsOutput$status` for a job-list request the loader endpoint served — the
/// member "returns the status of the job list request", and the loader endpoint states
/// that as the HTTP status line of the answer.
pub const LOADER_JOB_LIST_SERVED: &str = "200 OK";

/// The maximum of `ListLoaderJobsInput$limit`, which is also its default — the member's
/// own documentation states both: "Must be a positive integer greater than zero and not
/// more than 100 (which is the default)".
pub const LOADER_JOB_LIST_MAX_LIMIT: i32 = 100;

/// The minimum of `ListLoaderJobsInput$limit`: the member must be "a positive integer
/// greater than zero", which the model declares as the range's lower bound.
pub const LOADER_JOB_LIST_MIN_LIMIT: i32 = 1;

/// `ExecuteFastResetOutput$status` for a fast-reset command the engine accepted — the
/// member reports "whether or not the fast reset rquest is accepted", and the system
/// endpoint states that acceptance as the HTTP status line of the answer.
pub const FAST_RESET_ACCEPTED: &str = "200 OK";

/// How far a Gremlin query the engine accepted has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryState {
    /// Accepted and queued, not evaluating yet.
    Waiting,
    /// Evaluating now.
    Running,
    /// Finished on its own.
    Done,
    /// Stopped by `CancelGremlinQuery`.
    Cancelled,
}

impl QueryState {
    /// Whether the engine still owes this query an answer — accepted but not completed,
    /// which is what `ListGremlinQueriesOutput$acceptedQueryCount` counts ("accepted but
    /// not yet completed, including queries in the queue").
    pub fn is_accepted(self) -> bool {
        matches!(self, Self::Waiting | Self::Running)
    }

    /// Whether the query is evaluating right now, which is what
    /// `ListGremlinQueriesOutput$runningQueryCount` counts.
    pub fn is_running(self) -> bool {
        matches!(self, Self::Running)
    }

    /// Whether the query has finished, one way or the other. A finished query cannot be
    /// cancelled and is no longer one of "the current queries" the list reports.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Cancelled)
    }
}

/// One Gremlin query the engine accepted, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct GremlinQueryRecord {
    /// The id the query is reached by — `GetGremlinQueryStatus`'s and
    /// `CancelGremlinQuery`'s `queryId` label, and the key this record is stored under.
    pub query_id: String,
    /// The query exactly as it was submitted.
    pub query_string: String,
    /// The `serializer` (the submitting request's `accept` header), if it named one. Set
    /// by whatever admits the query WITH one; the Gremlin explain endpoint takes no
    /// serializer, so a query it admits leaves this unset.
    pub serializer: Option<String>,
    /// When the engine accepted the query.
    pub accepted_at: SystemTime,
    /// When evaluation began; `None` while the query is still queued.
    pub started_at: Option<SystemTime>,
    /// When evaluation ended; `None` while the query has not finished.
    pub finished_at: Option<SystemTime>,
    pub state: QueryState,
    /// `QueryEvalStats$cancelled`: whether this query was cancelled.
    pub cancelled: bool,
    /// How many subqueries this query has.
    pub subqueries: u64,
}

impl GremlinQueryRecord {
    /// A record for a query accepted at `accepted_at` and still queued.
    pub fn accepted(
        query_id: impl Into<String>,
        query_string: impl Into<String>,
        accepted_at: SystemTime,
    ) -> Self {
        Self {
            query_id: query_id.into(),
            query_string: query_string.into(),
            serializer: None,
            accepted_at,
            started_at: None,
            finished_at: None,
            state: QueryState::Waiting,
            cancelled: false,
            subqueries: 0,
        }
    }

    /// A record for a query the engine accepted at `accepted_at`, evaluated, and finished
    /// at `finished_at` — which is every query answered within the call that submitted
    /// it, as an explain is.
    pub fn finished(
        query_id: impl Into<String>,
        query_string: impl Into<String>,
        accepted_at: SystemTime,
        finished_at: SystemTime,
    ) -> Self {
        Self {
            started_at: Some(accepted_at),
            finished_at: Some(finished_at),
            state: QueryState::Done,
            ..Self::accepted(query_id, query_string, accepted_at)
        }
    }

    /// Milliseconds from `from` to `to`, saturated into the modelled `Integer`. A `to`
    /// before `from` is zero rather than negative: both bounds are instants this record
    /// stores, and a negative wait is not a value the member can carry.
    fn millis_between(from: SystemTime, to: SystemTime) -> i32 {
        let millis = to.duration_since(from).map_or(0, |d| d.as_millis());
        i32::try_from(millis.min(i32::MAX as u128)).unwrap_or(i32::MAX)
    }

    /// `QueryEvalStats$waited`: how long the query waited before evaluation began, or
    /// how long it has been waiting so far while it is still queued.
    ///
    /// A query cancelled while still queued never started, but its wait DID end — at the
    /// instant it finished — so the span closes there rather than following the clock for
    /// ever.
    pub fn waited_ms(&self, now: SystemTime) -> i32 {
        let waited_until = self.started_at.or(self.finished_at).unwrap_or(now);
        Self::millis_between(self.accepted_at, waited_until)
    }

    /// `QueryEvalStats$elapsed`: how long the query has been evaluating. Zero while it
    /// has not started, and frozen at the instant it finished.
    pub fn elapsed_ms(&self, now: SystemTime) -> i32 {
        match self.started_at {
            None => 0,
            Some(started) => Self::millis_between(started, self.finished_at.unwrap_or(now)),
        }
    }

    /// This record's `QueryEvalStats`, as of `now`.
    pub fn eval_stats(&self, now: SystemTime) -> QueryEvalStats {
        QueryEvalStats {
            waited: Some(self.waited_ms(now)),
            elapsed: Some(self.elapsed_ms(now)),
            cancelled: Some(self.cancelled),
            subqueries: Some(::aws_smithy_types::Document::Number(
                ::aws_smithy_types::Number::PosInt(self.subqueries),
            )),
        }
    }

    /// The `GremlinQueryStatus` projection `ListGremlinQueries` and
    /// `GetGremlinQueryStatus` report.
    pub fn status(&self, now: SystemTime) -> GremlinQueryStatus {
        GremlinQueryStatus {
            query_id: Some(self.query_id.clone()),
            query_string: Some(self.query_string.clone()),
            query_eval_stats: Some(self.eval_stats(now)),
        }
    }

    /// Cancel this query, unless it has already finished.
    ///
    /// Returns whether this call is what cancelled it, so a caller can tell a first
    /// cancel from a repeat. The record is KEPT either way: `GetGremlinQueryStatus` has
    /// to go on reporting `cancelled` for the id afterwards, and keeping it is also what
    /// makes the second cancel of one id answer exactly like the first.
    pub fn cancel(&mut self, now: SystemTime) -> bool {
        if self.state.is_terminal() {
            return false;
        }
        self.state = QueryState::Cancelled;
        self.cancelled = true;
        self.finished_at = Some(now);
        true
    }
}

/// One query language's registry: every query of that language the engine has accepted,
/// keyed by query id.
///
/// The two query languages keep SEPARATE registries, because each has its own status
/// family: a `queryId` the openCypher endpoints minted is not one the Gremlin status
/// reports on, and the other way round. What a registry holds is the same either way —
/// the model projects an openCypher query's status through the same `GremlinQueryStatus`
/// shape (`OpenCypherQueries`'s member), so one record type serves both.
///
/// Derefs to the map it holds, so a record is reached by id exactly as before.
#[derive(Debug, Default)]
pub struct QueryRegistry {
    /// A `BTreeMap` is what gives a list a stable order over the ids.
    queries: BTreeMap<String, GremlinQueryRecord>,
}

impl ::std::ops::Deref for QueryRegistry {
    type Target = BTreeMap<String, GremlinQueryRecord>;

    fn deref(&self) -> &Self::Target {
        &self.queries
    }
}

impl ::std::ops::DerefMut for QueryRegistry {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.queries
    }
}

impl QueryRegistry {
    /// How many accepted-but-not-completed queries this registry holds — what
    /// `acceptedQueryCount` counts ("accepted but not yet completed, including queries in
    /// the queue").
    pub fn accepted_count(&self) -> usize {
        self.queries
            .values()
            .filter(|query| query.state.is_accepted())
            .count()
    }

    /// How many of its queries are evaluating right now, which is what
    /// `runningQueryCount` counts.
    pub fn running_count(&self) -> usize {
        self.queries
            .values()
            .filter(|query| query.state.is_running())
            .count()
    }

    /// The current queries, oldest id first: the running ones, plus the queued ones when
    /// `include_waiting` is set.
    pub fn current(&self, include_waiting: bool, now: SystemTime) -> Vec<GremlinQueryStatus> {
        self.queries
            .values()
            .filter(|query| {
                query.state.is_running() || (include_waiting && query.state == QueryState::Waiting)
            })
            .map(|query| query.status(now))
            .collect()
    }

    /// Record a query this engine accepted, answered and finished within the call that
    /// submitted it.
    ///
    /// The finished record is KEPT, so the status family can still report the query the
    /// caller submitted — bounded by [`FINISHED_QUERY_HISTORY`], so a stream of queries
    /// cannot grow the registry without end.
    pub fn record_finished(
        &mut self,
        query_id: impl Into<String>,
        query_string: impl Into<String>,
        accepted_at: SystemTime,
        finished_at: SystemTime,
    ) {
        let record = GremlinQueryRecord::finished(query_id, query_string, accepted_at, finished_at);
        self.queries.insert(record.query_id.clone(), record);
        self.forget_oldest_finished();
    }

    /// Cancel every query the engine still owes an answer, as of `now`, and apply the
    /// registry's bound to the records that just became finished.
    pub fn cancel_all(&mut self, now: SystemTime) {
        for query in self.queries.values_mut() {
            query.cancel(now);
        }
        self.forget_oldest_finished();
    }

    /// Drop the finished queries past [`FINISHED_QUERY_HISTORY`], oldest first.
    ///
    /// Only FINISHED queries are forgotten: a query the engine still owes an answer is
    /// one the status family and a cancel have to go on reaching, however long the
    /// registry grows.
    fn forget_oldest_finished(&mut self) {
        let mut finished: Vec<(SystemTime, String)> = self
            .queries
            .values()
            .filter(|query| query.state.is_terminal())
            .map(|query| {
                (
                    query.finished_at.unwrap_or(query.accepted_at),
                    query.query_id.clone(),
                )
            })
            .collect();
        if finished.len() <= FINISHED_QUERY_HISTORY {
            return;
        }
        // Oldest first, ties broken by query id, so the same records are forgotten
        // whatever order the registry is walked in.
        finished.sort();
        let surplus = finished.len() - FINISHED_QUERY_HISTORY;
        for (_, query_id) in finished.iter().take(surplus) {
            self.queries.remove(query_id);
        }
    }
}

/// The fast-reset state of the database this engine serves.
///
/// Fast reset is a two-step command: `initiateDatabaseReset` mints the token, and only a
/// `performDatabaseReset` presenting that token empties the database. The token lives
/// here because it is what the two steps share: the engine, not the caller, decides which
/// token authorises a reset.
#[derive(Debug, Default, Clone)]
pub struct FastResetRecord {
    /// The token the latest initiate minted and no reset has consumed yet. `None` while no
    /// reset is outstanding, which is what makes every perform refusable.
    pub pending_token: Option<String>,
    /// When that token was minted; `None` whenever [`Self::pending_token`] is.
    ///
    /// The token does not expire: a test initiates a reset and performs it when it is
    /// ready, and a validity window would make that a race against a clock. The instant
    /// is kept because it is what the engine knows about the outstanding token.
    pub token_issued_at: Option<SystemTime>,
    /// How many resets this engine has performed.
    pub resets_performed: u64,
    /// When the latest reset was performed; `None` until one has been.
    pub last_reset_at: Option<SystemTime>,
}

impl FastResetRecord {
    /// Record `token` as the one token a reset may present.
    ///
    /// A second initiate supersedes the first: the store holds ONE outstanding token, so
    /// the token the latest initiate handed back is the only one that can authorise a
    /// reset.
    pub fn initiate(&mut self, token: impl Into<String>, now: SystemTime) {
        self.pending_token = Some(token.into());
        self.token_issued_at = Some(now);
    }

    /// Whether `token` is the outstanding token. False when nothing is outstanding, so a
    /// reset nobody initiated cannot be performed.
    pub fn authorises(&self, token: &str) -> bool {
        self.pending_token.as_deref() == Some(token)
    }

    /// Consume the outstanding token and record the reset it authorised. The token is
    /// single use: performing the reset leaves nothing outstanding, so the same token
    /// cannot empty the database twice.
    fn consume(&mut self, now: SystemTime) {
        self.pending_token = None;
        self.token_issued_at = None;
        self.resets_performed = self.resets_performed.saturating_add(1);
        self.last_reset_at = Some(now);
    }
}

/// Per-service state for com-amazonaws-neptunedata-amazonneptunedataplane. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsNeptunedataAmazonneptunedataplaneState {
    /// The one engine record `GetEngineStatus` reports, built when this state is first
    /// reached.
    pub engine_status: EngineStatusRecord,
    /// Every Gremlin query the engine has accepted, keyed by query id. The Gremlin status
    /// family reports from this one.
    pub gremlin_queries: QueryRegistry,
    /// Every openCypher query the engine has accepted, keyed by query id. The openCypher
    /// status family reports from this one: a `queryId` one language minted is not one the
    /// other's status reports on.
    pub open_cypher_queries: QueryRegistry,
    /// The fast-reset token and reset history `ExecuteFastReset` reads and writes.
    pub fast_reset: FastResetRecord,
    /// Every bulk-load job the engine has admitted, keyed by load id. `ListLoaderJobs`
    /// reports the load ids of the active ones.
    pub loader_jobs: BTreeMap<String, LoaderJobRecord>,
    /// The nodes and relationships the openCypher endpoints read and write — this
    /// engine's one copy of the graph's data.
    pub graph: PropertyGraph,
    /// The Neptune ML resources this engine holds, which the ML endpoints read and write.
    pub ml: MlRegistry,
    /// Every index rebuild the storage layer has admitted, keyed by rebuild id. The three
    /// index-rebuild endpoints read and write this one.
    pub index_rebuilds: IndexRebuildRegistry,
    /// The DFE statistics this engine holds for the property graph, which the
    /// property-graph statistics endpoints read and write. `None` while the engine keeps
    /// no statistics state for that data — before any management call has started one, and
    /// again once a delete has removed it — which is what makes a read of statistics that
    /// are not available a refusal rather than an answer.
    pub propertygraph_statistics: Option<StatisticsRecord>,
}

impl ComAmazonawsNeptunedataAmazonneptunedataplaneState {
    /// The load ids the loader job list reports — the jobs the registry holds, newest
    /// admission first, at most `limit` of them.
    ///
    /// RECORDED BEHAVIOUR: the endpoint's own documentation reads "a list of the `loadIds`
    /// for all active loader jobs", but the load id of a job that has FINISHED is one the
    /// endpoint answers with — the loader keeps the most recent loads
    /// ([`LOADER_JOB_HISTORY`]) and lists the ones it keeps, which is what lets a client
    /// find the id of a load it started and then read that load's status. So every job the
    /// registry holds is listed.
    ///
    /// `include_queued` keeps the queued load requests among them, which is what
    /// `includeQueuedLoads` chooses — the member "can be used to exclude the load IDs of
    /// queued load requests". Jobs admitted in the same instant are ordered by load id, so
    /// the order is total and `limit` takes the same prefix on every read.
    pub fn loader_job_ids(&self, include_queued: bool, limit: usize) -> Vec<String> {
        let mut listed: Vec<&LoaderJobRecord> = self
            .loader_jobs
            .values()
            .filter(|job| include_queued || !job.state.is_queued())
            .collect();
        listed.sort_by(|left, right| {
            right
                .admitted_at
                .cmp(&left.admitted_at)
                .then_with(|| left.load_id.cmp(&right.load_id))
        });
        listed
            .into_iter()
            .take(limit)
            .map(|job| job.load_id.clone())
            .collect()
    }

    /// Apply `mode` to the property-graph statistics, and report the id of the generation
    /// run a `refresh` performed — `None` for a mode that only sets how statistics are
    /// generated, which is what makes the payload "only returned for refresh mode".
    ///
    /// A management call is what starts the statistics state of this data: a record the
    /// engine keeps none for is created here, carrying the mode this call set. A `refresh`
    /// counts the graph's characteristic sets as of `now` and records the run, so what a
    /// later read reports follows from what the graph held when the run happened.
    pub fn manage_propertygraph_statistics(
        &mut self,
        mode: StatisticsMode,
        now: SystemTime,
    ) -> Option<String> {
        match mode {
            StatisticsMode::DisableAutoCompute => {
                self.propertygraph_statistics_record().auto_compute = false;
                None
            }
            StatisticsMode::EnableAutoCompute => {
                self.propertygraph_statistics_record().auto_compute = true;
                None
            }
            StatisticsMode::Refresh => {
                // The run reads the graph before the record is borrowed to write it, so one
                // call reads the graph once and writes the statistics once. Both of the
                // things a run counts — the characteristic sets the statistics endpoints
                // report and the summary the graph-summary endpoint reports — are counted
                // HERE, from the graph as it stands at `now`, because both are the snapshot
                // this one run took.
                let signatures = characteristic_sets(&self.graph);
                let summary = graph_summary(&self.graph);
                Some(
                    self.propertygraph_statistics_record()
                        .record_generation(signatures, summary, now),
                )
            }
        }
    }

    /// The property-graph statistics record to write, started here when the engine keeps
    /// none yet: a management call is what the statistics state of this data begins with.
    fn propertygraph_statistics_record(&mut self) -> &mut StatisticsRecord {
        self.propertygraph_statistics
            .get_or_insert_with(StatisticsRecord::default)
    }

    /// Delete the property-graph statistics, and report whether there were any to delete.
    ///
    /// `false` means the engine kept no statistics state for this data, which is what tells
    /// the delete there was nothing to delete. What is removed is the whole statistics state
    /// the endpoints report, so a read after it has nothing to answer from until the next
    /// management call starts a record again.
    pub fn delete_propertygraph_statistics(&mut self) -> bool {
        self.propertygraph_statistics.take().is_some()
    }

    /// Perform the fast reset `token` authorises, and report whether it did.
    ///
    /// `false` means `token` is not the outstanding one — nothing was reset, so a wrong
    /// token cannot empty the database. On a match the database is emptied: the graph's
    /// own data is dropped, every query the engine has accepted but not completed — in
    /// EITHER language — was asked about data the reset removes, so it is marked cancelled
    /// exactly as a cancel marks one (the record is KEPT, so the status families go on
    /// reporting those ids), and the token is consumed.
    pub fn perform_fast_reset(&mut self, token: &str, now: SystemTime) -> bool {
        if !self.fast_reset.authorises(token) {
            return false;
        }
        self.graph.clear();
        self.gremlin_queries.cancel_all(now);
        self.open_cypher_queries.cancel_all(now);
        self.fast_reset.consume(now);
        true
    }
}

/// A fresh id naming ONE response, for the `requestId` a modelled Neptune error carries.
///
/// The member documents it only as "the ID of the request", and the ids this data plane
/// answers one with are UUIDs, so one is minted here: the id names a request for
/// correlation, two responses never carry the same one, it is derived from nothing the
/// caller sent, and it is not a credential.
pub fn new_request_id() -> String {
    ::uuid::Uuid::new_v4().to_string()
}

/// A fresh fast-reset token: the UUID `ExecuteFastReset`'s `initiateDatabaseReset` action
/// hands back, which its own documentation describes as "a UUID token which you then
/// include when calling ExecuteFastReset again with action set to performDatabaseReset".
///
/// Version 4, so the value is unpredictable and derived from nothing the caller sent: it
/// is what authorises emptying the database, so a caller must not be able to guess the
/// token of a reset somebody else initiated.
pub fn new_fast_reset_token() -> String {
    ::uuid::Uuid::new_v4().to_string()
}

/// A fresh query id: the `queryId` a query status API reports for a query this engine has
/// accepted, and the label a cancel names it by.
///
/// Version 4, so the id is derived from nothing the caller sent and no caller can guess
/// the id of somebody else's query, which is what a cancel takes.
pub fn new_query_id() -> String {
    ::uuid::Uuid::new_v4().to_string()
}

/// The region this request's ARNs are minted in: the region of the caller's SigV4
/// credential scope, or [`DEFAULT_REGION`] when the request carries no scope to read one
/// from.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The region an ARN carries when the request named none.
pub const DEFAULT_REGION: &str = "us-east-1";

/// At most this many characters of a request-supplied value are echoed back in an error
/// message. Neptune names the offending value in its message, so echoing it is right, but
/// the length is the caller's choice and the message is not the place to carry it whole.
const ECHOED_VALUE_LIMIT: usize = 128;

/// A request-supplied value as an error message may carry it: control characters escaped
/// (so a message cannot forge a line in anything that renders it) and bounded in length.
pub fn echoed(value: &str) -> String {
    value.escape_debug().take(ECHOED_VALUE_LIMIT).collect()
}

/// The `InvalidParameterException` payload for a `queryId` the registry does not hold.
///
/// `detailedMessage` names the member and the value that was wrong, so the caller can fix
/// the request from it; the error CLASS a client branches on is written from the shape's own
/// name (see `crate::ops::types::error_message_wire`).
pub fn invalid_query_id(query_id: &str) -> InvalidParameterException {
    InvalidParameterException {
        detailed_message: format!("Supplied queryId {} is invalid", echoed(query_id)),
        request_id: new_request_id(),
    }
}

/// The `InvalidParameterException` payload for a request member carrying a number
/// outside the range the model publishes for it — a value the request cannot be served
/// with, so it is refused rather than quietly adjusted.
///
/// `member` is the modelled member's wire name and the bounds are the ones the model
/// declares, so the caller can fix the request from the message alone.
pub fn invalid_parameter_range(
    member: &str,
    value: i32,
    minimum: i32,
    maximum: i32,
) -> InvalidParameterException {
    InvalidParameterException {
        detailed_message: format!(
            "Supplied {member} {value} is invalid. {member} must be an integer \
             not less than {minimum} and not more than {maximum}"
        ),
        request_id: new_request_id(),
    }
}

/// The `MissingParameterException` payload for a parameter this request had to carry and
/// did not. `member` is the modelled member's wire name, so the caller can fix the
/// request from the message alone.
pub fn missing_parameter(member: &str) -> MissingParameterException {
    MissingParameterException {
        detailed_message: format!("Request is missing mandatory parameter '{member}'"),
        request_id: new_request_id(),
    }
}

/// The `BadRequestException` payload for a SYSTEM COMMAND that cannot be processed because
/// the member its action needs arrived empty: the engine reports the member it found no
/// value in, by its own name, and nothing else.
///
/// `BadRequestException` is the class — "raised when a request is submitted that cannot be
/// processed": the command names an action the engine supports and carries no value for it
/// to act on, so what is wrong is the request as a whole rather than a parameter outside
/// its bounds. `member` is the modelled member's wire name, which is the message's one
/// variable part.
pub fn member_is_null(member: &str) -> BadRequestException {
    BadRequestException {
        detailed_message: format!("{member} is null"),
        request_id: new_request_id(),
    }
}

/// The `InvalidParameterException` payload for a `token` that is not the one the
/// outstanding `initiateDatabaseReset` minted — including the case where no reset is
/// outstanding at all, and the case where the token has already been used.
///
/// The supplied value is deliberately NOT echoed: the token is what authorises emptying
/// the database, so the message names the member that was wrong and nothing else.
pub fn invalid_fast_reset_token() -> InvalidParameterException {
    InvalidParameterException {
        detailed_message: "Supplied token is invalid. Please initiate the database reset again."
            .to_string(),
        request_id: new_request_id(),
    }
}

/// The `IllegalArgumentException` payload for an `action` this service does not support,
/// naming the value that was wrong and the values the member accepts.
pub fn unsupported_action(action: &str, accepted: &[&str]) -> IllegalArgumentException {
    unsupported_member_value("action", action, accepted)
}

/// The `IllegalArgumentException` payload for a request member carrying a value this
/// service does not support, naming the member, the value that was wrong and the values the
/// member accepts, so the caller can fix the request from the message alone.
pub fn unsupported_member_value(
    member: &str,
    value: &str,
    accepted: &[&str],
) -> IllegalArgumentException {
    IllegalArgumentException {
        detailed_message: format!(
            "Supplied {member} {} is invalid. Accepted {member}s: {}",
            echoed(value),
            accepted.join(", ")
        ),
        request_id: new_request_id(),
    }
}

/// What a request that could not reach this service's store at all is told: a
/// server-side failure, not anything the caller sent. One statement of the message, so the
/// payloads below cannot drift apart.
const STORE_UNAVAILABLE_MESSAGE: &str = "the neptunedata engine store is unavailable";

/// The `FailureByQueryException` payload for a request that could not reach this
/// service's store.
pub fn store_unavailable_failure() -> FailureByQueryException {
    FailureByQueryException {
        detailed_message: STORE_UNAVAILABLE_MESSAGE.to_string(),
        request_id: new_request_id(),
    }
}

/// The same server-side failure for an operation that models `InternalFailureException`
/// instead.
pub fn store_unavailable_internal() -> InternalFailureException {
    InternalFailureException {
        detailed_message: STORE_UNAVAILABLE_MESSAGE.to_string(),
        request_id: new_request_id(),
    }
}

/// The same server-side failure for an operation whose only modelled server error is
/// `ServerShutdownException`.
pub fn store_unavailable_shutdown() -> ServerShutdownException {
    ServerShutdownException {
        detailed_message: STORE_UNAVAILABLE_MESSAGE.to_string(),
        request_id: new_request_id(),
    }
}

/// The same server-side failure for an operation that models no server error at all: the
/// ML endpoints model only client errors, and `BadRequestException` — "raised when a
/// request is submitted that cannot be processed" — is the one of them that states what
/// happened without blaming a member of the request.
pub fn store_unavailable_bad_request() -> BadRequestException {
    BadRequestException {
        detailed_message: STORE_UNAVAILABLE_MESSAGE.to_string(),
        request_id: new_request_id(),
    }
}

/// The `MissingParameterException` payload for a request that had to carry ONE of several
/// parameters and carried none of them.
pub fn missing_one_of_parameters(members: &[&str]) -> MissingParameterException {
    let named: Vec<String> = members.iter().map(|member| format!("'{member}'")).collect();
    MissingParameterException {
        detailed_message: format!(
            "Request is missing mandatory parameter {}",
            named.join(" or ")
        ),
        request_id: new_request_id(),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsNeptunedataAmazonneptunedataplaneState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsNeptunedataAmazonneptunedataplaneState>)
}
