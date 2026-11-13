// SPDX-License-Identifier: CC0-1.0

//! The EXECUTIONS of a scheduled query — the runs its own schedule makes due — as they are
//! recorded, run and reported.
//!
//! `CreateScheduledQuery` creates a query that "runs CloudWatch Logs Insights queries at
//! regular intervals", so a stored query produces runs with nothing else being called:
//! `GetScheduledQueryHistory` "retrieves the execution history of a scheduled query", and
//! every record it reports is one of those runs.
//!
//! WHEN a run happens is therefore owed by the CLOCK rather than by a call, exactly as a
//! started query's status is: the instants a query's own `scheduleExpression` names are walked
//! from the instant the query became eligible to run — the later of when it was created and
//! its `scheduleStartTime` ("The query will not execute before this time") — and each one that
//! has already passed is a run that was owed at that instant even if nobody had looked
//! ([`due_instants`]). A read of the history MATERIALISES those runs; nothing is slept on and
//! no run is ever brought forward because something was read. A query that is `DISABLED` runs
//! nothing, and neither does one past its `scheduleEndTime` ("The query will stop executing
//! after this time").
//!
//! WHAT a run produces is a real Insights query over this account's own log events:
//!
//! * the run mints a query the store then holds ([`super::QueryRecord`]) over the scheduled
//!   query's own string, log groups and lookback window, created AT the instant the run was
//!   triggered — so the `queryId` the history reports is a handle `GetQueryResults` answers
//!   that run's rows from, and the rows are the events of the window rather than of whenever
//!   somebody looked;
//! * the query is then RUN ([`super::insights`]) and its results recorded on it, which is what
//!   fixes the rows that run answers from then on and what the destinations below deliver;
//! * the execution's own status is the status that run reached: `Complete` for a run that
//!   produced its results, and `InvalidQuery` for a query string this service cannot run at
//!   all, carrying the reason as the record's `errorMessage`.
//!
//! WHERE the results go follows the query's `destinationConfiguration`, and a destination is
//! reported as processed only once the delivery really happened:
//!
//! * a lookup-table destination "automatically populate[s] or refresh[es] the specified lookup
//!   table with query results on each scheduled execution", which is a resource of THIS
//!   service: the table is created or replaced in this very store, so a later
//!   `GetLookupTable` reads the rows the run produced;
//! * an Amazon S3 destination is "the Amazon S3 URI where query results are delivered": the
//!   results are written there as an object of the object store, over the in-process callback
//!   the way every other hop of this service to a sibling service is made, so nothing opens a
//!   socket or leaves the host.
//!
//! WHAT BOUNDS IT: one history read materialises at most [`MAX_CATCHUP_INSTANTS`] schedule
//! instants, keeps at most [`MAX_TRIGGER_HISTORY_RECORDS`] of them per query and carries out at
//! most [`MAX_RUNS_PER_READ`] runs, each spending the same bounded budget every Insights run
//! spends; and each run lands once — a second read of the same execution re-runs nothing.
//!
//! WHICH UNIT the instants are stated in follows the members themselves: a scheduled query's
//! schedule window (`scheduleStartTime`, `scheduleEndTime`) and its history's window and
//! `triggeredTimestamp` are all stated "in Unix epoch format", which on this surface is the
//! epoch SECONDS its windows and offsets are stated in (`startTimeOffset`, `endTimeOffset`,
//! `StartQuery$startTime`) — while the instants of the RESOURCE (`creationTime`,
//! `lastUpdatedTime`, `lastTriggeredTime`) are the epoch milliseconds every record of this
//! service keeps. The store keeps every instant in milliseconds and converts at the edge
//! ([`ScheduledQueryExecution::triggered_secs`], [`schedule_instant_ms`]).

use ::std::sync::Arc;

use ::smithy_server_runtime::EmbeddedEmulator;

use super::callback_client::callback_client;
use super::ComAmazonawsCloudwatchlogsLogs20140328State as State;
use super::ScheduledQueryRecord;
use crate::ops::types::{LookupTableConfiguration, ScheduledQueryDestination, TriggerHistoryRecord};

/// A run that has been triggered and has not produced its results yet, from the model's
/// `ExecutionStatus` enum (`Running` | `InvalidQuery` | `Complete` | `Failed` | `Timeout`).
pub const EXECUTION_STATUS_RUNNING: &str = "Running";

/// A run that produced its results, from the model's `ExecutionStatus` enum.
pub const EXECUTION_STATUS_COMPLETE: &str = "Complete";

/// A run whose query string this service cannot execute over the events it holds, from the
/// model's `ExecutionStatus` enum: the query is invalid rather than the run having failed on
/// the way, which is why the reason is reported as the record's `errorMessage`.
pub const EXECUTION_STATUS_INVALID_QUERY: &str = "InvalidQuery";

/// A destination whose delivery is still owed, from the model's `ActionStatus` enum
/// (`IN_PROGRESS` | `CLIENT_ERROR` | `FAILED` | `COMPLETE`).
pub const DESTINATION_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// A destination the results were delivered to, from the model's `ActionStatus` enum.
pub const DESTINATION_STATUS_COMPLETE: &str = "COMPLETE";

/// A destination the results could not be delivered to, from the model's `ActionStatus` enum.
pub const DESTINATION_STATUS_FAILED: &str = "FAILED";

/// An Amazon S3 destination, from the model's `ScheduledQueryDestinationType` enum (`S3` |
/// `LOOKUP_TABLE`).
pub const DESTINATION_TYPE_S3: &str = "S3";

/// A lookup-table destination, from the model's `ScheduledQueryDestinationType` enum.
pub const DESTINATION_TYPE_LOOKUP_TABLE: &str = "LOOKUP_TABLE";

/// How long after a scheduled query becomes eligible to run its FIRST run falls due.
///
/// AWS documents no delay before the first run of a scheduled query — only the schedule that
/// governs the runs after it — so this span is THE EMULATOR'S OWN CHOICE for the transition the
/// clock owes a newly created query. It is deliberately not zero, so a caller that reads the
/// history straight after creating a query still sees the query before its first run; every
/// run after the first falls at the instant the query's OWN `scheduleExpression` names
/// ([`super::Schedule::next_after`]). The instant is derived and never slept on.
pub const FIRST_EXECUTION_DELAY_MS: i64 = 3_000;

/// THE EMULATOR'S OWN CEILING on the execution records one scheduled query keeps, not a
/// retention cited from AWS: a query keeps running for as long as the process does, so the
/// oldest records are dropped once it holds this many rather than growing without bound.
pub const MAX_TRIGGER_HISTORY_RECORDS: usize = 100;

/// The most schedule instants ONE read walks while catching a query up with the clock, so a
/// query whose schedule has named very many instants since it was last read costs a bounded
/// amount of work. What the walk does not reach is caught up by the next read.
const MAX_CATCHUP_INSTANTS: usize = 10_000;

/// The most runs ONE read carries out — the bound on the PRODUCT of how many runs a read can
/// owe and the budget each Insights run spends ([`super::insights::MAX_EXECUTION_TIME`] and its
/// companions), so a query read after a long gap cannot turn one request into unbounded work.
///
/// A run a read does not reach stays owed: its record is already in the history, its own query
/// answers that run's rows when they are asked for ([`super::QueryRecord`]), and the next read
/// settles it.
const MAX_RUNS_PER_READ: usize = 10;

/// Why a destination of a run the store no longer holds the query of was not delivered to: the
/// rows that run produced are no longer there to deliver, because the store dropped its query
/// to stay inside its own ceiling ([`super::MAX_QUERIES`]).
const RESULTS_NO_LONGER_HELD_MESSAGE: &str =
    "the results of this execution are no longer held by the service";

/// One recorded execution of a scheduled query — one `TriggerHistoryRecord`, "a record of a
/// scheduled query execution, including execution status, timestamp, and destination
/// processing results" — as it is stored.
#[derive(Debug, Clone)]
pub struct ScheduledQueryExecution {
    /// `TriggerHistoryRecord$queryId` — "The unique identifier for this query execution",
    /// which is the id the store holds this run's query under, so `GetQueryResults` answers
    /// this run's own rows under it.
    pub query_id: String,
    /// `TriggerHistoryRecord$triggeredTimestamp` — "The timestamp when the scheduled query
    /// execution was triggered". Kept in the epoch MILLISECONDS the run's own query is created
    /// at; the history reports it in the epoch seconds that surface states its instants in
    /// ([`Self::triggered_secs`]).
    pub triggered_ms: i64,
    /// `TriggerHistoryRecord$executionStatus` — "The execution status of the scheduled query
    /// run", one of the [`EXECUTION_STATUS_RUNNING`] family.
    pub status: String,
    /// `TriggerHistoryRecord$errorMessage` — "Error message if the query execution failed",
    /// and nothing at all for a run that did not.
    pub error_message: Option<String>,
    /// `TriggerHistoryRecord$destinations` — "Information about destination processing for
    /// this query execution", one entry per destination the query is configured with.
    pub destinations: Vec<ScheduledQueryDestination>,
    /// Whether this execution's RUN has landed: its results recorded and each destination
    /// processed. A read settles an execution once; one already landed re-runs nothing.
    pub run_landed: bool,
    /// Whether a read has CLAIMED this execution's run. A run is carried out with the store's
    /// guard released, so the read that takes it on claims it inside the same critical section
    /// that found it owed: a second read overlapping the first finds nothing owed here and the
    /// same run cannot be carried out, or delivered, twice.
    pub run_claimed: bool,
}

impl ScheduledQueryExecution {
    /// A run triggered at `triggered_ms`, whose query the store holds under `query_id`, with
    /// the destinations `configuration` names still owed.
    pub fn new(
        query_id: String,
        triggered_ms: i64,
        configuration: Option<&crate::ops::types::DestinationConfiguration>,
    ) -> Self {
        Self {
            query_id,
            triggered_ms,
            // The run's own results have not landed yet, so the execution starts in the
            // status of a run in flight.
            status: EXECUTION_STATUS_RUNNING.to_string(),
            error_message: None,
            destinations: owed_destinations(configuration),
            run_landed: false,
            run_claimed: false,
        }
    }

    /// The instant this run was triggered, in the epoch SECONDS this operation states its
    /// instants in: `triggeredTimestamp`, and the `startTime`/`endTime` of the window a read
    /// selects by, are "in Unix epoch format" — the unit this same surface states every window
    /// and offset of a query in (`startTimeOffset`, `endTimeOffset`, `StartQuery$startTime`).
    pub fn triggered_secs(&self) -> i64 {
        self.triggered_ms.div_euclid(MILLIS_PER_SECOND)
    }

    /// This execution as the model's `TriggerHistoryRecord`, which is what
    /// `GetScheduledQueryHistory` reports — so one stored execution renders the same way
    /// through any reader of it.
    pub fn reported(&self) -> TriggerHistoryRecord {
        TriggerHistoryRecord {
            query_id: Some(self.query_id.clone()),
            execution_status: Some(self.status.clone()),
            triggered_timestamp: Some(self.triggered_secs()),
            error_message: self.error_message.clone(),
            // A query configured with no destination delivers to none, so the member is
            // reported by nothing rather than as an empty list.
            destinations: (!self.destinations.is_empty()).then(|| self.destinations.clone()),
        }
    }

    /// Record `outcome` on the destination of `destination_type`, which is what makes a
    /// delivery that happened reportable as processed.
    fn settle(&mut self, destination_type: &str, outcome: DeliveryOutcome) {
        for destination in self
            .destinations
            .iter_mut()
            .filter(|destination| destination.destination_type.as_deref() == Some(destination_type))
        {
            destination.status = Some(outcome.status.to_string());
            destination.processed_identifier = outcome.processed_identifier.clone();
            destination.error_message = outcome.error_message.clone();
        }
    }
}

/// The destinations an execution of a query configured with `configuration` owes, each still
/// in progress: a destination is reported as processed only once its delivery has happened.
fn owed_destinations(
    configuration: Option<&crate::ops::types::DestinationConfiguration>,
) -> Vec<ScheduledQueryDestination> {
    let mut owed = Vec::new();
    let Some(configuration) = configuration else {
        return owed;
    };
    if let Some(s3) = &configuration.s3_configuration {
        owed.push(ScheduledQueryDestination {
            destination_type: Some(DESTINATION_TYPE_S3.to_string()),
            // "The identifier for the destination where results are delivered", which for
            // Amazon S3 is the URI the configuration names.
            destination_identifier: Some(s3.destination_identifier.clone()),
            status: Some(DESTINATION_STATUS_IN_PROGRESS.to_string()),
            processed_identifier: None,
            error_message: None,
        });
    }
    if let Some(table) = &configuration.lookup_table_configuration {
        owed.push(ScheduledQueryDestination {
            destination_type: Some(DESTINATION_TYPE_LOOKUP_TABLE.to_string()),
            destination_identifier: Some(table.table_name.clone()),
            status: Some(DESTINATION_STATUS_IN_PROGRESS.to_string()),
            processed_identifier: None,
            error_message: None,
        });
    }
    owed
}

/// What processing one destination produced: the status it reached, the identifier of what it
/// produced there, and why it did not, when it did not.
#[derive(Debug, Clone)]
struct DeliveryOutcome {
    /// One of the [`DESTINATION_STATUS_COMPLETE`] family.
    status: &'static str,
    /// `ScheduledQueryDestination$processedIdentifier` — "The identifier of the processed
    /// result at the destination", for a delivery that produced one.
    processed_identifier: Option<String>,
    /// `ScheduledQueryDestination$errorMessage` — "Error message if destination processing
    /// failed".
    error_message: Option<String>,
}

impl DeliveryOutcome {
    /// A delivery that happened, naming what it produced at the destination.
    fn complete(processed_identifier: Option<String>) -> Self {
        Self {
            status: DESTINATION_STATUS_COMPLETE,
            processed_identifier,
            error_message: None,
        }
    }

    /// A delivery that did not happen, carrying why.
    fn failed(error_message: String) -> Self {
        Self {
            status: DESTINATION_STATUS_FAILED,
            processed_identifier: None,
            error_message: Some(error_message),
        }
    }
}

impl ScheduledQueryRecord {
    /// Record `execution` as the newest execution of this query, which also makes it the
    /// query's last triggered run (`lastTriggeredTime`, `lastExecutionStatus` — both report an
    /// execution that happened).
    ///
    /// The history is kept oldest first and bounded by [`MAX_TRIGGER_HISTORY_RECORDS`]: the
    /// oldest record is dropped rather than the store growing without bound.
    pub fn record_execution(&mut self, execution: ScheduledQueryExecution) {
        self.last_triggered_time_ms = Some(execution.triggered_ms);
        self.last_execution_status = Some(execution.status.clone());
        self.trigger_history.push(execution);
        while self.trigger_history.len() > MAX_TRIGGER_HISTORY_RECORDS {
            self.trigger_history.remove(0);
        }
    }

    /// The execution this query holds under `query_id`, to write.
    fn execution_mut(&mut self, query_id: &str) -> Option<&mut ScheduledQueryExecution> {
        self.trigger_history
            .iter_mut()
            .find(|execution| execution.query_id == query_id)
    }
}

/// The instants the clock has ALREADY made due for `record`, oldest first, as of `now_ms`.
///
/// A run is due at an instant the query's own `scheduleExpression` names, from the instant it
/// became eligible to run — the later of its creation and its `scheduleStartTime` — with the
/// first one [`FIRST_EXECUTION_DELAY_MS`] after that instant. A query that is not `ENABLED`
/// runs nothing ("The initial state of the scheduled query"), one whose `scheduleEndTime` has
/// passed stops running after it, and one whose owner records no creation instant at all is a
/// query the SERVICE manages rather than one this store runs
/// ([`super::log_alarm_queries`]), so none of them has an instant due here.
///
/// The two ends of the schedule window are the caller's own "Unix epoch format" instants, so
/// they are read in the epoch SECONDS this surface states its instants in
/// ([`ScheduledQueryExecution::triggered_secs`]).
fn due_instants(record: &ScheduledQueryRecord, now_ms: i64) -> Vec<i64> {
    let Some(created_ms) = record.creation_time_ms else {
        return Vec::new();
    };
    if record.state != super::SCHEDULED_QUERY_STATE_ENABLED {
        return Vec::new();
    }
    let Some(schedule) = super::schedule_of(&record.schedule_expression) else {
        return Vec::new();
    };
    let eligible_from = schedule_instant_ms(record.schedule_start_time)
        .unwrap_or(created_ms)
        .max(created_ms);
    let stops_after = schedule_instant_ms(record.schedule_end_time);
    let timezone = record.timezone.as_deref();
    let mut candidate = match record.last_triggered_time_ms {
        // Every run after the first falls at the next instant the schedule names after the
        // run before it.
        Some(last) => schedule.next_after(last, timezone),
        None => Some(eligible_from.saturating_add(FIRST_EXECUTION_DELAY_MS)),
    };
    let mut due: ::std::collections::VecDeque<i64> = ::std::collections::VecDeque::new();
    let mut walked = 0usize;
    while let Some(instant) = candidate {
        if instant > now_ms || stops_after.is_some_and(|end| instant > end) {
            break;
        }
        due.push_back(instant);
        // Only the newest records are kept, so a query caught up after a long gap records the
        // runs it ends on rather than every run it ever owed.
        while due.len() > MAX_TRIGGER_HISTORY_RECORDS {
            due.pop_front();
        }
        walked = walked.saturating_add(1);
        if walked >= MAX_CATCHUP_INSTANTS {
            break;
        }
        candidate = schedule.next_after(instant, timezone);
    }
    due.into()
}

/// The window one run triggered at `triggered_ms` reads, in the epoch SECONDS a stored query
/// keeps its range in.
///
/// Both ends are offsets back from the instant the run was triggered: `startTimeOffset` "the
/// time offset in seconds that defines the lookback period for the query [...] how far back in
/// time the query searches from the execution time", and `endTimeOffset` "the time offset in
/// seconds that defines the end of the lookback period". A query that states no end offset
/// ends its window at the execution instant ([`super::DEFAULT_END_TIME_OFFSET_SECONDS`]); one
/// that states no lookback at all reads the period since the run before it, which for a
/// schedule naming instants at regular intervals is the gap between two of them — so
/// consecutive runs of such a query tile the time line rather than reading nothing.
fn execution_window_secs(
    record: &ScheduledQueryRecord,
    triggered_ms: i64,
    schedule: &super::Schedule,
) -> (i64, i64) {
    let end_offset_secs = record
        .end_time_offset
        .unwrap_or(super::DEFAULT_END_TIME_OFFSET_SECONDS);
    let start_offset_secs = record.start_time_offset.unwrap_or_else(|| {
        schedule
            .interval_after(triggered_ms, record.timezone.as_deref())
            .unwrap_or(0)
            / MILLIS_PER_SECOND
    });
    let end_ms = triggered_ms.saturating_sub(end_offset_secs.saturating_mul(MILLIS_PER_SECOND));
    let start_ms = triggered_ms.saturating_sub(start_offset_secs.saturating_mul(MILLIS_PER_SECOND));
    (start_ms / MILLIS_PER_SECOND, end_ms / MILLIS_PER_SECOND)
}

/// The milliseconds one second holds, the conversion between the epoch seconds this surface
/// states a scheduled query's windows and instants in and the epoch milliseconds the store
/// keeps them in.
const MILLIS_PER_SECOND: i64 = 1_000;

/// One end of a scheduled query's schedule window as an instant of the store's clock:
/// `scheduleStartTime` and `scheduleEndTime` are stated "in Unix epoch format", the epoch
/// seconds this surface states its instants in, so the stored value is read as seconds.
fn schedule_instant_ms(stated: Option<i64>) -> Option<i64> {
    stated.map(|seconds| seconds.saturating_mul(MILLIS_PER_SECOND))
}

impl State {
    /// Materialise the executions the clock has ALREADY made due for the scheduled query
    /// `identifier` names, as of `now_ms`, answering the `queryId` of every recorded execution
    /// whose run has not landed yet — oldest first.
    ///
    /// Each materialised run mints the query it ran ([`super::QueryRecord`]) over the scheduled
    /// query's own string, log groups and lookback window, created AT the instant the run was
    /// triggered: that query has already run, so the `queryId` the history reports is the
    /// handle `GetQueryResults` answers that run's rows from.
    pub fn materialize_due_scheduled_query_executions(
        &mut self,
        identifier: &str,
        now_ms: i64,
    ) -> Vec<String> {
        let Some(record) = self.scheduled_query(identifier).cloned() else {
            return Vec::new();
        };
        let Some(schedule) = super::schedule_of(&record.schedule_expression) else {
            return Vec::new();
        };
        for triggered_ms in due_instants(&record, now_ms) {
            let query_id = super::mint_query_id(self.take_query_seq());
            let (start_secs, end_secs) = execution_window_secs(&record, triggered_ms, &schedule);
            let mut query = super::QueryRecord::new(
                &query_id,
                &record.query_language,
                &record.query_string,
                run_log_group_names(&record),
                start_secs,
                end_secs,
                triggered_ms,
            );
            // The run happened AT the instant the schedule made it due, so the query it minted
            // is not one waiting on the clock to run: it is one that has run, whose results
            // this same call records on it ([`settle`]).
            query.status = super::QUERY_STATUS_COMPLETE.to_string();
            query.running_due_ms = None;
            query.complete_due_ms = None;
            self.queries.insert(query_id.clone(), query);
            self.enforce_query_ceiling();
            let mut execution = ScheduledQueryExecution::new(
                query_id,
                triggered_ms,
                record.destination_configuration.as_ref(),
            );
            // The run has produced its results; a query string this service cannot run at all
            // is what turns that into `InvalidQuery` once the run lands ([`record_run`]).
            execution.status = EXECUTION_STATUS_COMPLETE.to_string();
            if let Some(held) = self.scheduled_query_mut(identifier) {
                held.record_execution(execution);
            }
        }
        // The runs this read TAKES ON, claimed in the same critical section that found them
        // owed: the run itself happens with the guard released, so a read overlapping this one
        // must not carry out the same run a second time. At most
        // [`MAX_RUNS_PER_READ`] of them; the rest stay owed for the next read.
        let mut owed: Vec<String> = Vec::new();
        if let Some(held) = self.scheduled_query_mut(identifier) {
            for execution in held
                .trigger_history
                .iter_mut()
                .filter(|execution| !execution.run_landed && !execution.run_claimed)
                .take(MAX_RUNS_PER_READ)
            {
                execution.run_claimed = true;
                owed.push(execution.query_id.clone());
            }
        }
        owed
    }
}

/// The log groups one run of `record` reads, resolved to the names the store keys log groups
/// by: "An array of log group names or ARNs to query", so either form names the same group. A
/// query that names none leaves them to its own query string, exactly as a started query whose
/// `SOURCE` command selects them does.
fn run_log_group_names(record: &ScheduledQueryRecord) -> Vec<String> {
    let mut names: Vec<String> = Vec::with_capacity(record.log_group_identifiers.len());
    for identifier in &record.log_group_identifiers {
        let name = super::log_group_name_from_identifier(identifier).to_string();
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// Materialise, run and deliver every execution the clock owes the scheduled query
/// `identifier` names.
///
/// This is what makes a read of the history report runs that really happened: the runs the
/// clock already owed are recorded, each one's query is RUN over the events of its own window
/// and its results recorded on it, and each destination the query is configured with is
/// delivered to before it is reported as processed.
///
/// The store's guard is never held across a run or a delivery: everything a run needs is read
/// under it ([`super::insights::prepare`]), the run and the Amazon S3 write then happen with it
/// released, and it is taken again only to record what each produced.
pub async fn settle<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
    callback: &EmbeddedEmulator,
    region: &str,
    identifier: &str,
) {
    // The query's record lives in this very store, so a store that is not there holds no
    // query whose runs are owed.
    let Some(state) = super::service_state(&**engine) else {
        return;
    };
    let owed = state
        .lock()
        .materialize_due_scheduled_query_executions(identifier, super::now_ms());

    for query_id in owed {
        // Everything the run needs is read under the guard, which is then released: the query
        // below runs with no lock held.
        let prepared = {
            let store = state.lock();
            store
                .queries
                .get(&query_id)
                .map(|query| super::insights::prepare(&store, query))
        };
        // A query the store no longer holds was dropped by its ceiling, so there is no run left
        // to do over it: the execution lands as a run whose results are gone, carrying that as
        // its reason, rather than staying owed for every later read to retry.
        let Some(prepared) = prepared else {
            let mut store = state.lock();
            if let Some(execution) = store
                .scheduled_query_mut(identifier)
                .and_then(|record| record.execution_mut(&query_id))
            {
                execution.settle(
                    DESTINATION_TYPE_S3,
                    DeliveryOutcome::failed(RESULTS_NO_LONGER_HELD_MESSAGE.to_string()),
                );
                execution.settle(
                    DESTINATION_TYPE_LOOKUP_TABLE,
                    DeliveryOutcome::failed(RESULTS_NO_LONGER_HELD_MESSAGE.to_string()),
                );
                execution.run_landed = true;
            }
            continue;
        };
        let execution = super::insights::run(prepared).await;
        let results = results_csv(&execution);

        let deliveries = {
            let mut store = state.lock();
            // "Returns the results FROM the specified query": what this run produced is
            // recorded on its own query, so `GetQueryResults` reports this run's rows however
            // the log groups it read change afterwards.
            if let Some(query) = store.queries.get_mut(&query_id) {
                query.record_snapshot(execution.clone());
            }
            record_run(
                &mut store,
                identifier,
                &query_id,
                execution.failed.as_deref(),
                results.as_deref(),
                region,
            )
        };

        // An Amazon S3 destination belongs to the object store rather than to this service,
        // so the write happens with this store's guard released.
        for delivery in deliveries {
            let outcome = deliver_to_s3(callback, region, &delivery).await;
            let mut store = state.lock();
            if let Some(execution) = store
                .scheduled_query_mut(identifier)
                .and_then(|record| record.execution_mut(&query_id))
            {
                execution.settle(DESTINATION_TYPE_S3, outcome);
            }
        }

        // The run has landed: its results are recorded and every destination has been
        // processed, so a later read reports it rather than running it again.
        if let Some(execution) = state
            .lock()
            .scheduled_query_mut(identifier)
            .and_then(|record| record.execution_mut(&query_id))
        {
            execution.run_landed = true;
        }
    }
}

/// Record what one run produced on the execution the history reports, deliver to the
/// destination this service OWNS — the lookup table — and answer the Amazon S3 deliveries the
/// execution still owes.
///
/// `failed` is why the run could not run at all, for a query string this service cannot
/// execute, and `results` the rows it produced as the CSV a destination is handed.
fn record_run(
    store: &mut State,
    identifier: &str,
    query_id: &str,
    failed: Option<&str>,
    results: Option<&str>,
    region: &str,
) -> Vec<S3Delivery> {
    let Some(configuration) = store
        .scheduled_query(identifier)
        .map(|record| record.destination_configuration.clone())
    else {
        return Vec::new();
    };

    // A query this service cannot run produced nothing to deliver, so every destination it is
    // configured with reports that same reason rather than a delivery that did not happen.
    let lookup_outcome = configuration
        .as_ref()
        .and_then(|configuration| configuration.lookup_table_configuration.as_ref())
        .map(|table| match failed {
            Some(reason) => DeliveryOutcome::failed(reason.to_string()),
            None => refresh_lookup_table(store, table, results, region),
        });

    if let Some(execution) = store
        .scheduled_query_mut(identifier)
        .and_then(|record| record.execution_mut(query_id))
    {
        execution.status = match failed {
            Some(_) => EXECUTION_STATUS_INVALID_QUERY.to_string(),
            None => EXECUTION_STATUS_COMPLETE.to_string(),
        };
        execution.error_message = failed.map(str::to_string);
        if let Some(outcome) = lookup_outcome {
            execution.settle(DESTINATION_TYPE_LOOKUP_TABLE, outcome);
        }
    }
    // The query's own last execution status is the status of its newest run.
    if let Some(record) = store.scheduled_query_mut(identifier) {
        let newest = record
            .trigger_history
            .last()
            .map(|execution| execution.status.clone());
        if let Some(status) = newest {
            record.last_execution_status = Some(status);
        }
    }

    let Some(s3) = configuration
        .as_ref()
        .and_then(|configuration| configuration.s3_configuration.as_ref())
    else {
        return Vec::new();
    };
    // What this run OWES the object store: an object holding its rows, or nothing at all —
    // a run that could not run delivered nothing and says why, and one that produced no rows
    // has no rows to write, which is a processing that completed over nothing.
    let owed = match (failed, results) {
        (Some(reason), _) => Err(DeliveryOutcome::failed(reason.to_string())),
        (None, None) => Err(DeliveryOutcome::complete(None)),
        (None, Some(body)) => match s3_uri_parts(&s3.destination_identifier) {
            Some((bucket, prefix)) => {
                let name = store
                    .scheduled_query(identifier)
                    .map(|record| record.name.clone())
                    .unwrap_or_default();
                Ok(S3Delivery {
                    uri: s3.destination_identifier.clone(),
                    bucket,
                    key: results_object_key(&prefix, &name, query_id),
                    body: body.to_string(),
                })
            }
            // A destination that is no Amazon S3 URI addresses no bucket, so there is nowhere
            // to deliver to rather than a delivery that failed on the way.
            None => Err(DeliveryOutcome::failed(format!(
                "{} is not an Amazon S3 URI results can be delivered to",
                s3.destination_identifier
            ))),
        },
    };
    match owed {
        Ok(delivery) => vec![delivery],
        Err(outcome) => {
            if let Some(execution) = store
                .scheduled_query_mut(identifier)
                .and_then(|record| record.execution_mut(query_id))
            {
                execution.settle(DESTINATION_TYPE_S3, outcome);
            }
            Vec::new()
        }
    }
}

/// One object a run writes to its Amazon S3 destination.
#[derive(Debug)]
struct S3Delivery {
    /// The destination URI as the query's configuration names it, which the object's own URI
    /// is built from.
    uri: String,
    /// The bucket that URI names.
    bucket: String,
    /// The key the results are written under.
    key: String,
    /// The results, as the CSV a destination is handed.
    body: String,
}

/// The bucket and the key prefix an `s3://bucket/prefix` URI names, or `None` for a value that
/// is not such a URI.
fn s3_uri_parts(uri: &str) -> Option<(String, String)> {
    let rest = uri.strip_prefix("s3://")?;
    let (bucket, prefix) = match rest.split_once('/') {
        Some((bucket, prefix)) => (bucket, prefix),
        None => (rest, ""),
    };
    (!bucket.is_empty()).then(|| (bucket.to_string(), prefix.to_string()))
}

/// The key one run's results are written under: the prefix the destination URI names, then the
/// scheduled query the results belong to and the id of the run that produced them — so one
/// prefix holds the results of several queries and of several runs without collision.
fn results_object_key(prefix: &str, name: &str, query_id: &str) -> String {
    let prefix = prefix.trim_end_matches('/');
    if prefix.is_empty() {
        format!("{name}/{query_id}.csv")
    } else {
        format!("{prefix}/{name}/{query_id}.csv")
    }
}

/// Write one run's results to the Amazon S3 destination it is configured with, answering what
/// the delivery produced.
///
/// Amazon S3 is reached the way the framework documents a handler reaching another emulated
/// service: an AWS SDK client over the in-process callback's HTTP client and credentials, so
/// nothing here opens a socket or leaves the host.
async fn deliver_to_s3(
    callback: &EmbeddedEmulator,
    region: &str,
    delivery: &S3Delivery,
) -> DeliveryOutcome {
    let region = ::service_common::arn::region_label(region).unwrap_or(super::DEFAULT_REGION);
    // The emulator serves one endpoint for every service, so the bucket travels in the path
    // rather than in a per-bucket host the router could not resolve.
    let client = callback_client!(aws_sdk_s3, callback, "s3", region, force_path_style(true));
    let answer = client
        .put_object()
        .bucket(&delivery.bucket)
        .key(&delivery.key)
        .body(::aws_sdk_s3::primitives::ByteStream::from(
            delivery.body.clone().into_bytes(),
        ))
        .send()
        .await;
    match answer {
        // "The identifier of the processed result at the destination": the object the results
        // were written to, named as the URI that addresses it.
        Ok(_) => DeliveryOutcome::complete(Some(format!(
            "s3://{}/{}",
            delivery.bucket, delivery.key
        ))),
        Err(error) => {
            let reported = ::aws_smithy_types::error::display::DisplayErrorContext(&error);
            // The caller's own values are rendered in their escaped `Debug` form, so a bucket
            // name or prefix carrying a newline cannot forge a log line.
            ::tracing::warn!(
                destination = ?delivery.uri,
                key = ?delivery.key,
                error = %reported,
                "a scheduled query's results could not be written to Amazon S3"
            );
            DeliveryOutcome::failed(format!("{reported}"))
        }
    }
}

/// Create or replace the lookup table one run delivers its results to, answering what the
/// delivery produced.
///
/// "The query results automatically populate or refresh the specified lookup table on each
/// scheduled execution": the table is a resource of THIS service, so the delivery writes it
/// into this very store and a later `GetLookupTable` reads the rows the run produced. A table
/// the delivery creates carries the description, key and tags its configuration names — "the
/// service applies tags only during initial table creation" — and a refresh replaces the
/// content alone.
fn refresh_lookup_table(
    store: &mut State,
    configuration: &LookupTableConfiguration,
    results: Option<&str>,
    region: &str,
) -> DeliveryOutcome {
    // A run that produced no rows has nothing to populate the table with, so the table it
    // holds is left as it is.
    let Some(body) = results else {
        return DeliveryOutcome::complete(None);
    };
    let shape = match super::parse_table_body(body) {
        Ok(shape) => shape,
        Err(rejection) => {
            return DeliveryOutcome::failed(match rejection {
                super::TableBodyRejection::Unreadable(message) => message,
                super::TableBodyRejection::DuplicateFields => {
                    super::DUPLICATE_TABLE_FIELDS_MESSAGE.to_string()
                }
            })
        }
    };
    let now = super::now_ms();
    let arn = super::lookup_table_arn(region, &configuration.table_name);
    // The store's own ceilings bound the tables one account holds and the bytes they hold
    // together ([`super::MAX_LOOKUP_TABLES`], [`super::MAX_LOOKUP_TABLE_BYTES_TOTAL`]), so a
    // delivery that would grow it past either does not happen rather than growing the store on
    // the clock. A refresh REPLACES the table's content, so what it would add is the new body
    // less the body it replaces.
    let held_bytes = store
        .lookup_tables
        .get(&configuration.table_name)
        .map(|held| held.table_body.len())
        .unwrap_or_default();
    let prospective_bytes = store
        .lookup_table_bytes()
        .saturating_sub(held_bytes)
        .saturating_add(body.len());
    if prospective_bytes > super::MAX_LOOKUP_TABLE_BYTES_TOTAL {
        return DeliveryOutcome::failed(format!(
            "the lookup tables of this account may hold at most {} bytes in total",
            super::MAX_LOOKUP_TABLE_BYTES_TOTAL
        ));
    }
    if let Some(held) = store.lookup_tables.get_mut(&configuration.table_name) {
        held.replace_content(body.to_string(), shape, now);
        return DeliveryOutcome::complete(Some(held.arn.clone()));
    }
    if store.lookup_tables.len() >= super::MAX_LOOKUP_TABLES {
        return DeliveryOutcome::failed(format!(
            "the maximum of {} lookup tables per account has been reached",
            super::MAX_LOOKUP_TABLES
        ));
    }
    let mut table = super::LookupTableRecord::new(
        &configuration.table_name,
        &arn,
        body.to_string(),
        shape,
        now,
    );
    table.description = configuration.description.clone();
    table.kms_key_id = configuration.kms_key_id.clone();
    table.tags = configuration
        .tags
        .iter()
        .flatten()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    store
        .lookup_tables
        .insert(configuration.table_name.clone(), table);
    DeliveryOutcome::complete(Some(arn))
}

/// One run's results as the CSV a destination is handed: a header row naming the fields the
/// first row carries, then one row per result in the order the query ordered them.
///
/// `None` for a run that produced no rows: there is no header row to name, so there is nothing
/// to deliver.
fn results_csv(execution: &super::insights::Execution) -> Option<String> {
    let fields: Vec<String> = execution
        .rows
        .first()?
        .iter()
        .map(|(field, _)| field.clone())
        .collect();
    let mut writer = ::csv::Writer::from_writer(Vec::new());
    // Writing to a `Vec` cannot fail, and neither can finishing the records over one.
    writer.write_record(&fields).ok()?;
    for row in &execution.rows {
        let values: Vec<&str> = fields
            .iter()
            .map(|field| {
                row.iter()
                    .find(|(name, _)| name == field)
                    .map(|(_, value)| value.as_str())
                    .unwrap_or_default()
            })
            .collect();
        writer.write_record(&values).ok()?;
    }
    let body = writer.into_inner().ok()?;
    String::from_utf8(body).ok()
}
