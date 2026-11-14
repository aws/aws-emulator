// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-redshiftdata-redshiftdata service.
//!
//! The Redshift Data API is an HTTP control plane in front of a real SQL engine:
//! `ExecuteStatement` submits one SQL statement and answers immediately with an
//! `Id`, and the caller then polls `DescribeStatement` and fetches rows with
//! `GetStatementResult`. Everything this module holds exists to make that
//! lifecycle real:
//!
//! * [`Statement`] — one submitted statement and everything the API reports
//!   about it. `ExecuteStatement` writes it; `DescribeStatement`,
//!   `GetStatementResult`, `ListStatements` and `CancelStatement` all read it.
//!   A `BatchExecuteStatement` submission is one of these too, holding one per
//!   statement it submitted in [`Statement::sub_statements`] — a batch's
//!   statement has its own text, status, outcome and `Id` (`<batch id>:<n>`),
//!   so it is the same thing as any other statement rather than a second kind.
//! * [`Database`] — the SQL engine itself. One in-process SQLite database
//!   (`rusqlite`, a real engine, not a hand-written SQL subset) per
//!   connection target and database name, so a `CREATE TABLE` submitted by one
//!   statement is visible to the `SELECT` submitted by the next, exactly as a
//!   Redshift database is. A batch's statements run serially on that one
//!   connection, which is what lets [`ExecutionMode::Transaction`] commit or
//!   roll back all of them together. `DescribeTable` reads its catalog, so a
//!   table's columns are the ones the engine really declares. What a caller
//!   submits is REDSHIFT's SQL, which [`redshift_sql`] reads and [`functions`]
//!   completes: the schemas a database holds are its own, and the databases a
//!   warehouse holds are the catalogue this state keeps beside the engines it
//!   opens for them.
//! * [`Session`] — the connection a caller's later statements reuse. A submit
//!   operation that asks to keep a session alive opens one; a statement that
//!   names only its `SessionId` runs on the warehouse and database that session
//!   was opened with, and `ListSessions` reports it. A session reuses its
//!   database's one connection, so what a session gives a caller is a
//!   connection target and a lifetime, not an isolated transaction of its own.
//!
//! The asynchronous status timeline is the ENGINE's, not a clock's: a statement
//! is `SUBMITTED` when the submit operation returns, `PICKED` once its turn on
//! its database is its, `STARTED` from the moment the engine is running it on
//! the blocking pool, and it reaches its terminal status when that execution
//! really finishes — so a statement reports STARTED for exactly as long as its
//! work takes, a caller that polls a long query sees it running, a caller that
//! cancels while it runs stops the engine itself, and nothing is ever reported
//! for work that has not been done. A submission the
//! caller asked to wait for ([`wait_budget`]) is awaited inside its own request;
//! one it did not is left to finish on its own, which is what makes two
//! concurrently submitted statements concurrent. A session's close is on the
//! clock, through the `SessionTtl` it carries.
//!
//! A submission that asked to be TOLD how it ended rather than polled for
//! (`WithEvent`) has the status change it makes when it stops published to
//! EventBridge by the execution that produced it — see [`statement_events`].

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use crate::ops::types::{ColumnMetadata, Field, SqlParameter};

/// Whether the cluster or workgroup a submit request names exists at all — a
/// fact the services that OWN those resources answer, not this one.
pub mod warehouse;

/// Resolving a submission's named `Parameters` against the placeholders the SQL
/// it submits really references.
pub mod sql_parameters;

/// Reading a submitted statement as Amazon Redshift's dialect, and handing the
/// engine what it has to run for it.
pub mod redshift_sql;

/// The Amazon Redshift SQL functions the engine does not carry, implemented and
/// registered on every connection.
pub mod functions;

/// The EventBridge event a statement whose submission asked for one publishes
/// when it stops.
pub mod statement_events;

/// Taking and restoring a snapshot of a warehouse's databases, for the control
/// planes that own the warehouses.
mod capture;
pub use capture::*;

/// Restoring ONE table of a captured warehouse into a live one, for the control
/// plane whose `RestoreTableFromClusterSnapshot` asks for it.
mod table_restore;
pub use table_restore::*;

/// The CONNECTION a submission's statements run on: the temporary relations
/// that belong to it alone, and the locks it holds against the others.
mod connections;
pub use connections::*;

/// Page size `ListStatements` uses when the request leaves `MaxResults` at its
/// modelled default of 0 — the modelled maximum, so an unparameterised call
/// returns as much as one page can hold.
pub const DEFAULT_LIST_PAGE_SIZE: usize = 100;

/// Page size an operation whose `MaxResults` is the modelled `PageSize` uses
/// when the request leaves that member at its default of 0 — `PageSize`'s
/// modelled maximum of 1000, so an unparameterised call returns as much as one
/// page can hold.
pub const DEFAULT_PAGE_SIZE: usize = 1_000;

/// Rows `GetStatementResult` returns in one page. Redshift documents no page
/// size for this operation, so this is an emulator-chosen bound; it only
/// decides where `NextToken` appears, never which rows exist.
pub const RESULT_PAGE_ROWS: usize = 1_000;

/// Bytes of RESULT DATA one page of a statement's result carries.
///
/// The Amazon Redshift Data API documents its ceiling on a RESULT READ: a call
/// that would return more than 100 MB of response data is ended, so the bound
/// is on what ONE call answers with and not on the result set a statement
/// produces — a larger result set is served across pages, which is what
/// `NextToken` is for. A page therefore ends at whichever comes first, the rows
/// [`RESULT_PAGE_ROWS`] admits or this many bytes of them, and a page always
/// carries at least one row so that paging cannot stall.
pub const MAX_RESULT_PAGE_BYTES: usize = 100 * 1024 * 1024;

/// The message the Data API answers a result read for a statement that produced
/// no result with — the fixed text recorded from the service, which points the
/// caller at the operation that does report the statement's outcome.
///
/// It lives here, beside the predicate that decides the case
/// ([`Statement::has_fetchable_result`]), so that every reader of a statement's
/// result refuses it the same way.
pub const NO_RESULT_MESSAGE: &str =
    "Query does not have result. Please check query status with DescribeStatement.";

/// The HTTP status the Data API answers a result read for a statement that
/// produced no result at.
///
/// The missing-resource shape it carries declares `@httpError 404`, which is the
/// status that shape renders with WHEREVER it is used; on this path the service
/// answers it as a client-input failure instead — the statement exists and its
/// outcome is readable, and what is wrong is that the caller asked for a result
/// of a statement that produced none — so the readers state the status here and
/// serve the error at it
/// ([`crate::scaffolded::Scaffolded::ModeledWithStatus`]) rather than swapping
/// the error code to move the status.
pub const NO_RESULT_STATUS: u16 = 400;

/// What a reader of a statement's result requires of the statement it is asked
/// for, when the statement's result is not read in the format that reader
/// serves.
///
/// A statement's rows are read in the format the statement was SUBMITTED with,
/// and each format has its own reader: `GetStatementResult` serves the JSON form
/// and `GetStatementResultV2` the CSV one, so the two are mutually exclusive for
/// any one statement. Each refuses with the requirement the model DOCUMENTS for
/// it, rather than with a sentence of this emulator's own: "The ExecuteStatement
/// or BatchExecuteStatement operation that ran the SQL statement must have
/// specified ResultFormat as JSON, or let the format default to JSON"
/// (`smithy_query(query="doc",shape="GetStatementResult")`) and "… must have
/// specified ResultFormat as CSV"
/// (`smithy_query(query="doc",shape="GetStatementResultV2")`).
pub const JSON_RESULT_READER_REQUIREMENT: &str =
    "The ExecuteStatement or BatchExecuteStatement operation that ran the SQL statement must \
     have specified ResultFormat as JSON, or let the format default to JSON.";

/// The same requirement, as the CSV reader's own documentation states it. See
/// [`JSON_RESULT_READER_REQUIREMENT`].
pub const CSV_RESULT_READER_REQUIREMENT: &str =
    "The ExecuteStatement or BatchExecuteStatement operation that ran the SQL statement must \
     have specified ResultFormat as CSV.";

/// The message the Data API answers a result read addressed to a BATCH's own
/// `Id` with: a batch is not a query, and its results live on the `:<n>` ids of
/// the statements it submitted.
pub const BATCH_PARENT_RESULT_MESSAGE: &str =
    "Query is a batch statement; fetch the results of each SQL statement of the batch with its \
     own Id, which is this Id with a :<n> suffix";

/// The message the Data API answers an `Id` it holds no query for with.
///
/// Fixed text, carrying no part of the request: the service states that the
/// query does not exist and nothing about the id that was asked for. It lives
/// here so `DescribeStatement`, `GetStatementResult` and `CancelStatement` — the
/// three readers that take an `Id` — cannot drift apart on either the wording or
/// the error it travels with.
pub const UNKNOWN_QUERY_MESSAGE: &str = "Query does not exist.";

/// The HTTP status the Data API answers that refusal with.
///
/// The missing-resource shape it carries declares `@httpError 404`, which is the
/// status that shape renders with WHEREVER it is used; on this path the service
/// answers it as a client-input failure instead, so the three readers state the
/// status here and serve the error at it
/// ([`crate::scaffolded::Scaffolded::ModeledWithStatus`]) rather than swapping
/// the error code to move the status.
pub const UNKNOWN_QUERY_STATUS: u16 = 400;

/// The message the Data API answers a continuation token it did not issue with.
///
/// Fixed text for every paginated operation of the service: a token is opaque to
/// the caller, so the refusal says only that the value is not a token and never
/// which operation minted (or did not mint) it.
pub const INVALID_TOKEN_MESSAGE: &str = "Invalid token value";

/// The message the Data API answers a `SessionId` it holds no session for with.
///
/// Fixed text, carrying no part of the request: the service states that the
/// session does not exist and nothing about the id that was asked for. It lives
/// here so every operation that RESOLVES a `SessionId` — the two submit
/// operations, which run a statement on the session's connection, and
/// `ListSessions`, which reports one session's metadata — states the same
/// sentence for the same missing resource.
pub const UNKNOWN_SESSION_MESSAGE: &str = "Session not found.";

/// The HTTP status `ListSessions` answers that refusal at.
///
/// The missing-resource shape it carries declares `@httpError 404`, which is the
/// status the submit operations answer it at; on the metadata read the service
/// answers it as a client-input failure instead, so that reader states the status
/// here and serves the error at it
/// ([`crate::scaffolded::Scaffolded::ModeledWithStatus`]) rather than swapping
/// the error code to move the status.
pub const UNKNOWN_SESSION_STATUS: u16 = 400;

/// The message the Data API answers a request that supplies members it documents
/// as MUTUALLY EXCLUSIVE with.
///
/// Fixed text down to the closing period; the only variable part is the member
/// list, which names the mutually exclusive members as the model spells them, in
/// the order that operation's own documentation states the rule in.
pub fn mutually_exclusive_message(members: &[&str]) -> String {
    format!(
        "Only one of the following can be specified: {}.",
        members.join(", ")
    )
}

/// The compute-target selectors ONE operation accepts, in the order its refusal
/// enumerates them.
///
/// The refusal of a request that does not select exactly one compute target
/// enumerates the selectors THAT operation accepts and no others, so an operation
/// whose input shape models no session selector never names one: a caller told to
/// choose between parameters it cannot pass has been sent after the wrong thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetSelectors {
    /// A SUBMIT operation (`ExecuteStatement`, `BatchExecuteStatement`): it runs a
    /// statement, so beside the two warehouse selectors its input models
    /// `SessionId`, and it admits a Glue-managed target as well.
    Submit,
    /// A reader of ONE warehouse's catalog (`ListDatabases`, `DescribeTable`):
    /// its input shape models the two warehouse selectors and no other — neither
    /// a session nor a Glue resource
    /// (`smithy_query(query="operation",operation="ListDatabases")`).
    Warehouse,
}

impl TargetSelectors {
    /// The selectors, as the refusal lists them.
    pub fn listed(self) -> &'static str {
        match self {
            Self::Submit => "ClusterIdentifier, WorkgroupName, Glue resource, SessionId",
            Self::Warehouse => "ClusterIdentifier, WorkgroupName",
        }
    }

    /// The message the Data API answers a request that does not name exactly one
    /// of these selectors with — the same sentence whether the request named none
    /// of them or more than one, since neither decides an endpoint.
    pub fn one_target_message(self) -> String {
        format!(
            "There should be only one of the following: {}",
            self.listed()
        )
    }
}

/// The message the Data API answers a target that resolves to no endpoint in the
/// caller's Region with. Fixed text: it names neither the cluster nor the
/// workgroup that could not be resolved.
pub const ABSENT_ENDPOINT_MESSAGE: &str = "Redshift endpoint doesn't exist in this region.";

/// Ceiling on the wall-clock bound one submission's execution runs under. The
/// SQL engine runs in-process with no isolation boundary, so a statement that
/// never terminates would take the whole emulator down rather than just its own
/// request.
///
/// It bounds the EXECUTION and not the request: a submission runs on the
/// blocking pool, independently of the request that submitted it, so the
/// statement reaches a terminal status whatever the caller does with its
/// response — and reaches it because the engine stopped, never because a caller
/// stopped waiting. A caller's own patience is `WaitTimeSeconds`
/// ([`wait_budget`]), which is shorter or longer than this and changes nothing
/// about the statement.
pub const MAX_ENGINE_DEADLINE: Duration = Duration::from_secs(10);

/// Step budget per second of an execution's wall-clock deadline, counted in
/// SQLite virtual-machine instructions. A wall-clock timeout cannot preempt a
/// CPU-bound query if the host clock stalls or the process is suspended; this
/// can.
///
/// Scaled to the deadline rather than fixed, and set well above the instruction
/// rate the engine actually sustains, so it is the BACKSTOP it is meant to be:
/// work that fits inside the deadline is never cut short by it, and work that
/// does not is stopped by whichever bound is reached first.
const SQL_STEPS_PER_SECOND: u64 = 150_000_000;

/// How many virtual-machine instructions pass between two consecutive checks of
/// the timeout and the step budget.
const PROGRESS_HANDLER_OPS: i32 = 1_000;

/// How many statements the text of ONE submission may carry.
///
/// The text itself is bounded by the engine's own `SQLITE_LIMIT_SQL_LENGTH`, but
/// that still admits very many statements, and each one is a prepare and a run
/// of its own — so how many a submission may hold is bounded here, as every
/// other thing a request can multiply is.
pub const MAX_STATEMENTS_PER_SUBMISSION: usize = 100;

/// Cap on the number of rows one submission's results retain.
pub const MAX_RESULT_ROWS: usize = 100_000;

/// Cap on the number of VALUES (rows times columns) one submission's results
/// retain. A cell costs memory whatever it holds, so a wide projection of
/// constants is bounded by this even though it charges almost nothing in bytes.
const MAX_RESULT_CELLS: usize = 2_000_000;

/// Cap on the size of one SUBMISSION's retained results, in bytes of RESULT
/// DATA — the size the result's own values take, which is what `ResultSize`
/// reports.
///
/// It is this emulator's MEMORY bound and not a limit of the service: the Data
/// API's own documented ceiling is on what one result READ answers with
/// ([`MAX_RESULT_PAGE_BYTES`]), so a statement whose whole result set is larger
/// than that still runs to completion and is served across pages — a bound here
/// that refused such a statement would fail work the real service serves. It is
/// therefore well above that per-call ceiling, and bounds what holding one
/// request's results can cost this process. A batch's statements share this one
/// budget, so a request cannot retain more by submitting more statements.
///
/// What a retained result RESIDENTLY costs is more than the bytes counted here,
/// and the other two caps are what bound the difference: each cell is held in a
/// [`Field`] of its own, so the per-cell overhead is at most
/// `MAX_RESULT_CELLS * size_of::<Field>()`, and a text value copied out of the
/// engine is at worst three times its engine size, because
/// `String::from_utf8_lossy` replaces each invalid byte with a three-byte
/// replacement character. One submission's retained result therefore costs this
/// process at most three times this ceiling plus that per-cell overhead, which
/// is what the three caps bound TOGETHER.
const MAX_RESULT_BYTES: usize = 256 * 1024 * 1024;

/// Cap on what the retained statement history holds in RESULTS at once, across
/// every statement of it, in the same bytes [`MAX_RESULT_BYTES`] counts.
///
/// [`MAX_RESULT_BYTES`] bounds ONE submission, and this bounds their sum: the
/// oldest statements whose execution has reported are dropped (with their
/// results) until the history is inside it, so a caller repeating a large query
/// cannot make the emulator hold result sets without limit. Larger than one
/// submission's ceiling, so a submission that is inside its own bound is never
/// refused by this one.
const MAX_RETAINED_RESULT_BYTES: usize = 512 * 1024 * 1024;

/// `SQLITE_LIMIT_LENGTH`: the largest single string or blob the engine will
/// produce, so one value cannot be larger than the whole result budget.
const MAX_VALUE_BYTES: i32 = 1024 * 1024;

/// `SQLITE_LIMIT_SQL_LENGTH`: the largest statement text the engine will accept.
const MAX_SQL_BYTES: i32 = 1024 * 1024;

/// `SQLITE_LIMIT_COLUMN`: the widest row the engine will produce.
const MAX_COLUMNS: i32 = 1_000;

/// `SQLITE_LIMIT_EXPR_DEPTH`: the depth bound on a parsed expression tree, so a
/// deeply nested expression cannot recurse the parser off its stack.
const MAX_EXPR_DEPTH: i32 = 100;

/// `SQLITE_LIMIT_COMPOUND_SELECT`: how many `UNION`/`EXCEPT`/`INTERSECT` terms
/// one statement may chain.
const MAX_COMPOUND_SELECT: i32 = 50;

/// `SQLITE_LIMIT_VARIABLE_NUMBER`: how many parameters one statement may bind.
const MAX_BOUND_PARAMETERS: i32 = 999;

/// `PRAGMA max_page_count`: the engine's own storage bound, in pages, applied to
/// BOTH schemas a statement can write — `main` and `temp`. At SQLite's default
/// 4 KiB page this is 256 MiB each, so a statement that builds a huge table, or
/// a huge TEMP table, runs out of database rather than out of the host's memory.
///
/// It has to admit the tables a Data API caller really builds: the service's own
/// documented ceiling on what ONE query may answer with is 100 MB, so a caller
/// paging through a large result set has a table of that order to build first,
/// and a bound below it would refuse work the real service serves. What bounds
/// the TOTAL across the databases a caller can open is the engine's allocator
/// ceiling ([`MAX_ENGINE_HEAP_BYTES`]), which is process-wide.
const MAX_DATABASE_PAGES: i64 = 65_536;

/// `PRAGMA hard_heap_limit`: the ceiling on what the SQL engine may allocate.
/// The page counts above bound each schema; this bounds the allocator itself, so
/// aggregate engine memory is capped however many databases are open and
/// whatever a statement does inside one. Above the per-schema bound, so one
/// database may really fill the pages it is allowed, and far below it times
/// [`MAX_OPEN_DATABASES`], so the databases a caller opens cannot add up to the
/// host's memory.
const MAX_ENGINE_HEAP_BYTES: i64 = 1024 * 1024 * 1024;

/// How many distinct connection targets and database names may be open at once.
/// The key is caller-chosen, so without this bound a caller could make the
/// emulator open connections and handles without limit.
pub const MAX_OPEN_DATABASES: usize = 16;

/// How many schemas one database holds. Amazon Redshift publishes a quota of
/// 9,900 schemas per database, which is what a `CREATE SCHEMA` past this reports
/// — and it bounds a map a caller grows one statement at a time.
pub const MAX_SCHEMAS_PER_DATABASE: usize = 9_900;

/// How many databases the warehouses of this emulator hold between them because
/// a caller CREATED them.
///
/// The catalogue is a map a caller grows one `CREATE DATABASE` at a time, under
/// names it chooses, so it is bounded like every other map here. It is larger
/// than [`MAX_OPEN_DATABASES`] because a database in the catalogue costs a name
/// until a statement runs on it, and only then an engine.
pub const MAX_CATALOGUE_DATABASES: usize = 64;

/// How many submissions may be running or queued at once.
///
/// Every accepted submission runs independently of the request that made it, so
/// without this bound a caller could make the emulator hold executions — and
/// the tasks and the SQL text carrying them — without limit. It is what the
/// modelled `ActiveStatementsExceededException` ("the number of active
/// statements exceeds the limit") reports; Amazon Redshift publishes a quota of
/// 200 active statements per cluster, and this emulator's bound is lower than
/// that because a submission in flight is also a statement in the history
/// ([`MAX_RETAINED_STATEMENTS`]), which is never evicted while its execution has
/// not reported. Half of that history, so admitting the bound's worth of
/// submissions still leaves room for the statements a caller can read back.
pub const MAX_ACTIVE_STATEMENTS: usize = MAX_RETAINED_STATEMENTS / 2;

/// How long an unused database is kept open. A database no statement has touched
/// for this long is closed, which releases the connection and everything a
/// statement committed to it — otherwise memory a single statement allocated
/// inside the engine would be held for the life of the process. The span is the
/// same as [`STATEMENT_RETENTION`], so a database is closed only once nothing
/// that ran on it is readable any more.
const DATABASE_IDLE_TIMEOUT: Duration = STATEMENT_RETENTION;

/// How many statements are retained. An emulator-chosen memory bound on top of
/// [`STATEMENT_RETENTION`]: each retained statement holds its result set, so the
/// per-statement caps above only bind if the number retained is bounded too.
///
/// It counts STATEMENTS, not submissions: a batch counts as itself plus each of
/// its statements, because each of those holds a result set of its own. What
/// those result sets add up to is bounded by [`MAX_RETAINED_RESULT_BYTES`],
/// whatever mix of single statements and batches a caller sends. The oldest
/// submissions are evicted first.
pub const MAX_RETAINED_STATEMENTS: usize = 100;

/// How long a statement stays readable. The Amazon Redshift Data API
/// documentation states that "the Data API retains the result set for 24 hours"
/// after a statement runs, so a statement older than that is no longer readable
/// on the real service either.
const STATEMENT_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);

/// How long a session is reported after it was created — `ListSessions` "lists
/// the sessions that the caller created in the last 24 hours", so one older
/// than this is no longer reported, closed or not.
const SESSION_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);

/// Sessions this emulator holds at once. A caller opens one per submit request
/// that asks to keep a session alive, so this bounds that map the way
/// [`MAX_RETAINED_STATEMENTS`] bounds the statement history.
pub const MAX_RETAINED_SESSIONS: usize = 100;

/// SQL functions this emulator refuses even though the engine's own build may
/// carry them: each one exists to reach something outside the database.
const DENIED_SQL_FUNCTIONS: &[&str] = &[
    "load_extension",
    "readfile",
    "writefile",
    "fsdir",
    "edit",
    "fts3_tokenizer",
];

/// The first process identifier handed to a database connection. Redshift
/// reports a backend process id (`RedshiftPid`); the value only has to be a
/// stable, unique long per connection.
const FIRST_PID: i64 = 1_000;

// ---------------------------------------------------------------------------
// Statement lifecycle
// ---------------------------------------------------------------------------

/// The statement lifecycle states this emulator produces.
///
/// The wire spellings are the ones `StatementStatusString` declares
/// (`shared_types::STATEMENT_STATUS_STRING_VALUES`), and the model documents
/// what each one means: "PICKED - The query has been chosen to be run. STARTED -
/// The query run has started. SUBMITTED - The query was submitted, but not yet
/// processed." A statement here walks exactly that timeline, because its
/// execution really does: it is SUBMITTED while it waits for its turn on its
/// database, PICKED once that turn is its, STARTED from the moment the engine is
/// running it, and then FINISHED, FAILED or ABORTED when that run ends. So a
/// caller polling a long statement sees it RUNNING — which is what makes it
/// cancellable — rather than one state covering queued and running alike.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatementStatus {
    /// The query was submitted, but not yet processed.
    Submitted,
    /// The query has been chosen to be run: its turn on its database is its, and
    /// the engine has not started it yet.
    Picked,
    /// The query run has started: the engine is running it now.
    Started,
    /// The query has finished running.
    Finished,
    /// The query run failed.
    Failed,
    /// The query run was stopped by the user.
    Aborted,
}

/// How a submission's statements are committed.
///
/// The wire spellings are the ones `ExecutionMode` declares
/// (`shared_types::EXECUTION_MODE_VALUES`): TRANSACTION and AUTO_COMMIT. The
/// model documents what each one does: "If set to TRANSACTION (the default),
/// all SQL statements are run as a single transaction and they are committed or
/// rolled back together. If set to AUTO_COMMIT, each SQL statement is committed
/// individually, and a failure of one statement does not affect the others."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    /// The statements are committed or rolled back together.
    Transaction,
    /// Each statement is committed on its own.
    AutoCommit,
}

impl ExecutionMode {
    /// The wire value, as `ExecutionMode` spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Transaction => "TRANSACTION",
            Self::AutoCommit => "AUTO_COMMIT",
        }
    }

    /// The mode a request's `ExecutionMode` names, or `None` for a value the
    /// model does not declare.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "TRANSACTION" => Some(Self::Transaction),
            "AUTO_COMMIT" => Some(Self::AutoCommit),
            _ => None,
        }
    }
}

impl StatementStatus {
    /// The wire value, as `StatementStatusString` spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Submitted => "SUBMITTED",
            Self::Picked => "PICKED",
            Self::Started => "STARTED",
            Self::Finished => "FINISHED",
            Self::Failed => "FAILED",
            Self::Aborted => "ABORTED",
        }
    }

    /// Whether the statement has stopped moving on its own.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Finished | Self::Failed | Self::Aborted)
    }
}

/// What a `CancelStatement` request found when it asked for a statement to be
/// stopped. Carries the status the store OBSERVED, so the caller never has to
/// look the statement up a second time to describe the outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cancellation {
    /// This call stopped a statement that was still running.
    Cancelled,
    /// The statement had already stopped, in this status, so this call
    /// cancelled nothing.
    AlreadyStopped(StatementStatus),
    /// The `Id` names ONE statement of a batch, which is not a query that runs
    /// on its own: its batch is what a caller cancels. Carries the batch's
    /// `Id`.
    BatchMember(String),
    /// No retained statement carries that `Id`.
    Unknown,
}

/// The rows and column metadata one statement produced.
#[derive(Debug, Clone, Default)]
pub struct ResultSet {
    /// One entry per column, in result order.
    pub column_metadata: Vec<ColumnMetadata>,
    /// One entry per row; each row has one [`Field`] per column.
    pub rows: Vec<Vec<Field>>,
}

/// The status one session reports.
///
/// The wire spellings are the ones `SessionStatusString` declares, and the
/// model documents what each one means: "AVAILABLE – The session is open and
/// ready to run a SQL statement. BUSY – The session is currently running a SQL
/// statement. CLOSED – The session is closed and can no longer run SQL
/// statements."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    /// Open, with no statement running in it.
    Available,
    /// A statement submitted in this session has not reached its terminal
    /// status yet.
    Busy,
    /// Past its `SessionTtl`: the clock closed it.
    Closed,
}

impl SessionStatus {
    /// The wire value, as `SessionStatusString` spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Available => "AVAILABLE",
            Self::Busy => "BUSY",
            Self::Closed => "CLOSED",
        }
    }
}

/// One Data API session: the connection a caller's later statements reuse, and
/// the clock that closes it.
///
/// A session is opened by a submit operation that asked to keep one alive
/// (`SessionKeepAliveSeconds`) and is reached afterwards by its `SessionId`,
/// which stands in for the whole connection — "The number of seconds to keep
/// the session alive after the query finishes", so its expiry is measured from
/// the last statement that ran in it.
#[derive(Debug, Clone)]
pub struct Session {
    /// `SessionId`, a UUID as the `UUID` shape's pattern requires.
    pub id: String,
    /// Monotonic sequence number. `ListSessions` orders by it and its
    /// pagination cursor names it, so a page boundary survives later sessions.
    pub seq: u64,
    /// The cluster or workgroup the session is connected to.
    pub target: ConnectionTarget,
    /// The database the session is connected to.
    pub database: String,
    /// `DbUser`, when the session was opened with temporary credentials.
    pub db_user: Option<String>,
    /// The `SecretArn` the session was opened with, when it authenticates with
    /// Secrets Manager. Not a member of `SessionData`, and held only so a
    /// statement that reuses the session runs under the same authorization.
    pub secret_arn: Option<String>,
    /// `CreatedAt`.
    pub created_at: SystemTime,
    /// `UpdatedAt`: when the session last ran a statement, which is what its
    /// expiry is measured from.
    pub updated_at: SystemTime,
    /// `SessionAliveSeconds`: the keep-alive the caller asked for.
    pub keep_alive_seconds: i32,
    /// `SessionTtl`: when the clock closes the session.
    pub expires_at: SystemTime,
}

impl Session {
    /// Whether the clock has closed this session by `now`.
    fn is_closed(&self, now: SystemTime) -> bool {
        now >= self.expires_at
    }

    /// How long after its last statement the session stays open: the
    /// keep-alive it was opened with, which the model bounds to 0..86400 ("The
    /// maximum time a session can keep alive is 24 hours").
    fn keep_alive(&self) -> Duration {
        Duration::from_secs(self.keep_alive_seconds.unsigned_abs().into())
    }
}

/// Which data warehouse a statement was submitted against. Redshift accepts a
/// provisioned cluster or a serverless workgroup, never both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionTarget {
    /// A provisioned cluster, by `ClusterIdentifier`.
    Cluster(String),
    /// A serverless workgroup, by `WorkgroupName`.
    Workgroup(String),
}

impl ConnectionTarget {
    /// The `ClusterIdentifier` to report, present only for a provisioned cluster.
    pub fn cluster_identifier(&self) -> Option<String> {
        match self {
            Self::Cluster(id) => Some(id.clone()),
            Self::Workgroup(_) => None,
        }
    }

    /// The `WorkgroupName` to report, present only for a serverless workgroup.
    pub fn workgroup_name(&self) -> Option<String> {
        match self {
            Self::Cluster(_) => None,
            Self::Workgroup(name) => Some(name.clone()),
        }
    }

    /// This warehouse as ONE identifier, for anything derived per warehouse: a
    /// storage key ([`Self::database_key`]), or the kind of a pagination cursor
    /// whose position means something only within one warehouse.
    ///
    /// The caller-chosen name is LENGTH-PREFIXED rather than joined by a
    /// separator, so no two distinct warehouses can produce one identifier
    /// whatever characters their names carry.
    pub fn key(&self) -> String {
        let (kind, name) = match self {
            Self::Cluster(id) => ("cluster", id.as_str()),
            Self::Workgroup(name) => ("workgroup", name.as_str()),
        };
        format!("{kind}:{}:{name}", name.len())
    }

    /// The key identifying this target's `database` in [`Database`] storage.
    ///
    /// Every caller-chosen part is LENGTH-PREFIXED rather than joined by a
    /// separator, so no two distinct (target, database) pairs can produce one
    /// key whatever characters the names carry — two different warehouses must
    /// never share one engine.
    fn database_key(&self, database: &str) -> String {
        format!("{}:{}:{database}", self.key(), database.len())
    }
}

/// One submitted SQL statement, and everything the Data API reports about it.
///
/// A batch submission is held as one statement too, with one of these per
/// statement it submitted in [`Statement::sub_statements`]: a batch's statement
/// is a statement in its own right — it has its own text, status, outcome and
/// `Id` (`<batch id>:<n>`) and is read back through the same operations — so it
/// is the same type rather than a parallel one.
#[derive(Debug)]
pub struct Statement {
    /// The `Id` minted at submit, a UUID as the `UUID` shape's pattern requires;
    /// for one statement of a batch, that batch's `Id` with the `:<n>` suffix
    /// the same pattern allows.
    pub id: String,
    /// `RedshiftQueryUuid`: the query the cluster ran. A batch is not itself a
    /// query — its statements are, and each one carries its own — so a batch has
    /// none.
    pub query_uuid: Option<String>,
    /// Monotonic submission sequence number. `ListStatements` orders by it and
    /// its pagination cursor names it, so a page boundary survives later
    /// submissions.
    pub seq: u64,
    /// The SQL text as submitted, reported as `QueryString`. Empty for a batch,
    /// whose statements each carry their own text.
    sql: String,
    /// The caller's `StatementName`, if any.
    pub statement_name: Option<String>,
    /// The database the statement ran against.
    pub database: Option<String>,
    /// The `DbUser` the caller authenticated as, when using temporary credentials.
    pub db_user: Option<String>,
    /// The `SecretArn` the caller authenticated with, when using Secrets Manager.
    pub secret_arn: Option<String>,
    /// The cluster or workgroup the statement ran against.
    pub target: ConnectionTarget,
    /// Whether this is a batch submission (`IsBatchStatement`), whose statements
    /// are [`Statement::sub_statements`].
    pub is_batch: bool,
    /// How this submission's statements were committed (`ExecutionMode`).
    execution_mode: ExecutionMode,
    /// The statements of a batch, in submission order. Empty for a single
    /// statement.
    pub sub_statements: Vec<Statement>,
    /// The status as of the last read.
    pub status: StatementStatus,
    /// The status this statement's execution reported, once it has reported.
    terminal_status: StatementStatus,
    /// When that reported transition fell due on the monotonic clock: the
    /// instant the execution finished. `None` while the execution is still
    /// queued or running, which is what SUBMITTED means.
    due_at: Option<Instant>,
    /// The wall-clock instant of that transition, reported as `UpdatedAt`.
    completed_at: SystemTime,
    /// Abandons this submission's execution at the engine's next progress
    /// callback, so `CancelStatement` stops work that is really running. Dropped
    /// once the execution has reported.
    interrupt: Option<Interrupt>,
    /// The engine's own error text, once the statement has FAILED.
    error: Option<String>,
    /// `CreatedAt`.
    pub created_at: SystemTime,
    /// `UpdatedAt`.
    pub updated_at: SystemTime,
    /// Measured execution time in nanoseconds, reported as `Duration`.
    duration_nanos: i64,
    /// `RedshiftPid`: the connection's backend process id.
    pub redshift_pid: i64,
    /// `RedshiftQueryId`.
    pub redshift_query_id: i64,
    /// Whether the statement produced a result set.
    has_result_set: bool,
    /// `ResultRows`: rows returned, or rows affected for a DML statement.
    result_rows: i64,
    /// `ResultSize`: the result set's size in bytes, or -1 when there is none.
    result_size: i64,
    /// Whether the caller asked for an EventBridge event.
    pub with_event: bool,
    /// The Region the submission was signed for: where the statement ran, and
    /// the Region an event about it is published in.
    pub region: String,
    /// The IAM principal that submitted the statement
    /// ([`statement_events::submitting_principal`]).
    pub principal: String,
    /// The ARN of the warehouse the statement ran on, when this service could
    /// name it: a provisioned cluster's documented ARN, or the ARN the
    /// serverless control plane minted for a workgroup.
    pub resource_arn: Option<String>,
    /// `ResultFormat` as the request SPELLED it: one of the modelled tokens,
    /// defaulted to JSON as the model documents. Read through
    /// [`Statement::reported_result_format`], which is the spelling the API
    /// reports it in.
    result_format: String,
    /// The `Parameters` the statement was submitted with.
    pub parameters: Vec<SqlParameter>,
    /// The session the statement ran in, when the caller used one.
    pub session_id: Option<String>,
    /// The `ClientToken` this statement was submitted with, so the idempotency
    /// map can be pruned together with the statement it points at.
    client_token: Option<String>,
    /// The rows the statement produced, if any.
    result: Option<ResultSet>,
    /// What holding that result costs this process, in bytes: what
    /// [`MAX_RETAINED_RESULT_BYTES`] bounds across the retained history.
    retained_bytes: usize,
}

impl Statement {
    /// Materialise the transition this statement's execution reported, if `now`
    /// has reached the instant it finished. Idempotent, and a no-op for a
    /// statement that is already terminal or whose execution has not reported:
    /// a read calls this so that an outcome the engine has produced is reported
    /// even when nothing else has touched the statement since.
    fn advance(&mut self, now: Instant) {
        // A batch's statements report through their batch, so one call
        // materialises the whole submission.
        for sub_statement in &mut self.sub_statements {
            sub_statement.advance(now);
        }
        let Some(due_at) = self.due_at else {
            return;
        };
        if self.status.is_terminal() || now < due_at {
            return;
        }
        self.status = self.terminal_status;
        self.updated_at = self.completed_at;
    }

    /// Record what this statement's execution produced: its outcome falls due at
    /// the instant the execution finished, which is `now`.
    fn apply(&mut self, execution: Execution, now: Instant, now_wall: SystemTime) {
        self.terminal_status = execution.status;
        self.error = execution.error;
        self.duration_nanos = execution.duration_nanos;
        self.has_result_set = execution.has_result_set;
        self.result_rows = execution.result_rows;
        self.result_size = execution.result_size;
        self.result = execution.result;
        self.retained_bytes = execution.retained_bytes;
        self.due_at = Some(now);
        self.completed_at = now_wall;
        self.interrupt = None;
        self.advance(now);
    }

    /// What holding this statement's results costs this process: its own, plus
    /// those of the statements of a batch, which are held on it.
    fn retained_bytes(&self) -> usize {
        self.sub_statements
            .iter()
            .fold(self.retained_bytes, |total, sub_statement| {
                total.saturating_add(sub_statement.retained_bytes())
            })
    }

    /// The `Error` to report: the engine's own message, and only once the
    /// statement has stopped WITHOUT finishing — a statement that failed carries
    /// the engine's reason, and a cancelled one the cancellation the caller
    /// asked for.
    pub fn error(&self) -> Option<String> {
        match self.status {
            StatementStatus::Failed | StatementStatus::Aborted => self.error.clone(),
            _ => None,
        }
    }

    /// `Duration` — nanoseconds the statement ran. Zero until it stops running.
    pub fn duration_nanos(&self) -> i64 {
        if self.status.is_terminal() {
            self.duration_nanos
        } else {
            0
        }
    }

    /// `HasResultSet`. Not known while the statement is still running, so it is
    /// reported only once the statement is terminal.
    pub fn has_result_set(&self) -> Option<bool> {
        self.status
            .is_terminal()
            .then_some(self.has_result_set && self.status == StatementStatus::Finished)
    }

    /// `ResultRows`. Zero until the statement stops running; -1 (the modelled
    /// "null" marker) when it stopped without producing a count.
    pub fn result_rows(&self) -> i64 {
        match self.status {
            StatementStatus::Submitted | StatementStatus::Picked | StatementStatus::Started => 0,
            StatementStatus::Finished => self.result_rows,
            StatementStatus::Failed | StatementStatus::Aborted => -1,
        }
    }

    /// `ResultSize`. Same shape as [`Statement::result_rows`].
    pub fn result_size(&self) -> i64 {
        match self.status {
            StatementStatus::Submitted | StatementStatus::Picked | StatementStatus::Started => 0,
            StatementStatus::Finished => self.result_size,
            StatementStatus::Failed | StatementStatus::Aborted => -1,
        }
    }

    /// The result set, readable only once the statement FINISHED.
    pub fn result(&self) -> Option<&ResultSet> {
        match self.status {
            StatementStatus::Finished => self.result.as_ref(),
            _ => None,
        }
    }

    /// The `Id` of the SUBMISSION this statement belongs to: its own for a
    /// statement submitted on its own, and the BATCH's for one statement of a
    /// batch.
    ///
    /// `SubStatementData.Id` documents the `:<n>` suffix positionally — "a
    /// suffix indicates the number of the SQL statement" — so the suffix is an
    /// ADDRESS for reaching one statement of a batch, not the identity that
    /// statement's record reports: a client walking a batch's `SubStatements`
    /// and describing each one correlates the record it gets back to the batch
    /// it belongs to through this.
    pub fn submission_id(&self) -> &str {
        split_batch_member(&self.id).map_or(self.id.as_str(), |(batch_id, _)| batch_id)
    }

    /// Whether this statement has a result a caller may fetch: it ran to
    /// completion AND produced a result set.
    ///
    /// A statement that FAILED or was ABORTED has no result, and neither has a
    /// FINISHED statement that returned no result set (DDL, or DML such as
    /// INSERT) — "no result set" and "an empty result set" are different
    /// answers, so the readers refuse the first instead of serving the second.
    pub fn has_fetchable_result(&self) -> bool {
        self.status == StatementStatus::Finished && self.result.is_some()
    }

    /// `ExecutionMode`. A single `ExecuteStatement` is committed on its own,
    /// which is what the modelled `AUTO_COMMIT` means; a batch reports the mode
    /// it was submitted with.
    pub fn execution_mode(&self) -> &'static str {
        self.execution_mode.as_str()
    }

    /// `ResultFormat`: the format this statement's result is read in, in the
    /// spelling the Data API REPORTS it in — the LOWER-CASE form of the token
    /// the request supplied.
    ///
    /// The model fixes the spelling of the INPUT member only (`ResultFormatString`
    /// is an enum of `JSON` and `CSV`, and it types `ExecuteStatementInput`'s
    /// member as well as every member that reports a statement's format); the
    /// service answers the lower-case form, which is what a client comparing the
    /// member against the reader it should use reads. So the submitted token is
    /// what is accepted and stored, and this is what every reporting member
    /// carries.
    pub fn reported_result_format(&self) -> String {
        self.result_format.to_ascii_lowercase()
    }

    /// Whether this statement's result is read in the CSV form: the format the
    /// request asked for, whatever case it spelled the token in.
    pub fn result_format_is_csv(&self) -> bool {
        self.result_format.eq_ignore_ascii_case("CSV")
    }

    /// `QueryString`: the text of one statement. A batch has no single query
    /// string — its statements are reported as `QueryStrings` and as
    /// `SubStatements` — so it reports none.
    pub fn query_string(&self) -> Option<String> {
        (!self.is_batch).then(|| self.sql.clone())
    }

    /// `QueryStrings`: the texts of a batch's statements, in submission order.
    /// A single statement reports its text as `QueryString` instead.
    pub fn query_strings(&self) -> Option<Vec<String>> {
        self.is_batch.then(|| {
            self.sub_statements
                .iter()
                .map(|sub_statement| sub_statement.sql.clone())
                .collect()
        })
    }

    /// Move this statement along its lifecycle to `reached`, which its own
    /// execution has really got to: PICKED when the submission's turn on its
    /// database is its, STARTED when the engine begins running it.
    ///
    /// Never moves a statement that has already stopped: a caller that cancelled
    /// it, or an execution that has already reported, has the last word.
    fn reach(&mut self, reached: StatementStatus, now_wall: SystemTime) {
        if self.status.is_terminal() {
            return;
        }
        self.status = reached;
        self.updated_at = now_wall;
    }

    /// Stop a still-running statement, as `CancelStatement` does. Returns false
    /// when the statement had already stopped, which is not a cancellation.
    ///
    /// The engine itself is abandoned: the execution is told to stop at its next
    /// progress callback, so a cancelled statement's remaining work does not
    /// run and a transaction it had opened is rolled back rather than committed.
    /// Its result set is discarded with it, because an aborted query has no rows
    /// to fetch.
    fn cancel(&mut self, now_wall: SystemTime) -> bool {
        if self.status.is_terminal() {
            return false;
        }
        // Cancelling a batch stops every statement of it: they are one
        // submission, and none of them is still running after this call.
        for sub_statement in &mut self.sub_statements {
            sub_statement.cancel(now_wall);
        }
        if let Some(interrupt) = self.interrupt.take() {
            interrupt.now();
        }
        self.status = StatementStatus::Aborted;
        self.terminal_status = StatementStatus::Aborted;
        // The query was stopped at the caller's request, which is what the
        // backend reports as the reason it stopped. A batch is not itself a
        // query, so it carries the message of the statement it was running.
        self.error = Some(
            self.sub_statements
                .iter()
                .find_map(Statement::error)
                .unwrap_or_else(|| cancelled_message(self.redshift_query_id)),
        );
        self.due_at = Some(Instant::now());
        self.completed_at = now_wall;
        self.updated_at = now_wall;
        self.duration_nanos = now_wall
            .duration_since(self.created_at)
            .ok()
            .and_then(|elapsed| i64::try_from(elapsed.as_nanos()).ok())
            .unwrap_or(0);
        // The rows the engine produced are discarded: a cancelled statement has
        // no result set to fetch.
        self.result = None;
        self.retained_bytes = 0;
        self.has_result_set = false;
        true
    }
}

// ---------------------------------------------------------------------------
// The SQL engine
// ---------------------------------------------------------------------------

/// The `Error` a statement stopped at the caller's request carries.
///
/// The query was running when it was stopped, so what is reported is the
/// backend's own reason for stopping it, naming the query it stopped
/// (`RedshiftQueryId`) — the id this service really minted for that query, never
/// a number of its own.
fn cancelled_message(redshift_query_id: i64) -> String {
    format!("ERROR: Query ({redshift_query_id}) cancelled on user's request")
}

/// The bound ONE execution runs under: a deadline, a remaining step budget and
/// an abandon flag, read by the connection's progress handler.
#[derive(Debug, Default)]
struct ExecutionGuard {
    deadline: Mutex<Option<Instant>>,
    steps_left: AtomicU64,
    abandoned: std::sync::atomic::AtomicBool,
}

impl ExecutionGuard {
    /// Arm the guard for one execution.
    fn arm(&self, timeout: Duration, steps: u64) {
        *self
            .deadline
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Instant::now() + timeout);
        self.steps_left.store(steps, Ordering::SeqCst);
    }

    /// Disarm the guard once the execution has returned, so an idle connection
    /// is never interrupted.
    fn disarm(&self) {
        *self
            .deadline
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    /// Abandon this execution: the engine stops it at its next progress
    /// callback, and it stops before it starts if it has not started yet — the
    /// flag outlives [`ExecutionGuard::arm`] rather than being overwritten by it.
    fn abandon(&self) {
        self.abandoned.store(true, Ordering::SeqCst);
    }

    /// Whether this execution must start no FURTHER work: it was abandoned, or
    /// its wall-clock deadline has passed.
    ///
    /// Read between the statements of ONE submission, where the engine's
    /// progress handler cannot be: a submission a caller cancelled runs no
    /// further statement of its text, and one past its deadline starts none. It
    /// spends no step budget, so asking costs the execution nothing.
    fn stopped(&self) -> bool {
        if self.abandoned.load(Ordering::SeqCst) {
            return true;
        }
        self.deadline
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some_and(|deadline| Instant::now() >= deadline)
    }

    /// Whether SQLite should abort the running statement. Called from the
    /// progress handler every [`PROGRESS_HANDLER_OPS`] instructions.
    fn should_interrupt(&self) -> bool {
        if self.abandoned.load(Ordering::SeqCst) {
            return true;
        }
        let deadline = *self
            .deadline
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(deadline) = deadline else {
            return false;
        };
        if Instant::now() >= deadline {
            return true;
        }
        let spent = u64::try_from(PROGRESS_HANDLER_OPS).unwrap_or(1);
        // The budget is compared AFTER this callback's own spend, so the
        // statement is interrupted on the callback that exhausts it rather than
        // on the one after — and a `saturating_sub` that lands past zero is
        // caught rather than missed.
        //
        // The comparison reads the value from BEFORE the update, which is what
        // the long-standing atomic method below returns; a newer toolchain
        // renames it and so reports it as deprecated here, and it is kept rather
        // than swapped for the new name, which an older toolchain does not have.
        #[allow(deprecated)]
        let previous = self
            .steps_left
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                Some(left.saturating_sub(spent))
            })
            .unwrap_or(0);
        previous.saturating_sub(spent) == 0
    }
}

/// The bound for ONE execution, created before that execution starts.
///
/// Per execution and never per database: the handler makes one, keeps an
/// [`Interrupt`] for it, and hands the bound to the blocking task, so
/// abandoning one request's statement can never abandon another request's.
#[derive(Debug)]
pub struct ExecutionBound(std::sync::Arc<ExecutionGuard>);

impl ExecutionBound {
    /// A fresh bound for one execution.
    pub fn new() -> Self {
        Self(std::sync::Arc::new(ExecutionGuard::default()))
    }

    /// A handle that abandons THIS execution and no other.
    pub fn interrupt(&self) -> Interrupt {
        Interrupt(std::sync::Arc::clone(&self.0))
    }

    /// Whether this execution has been abandoned — what a statement waiting for
    /// something other than the engine (a lock another connection holds) reads,
    /// so a cancelled statement stops waiting.
    pub fn abandoned(&self) -> bool {
        self.0.stopped()
    }
}

impl Default for ExecutionBound {
    fn default() -> Self {
        Self::new()
    }
}

/// A handle that asks one execution to be abandoned at the engine's next
/// progress callback — or before it starts, if it has not started yet.
#[derive(Clone, Debug)]
pub struct Interrupt(std::sync::Arc<ExecutionGuard>);

impl Interrupt {
    /// Abandon the execution now.
    pub fn now(&self) {
        self.0.abandon();
    }
}

/// One database of one data warehouse: a real SQL engine running in-process,
/// confined before it ever sees caller SQL (see [`Database::open`]).
///
/// Neither the connection's `RedshiftPid` nor any execution bound is held here:
/// the pid is immutable and lives on the state's own entry (`OpenDatabase`), so
/// a handler can resolve a database without taking this engine's lock — which is
/// held for a whole execution — and each execution brings its OWN
/// [`ExecutionBound`], so no bound outlives the statement it belongs to.
#[derive(Debug)]
pub struct Database {
    connection: rusqlite::Connection,
    /// The schemas this database holds — its own catalogue, which the engine has
    /// no notion of. `CREATE SCHEMA` adds one and `DROP SCHEMA` removes one, and
    /// a table of a schema is held under its qualified name (see
    /// [`redshift_sql::engine_table_name`]), so which schemas exist decides
    /// which tables a qualified name reaches.
    schemas: redshift_sql::SchemaSet,
}

impl Database {
    /// Open a fresh in-memory database, confine it, and install its execution
    /// bound.
    ///
    /// The engine is a real SQL engine, not a purpose-built tree walker, so the
    /// statements it would otherwise accept include ones that act on the HOST
    /// rather than on this database: `ATTACH DATABASE '<path>'` creates, opens or
    /// reads an arbitrary host file, `VACUUM INTO '<path>'` writes one, and a
    /// `PRAGMA` can turn the engine's own switches back on. Every one of those is
    /// closed here, before any request reaches `prepare`, and the order matters:
    /// the emulator's own `PRAGMA`s run first, because the authorizer installed
    /// at the end refuses `PRAGMA` from then on.
    fn open() -> rusqlite::Result<Self> {
        // Opened WITHOUT `SQLITE_OPEN_URI`: a URI filename is another way a
        // caller-chosen string could name a file, and nothing here needs it.
        // `SQLITE_OPEN_MEMORY` keeps the main database off the filesystem.
        let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
            | rusqlite::OpenFlags::SQLITE_OPEN_CREATE
            | rusqlite::OpenFlags::SQLITE_OPEN_MEMORY
            | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let mut connection = rusqlite::Connection::open_in_memory_with_flags(flags)?;

        // `temp_store = MEMORY`: a large sort or hash must not spill a file into
        // the host's temp directory. `max_page_count` on BOTH `main` and `temp`:
        // the pragma applies to the schema it is issued against, and
        // `temp_store = MEMORY` puts every temporary object on the process heap,
        // so bounding `main` alone would leave a `CREATE TEMP TABLE ... AS`
        // unbounded. `hard_heap_limit` bounds the engine's allocator itself, so
        // aggregate engine memory is capped whatever a statement does inside one
        // database and however many databases are open.
        connection.execute_batch(&format!(
            "PRAGMA temp_store = MEMORY;
             PRAGMA hard_heap_limit = {MAX_ENGINE_HEAP_BYTES};
             PRAGMA main.max_page_count = {MAX_DATABASE_PAGES};
             PRAGMA temp.max_page_count = {MAX_DATABASE_PAGES};"
        ))?;

        // Extension loading stays off DELIBERATELY, not incidentally: rusqlite's
        // `load_extension` feature is not enabled, so neither
        // `sqlite3_enable_load_extension` nor `Connection::load_extension` is
        // reachable from this crate at all, and the authorizer below denies the
        // `load_extension()` SQL function by name as well.
        // Defensive mode: no direct writes to the schema or to shadow tables.
        connection.set_db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
        // The schema is not trusted: a view or trigger stored in the database
        // cannot reach a privileged SQL function.
        connection.set_db_config(
            rusqlite::config::DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA,
            false,
        )?;

        // Engine limits, so a single statement cannot outgrow the bounds this
        // module enforces around it.
        use rusqlite::limits::Limit;
        // No database may be attached at all, which closes `ATTACH` even if the
        // authorizer below were ever bypassed.
        connection.set_limit(Limit::SQLITE_LIMIT_ATTACHED, 0)?;
        connection.set_limit(Limit::SQLITE_LIMIT_LENGTH, MAX_VALUE_BYTES)?;
        connection.set_limit(Limit::SQLITE_LIMIT_SQL_LENGTH, MAX_SQL_BYTES)?;
        connection.set_limit(Limit::SQLITE_LIMIT_COLUMN, MAX_COLUMNS)?;
        connection.set_limit(Limit::SQLITE_LIMIT_EXPR_DEPTH, MAX_EXPR_DEPTH)?;
        connection.set_limit(Limit::SQLITE_LIMIT_COMPOUND_SELECT, MAX_COMPOUND_SELECT)?;
        connection.set_limit(Limit::SQLITE_LIMIT_VARIABLE_NUMBER, MAX_BOUND_PARAMETERS)?;

        // Default-deny: every action a Data API statement legitimately needs is
        // named, and anything else — including an action a future engine version
        // introduces — is refused.
        connection.authorizer(Some(authorize))?;

        // The Redshift functions the engine does not carry, installed BEFORE
        // any caller SQL reaches the connection so that a statement naming one
        // of them finds an implementation rather than an unknown function.
        functions::install(&connection)?;

        Ok(Self {
            connection,
            // "PUBLIC schema … Every database created in Amazon Redshift has a
            // schema named PUBLIC": a database holds it from the moment it
            // exists, and a table nothing qualifies is a table of it.
            schemas: [DEFAULT_SCHEMA.to_string()].into_iter().collect(),
        })
    }
}

/// The engine's default-deny authorizer: what a submitted statement may do.
fn authorize(context: rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization {
    use rusqlite::hooks::{AuthAction, Authorization};
    match context.action {
        // The host-filesystem escapes, denied by name. `ATTACH`/`DETACH` open an
        // arbitrary path; `PRAGMA` is also how `VACUUM INTO '<path>'` is
        // authorized, and how the switches set in `Database::open` would be
        // turned back on.
        AuthAction::Attach { .. } | AuthAction::Detach { .. } | AuthAction::Pragma { .. } => {
            Authorization::Deny
        }
        // A virtual table module is an arbitrary extension point, some of whose
        // implementations read the filesystem.
        AuthAction::CreateVtable { .. } | AuthAction::DropVtable { .. } => Authorization::Deny,
        // A function whose whole purpose is to reach outside the database.
        AuthAction::Function { function_name, .. }
            if DENIED_SQL_FUNCTIONS
                .iter()
                .any(|denied| function_name.eq_ignore_ascii_case(denied)) =>
        {
            Authorization::Deny
        }
        // The SQL a Data API statement legitimately runs: query, modify and
        // define objects inside this one database.
        AuthAction::Select
        | AuthAction::Read { .. }
        | AuthAction::Insert { .. }
        | AuthAction::Update { .. }
        | AuthAction::Delete { .. }
        | AuthAction::CreateTable { .. }
        | AuthAction::CreateTempTable { .. }
        | AuthAction::DropTable { .. }
        | AuthAction::DropTempTable { .. }
        | AuthAction::CreateIndex { .. }
        | AuthAction::CreateTempIndex { .. }
        | AuthAction::DropIndex { .. }
        | AuthAction::DropTempIndex { .. }
        | AuthAction::CreateView { .. }
        | AuthAction::CreateTempView { .. }
        | AuthAction::DropView { .. }
        | AuthAction::DropTempView { .. }
        | AuthAction::CreateTrigger { .. }
        | AuthAction::CreateTempTrigger { .. }
        | AuthAction::DropTrigger { .. }
        | AuthAction::DropTempTrigger { .. }
        | AuthAction::AlterTable { .. }
        | AuthAction::Reindex { .. }
        | AuthAction::Analyze { .. }
        | AuthAction::Transaction { .. }
        | AuthAction::Savepoint { .. }
        | AuthAction::Recursive
        | AuthAction::Function { .. } => Authorization::Allow,
        // Anything else, including an action this engine version does not yet
        // name: refused rather than allowed by omission.
        _ => Authorization::Deny,
    }
}

/// What one execution produced. Opaque to a handler, which only carries it from
/// [`run_on`] or [`run_batch_on`] to
/// [`ComAmazonawsRedshiftdataRedshiftdataState::complete`].
#[derive(Debug)]
pub struct Execution {
    status: StatementStatus,
    error: Option<String>,
    duration_nanos: i64,
    has_result_set: bool,
    result_rows: i64,
    result_size: i64,
    result: Option<ResultSet>,
    /// The bytes of RESULT DATA this statement's result holds — what
    /// [`MAX_RETAINED_RESULT_BYTES`] bounds across the whole retained history.
    retained_bytes: usize,
    /// What this statement does to the WAREHOUSE's catalogue of databases, for
    /// the statements that act on it rather than on one database's contents, in
    /// the order the submission's statements asked for it. Applied by
    /// [`ComAmazonawsRedshiftdataRedshiftdataState::complete`], which is where
    /// that catalogue is reachable.
    catalogue: Vec<CatalogueEffect>,
    /// What this statement did to the CONNECTION it ran on, applied by the same
    /// call and for the same reason: the connection registry lives beside the
    /// catalogue, under the state lock, and not in the engine.
    connection: ConnectionOutcome,
}

/// What one statement did to the CONNECTION it ran on: the temporary relations
/// it created there, and what it did to the locks that connection holds.
#[derive(Debug, Default)]
struct ConnectionOutcome {
    /// The temporary relations the statement created, as the caller named them.
    temp_created: Vec<String>,
    /// The locks it took and released, in the order it asked for them.
    locks: Vec<LockEffect>,
}

/// What one statement does to the WAREHOUSE's catalogue of databases.
///
/// A `CREATE DATABASE` adds a database to the warehouse. That is not work for
/// the engine the statement runs on — that engine IS one database — so the
/// statement is carried out where the catalogue lives, under the state lock that
/// holds it, and the statement reports what the catalogue answered. `DROP
/// DATABASE` is NOT handled here: this emulator's catalogue holds the databases
/// the warehouse's own control plane provisioned beside the ones callers
/// created, and it cannot drop what it does not own, so the statement is left to
/// the engine, which refuses it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CatalogueEffect {
    /// Add a database of this name to the warehouse.
    CreateDatabase(String),
}

impl Execution {
    /// A statement that produced nothing because the engine refused it, with
    /// the engine's own message for why.
    fn failed(message: String) -> Self {
        Self {
            status: StatementStatus::Failed,
            error: Some(message),
            duration_nanos: 0,
            has_result_set: false,
            result_rows: -1,
            result_size: -1,
            result: None,
            retained_bytes: 0,
            catalogue: Vec::new(),
            connection: ConnectionOutcome::default(),
        }
    }

    /// A statement of a batch the run never reached: an earlier statement of
    /// the same transaction failed, so the batch stopped there and everything
    /// it had done was rolled back.
    ///
    /// It reports ABORTED — the only modelled status for a run that stopped
    /// without finishing or failing — and no `Error`, because this statement
    /// never reached the engine and so has no message from it.
    fn never_ran() -> Self {
        Self {
            status: StatementStatus::Aborted,
            error: None,
            duration_nanos: 0,
            has_result_set: false,
            result_rows: -1,
            result_size: -1,
            result: None,
            retained_bytes: 0,
            catalogue: Vec::new(),
            connection: ConnectionOutcome::default(),
        }
    }

    /// Report a statement whose work did not land after all, because the
    /// transaction holding it could not be closed. Its rows are dropped with    /// it: a result computed inside a transaction that was never committed is
    /// not a result a caller may fetch.
    fn mark_uncommitted(&mut self, message: &str) {
        if self.status != StatementStatus::Finished {
            return;
        }
        self.status = StatementStatus::Failed;
        self.error = Some(message.to_string());
        self.has_result_set = false;
        self.result_rows = -1;
        self.result_size = -1;
        self.result = None;
        self.retained_bytes = 0;
        // Nothing this statement asked of the warehouse's catalogue happened
        // either: the transaction that held it was never committed.
        self.catalogue = Vec::new();
        // Nor did anything it built on its connection: a temporary relation
        // created inside that transaction is not there either, and a lock it
        // took went with the transaction that held it.
        self.connection = ConnectionOutcome::default();
    }
}

/// What ONE submission may still retain of its results: the rows, the values
/// and the bytes left of its budget.
///
/// The budget belongs to the SUBMISSION, not to one execution: a batch's
/// statements all draw on one of these, so a batch cannot retain more result
/// data than a single `ExecuteStatement` can, whatever it submits and however
/// many statements it submits. [`MAX_RESULT_BYTES`] therefore bounds what one
/// REQUEST can add to the retained history, which is what makes
/// [`MAX_RETAINED_STATEMENTS`] a bound on the whole of it.
///
/// Only a result that is retained is charged: a statement that fails retains
/// nothing, so it spends nothing, and a statement with no result set spends
/// nothing either.
#[derive(Debug)]
struct Retention {
    rows_left: usize,
    cells_left: usize,
    bytes_left: usize,
}

impl Retention {
    /// A full budget, for one submission.
    fn new() -> Self {
        Self {
            rows_left: MAX_RESULT_ROWS,
            cells_left: MAX_RESULT_CELLS,
            bytes_left: MAX_RESULT_BYTES,
        }
    }

    /// Charge a result set this submission is keeping against the budget.
    fn spend(&mut self, rows: usize, cells: usize, bytes: usize) {
        self.rows_left = self.rows_left.saturating_sub(rows);
        self.cells_left = self.cells_left.saturating_sub(cells);
        self.bytes_left = self.bytes_left.saturating_sub(bytes);
    }
}

/// Which of a submission's named `Parameters` are bound to one statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Binding {
    /// Every parameter is bound, so one the statement does not reference is the
    /// engine's own error: `ExecuteStatement`'s `Parameters` are "the parameters
    /// for the SQL statement", that one statement's.
    Every,
    /// Only the parameters the statement references are bound: a batch's
    /// `Parameters` "are available to all SQL statements in the batch. Each
    /// statement can reference any subset of the provided parameters."
    Referenced,
}

/// Either the rows a query returned, or the row count a DML/DDL statement
/// affected. SQLite reports a statement that returns no columns as having none,
/// which is exactly the Data API's `HasResultSet=false` case.
enum Produced {
    /// The rows a query returned: the result set, the size the engine reports
    /// for it (`ResultSize`), and what the retained history charges for holding
    /// it — the same bytes, since what is retained IS the result.
    Rows(ResultSet, i64, usize),
    /// The rows a DML or DDL statement affected.
    Affected(i64),
}

/// Why one execution did not produce a complete result.
enum ExecutionFailure {
    /// The engine refused, interrupted or could not run the statement.
    Engine(rusqlite::Error),
    /// The result set reached one of this emulator's retention bounds. The
    /// statement FAILS rather than reporting a truncated result as a complete
    /// one: a caller must never be handed a short answer that looks whole.
    Bound(String),
    /// A catalogue refused the statement: a schema that is already there, one
    /// that is not, or one that still holds objects. The message is the one the
    /// data warehouse states for that refusal.
    Catalogue(String),
}

impl From<rusqlite::Error> for ExecutionFailure {
    fn from(error: rusqlite::Error) -> Self {
        Self::Engine(error)
    }
}

impl ExecutionFailure {
    /// The message to report as the statement's `Error`.
    fn message(&self) -> String {
        match self {
            Self::Engine(error) => scrub_paths(&error.to_string()),
            Self::Bound(message) | Self::Catalogue(message) => message.clone(),
        }
    }
}

/// One `Execution` per submitted statement for a submission whose execution
/// task did not return a verdict at all (its thread panicked, or the runtime
/// dropped it). Each statement FAILED, with the only thing that can honestly be
/// said about it: the engine reported nothing.
pub fn unreported_executions(statements: usize) -> Vec<Execution> {
    (0..statements)
        .map(|_| Execution::failed(UNREPORTED_EXECUTION_MESSAGE.to_string()))
        .collect()
}

/// One `Execution` per submitted statement for a submission that never reached
/// an engine at all, each FAILED with `message`.
///
/// The submit operations' channel for a statement whose CONNECTION is what
/// fails: a statement naming a database the warehouse does not hold is admitted
/// like any other and then fails with the backend's own message for a database
/// that is not there, rather than running against a database nobody created.
pub fn connection_failures(statements: usize, message: &str) -> Vec<Execution> {
    (0..statements)
        .map(|_| Execution::failed(message.to_string()))
        .collect()
}

/// The `Error` a statement whose execution never reported carries.
const UNREPORTED_EXECUTION_MESSAGE: &str = "the SQL engine did not report a result for this statement";

/// Run `sql` on the engine `handle` guards, on the connection `scope` names.
///
/// BLOCKING, deliberately: the engine is synchronous, so a caller must put this
/// on a blocking pool (`tokio::task::spawn_blocking`) rather than run it on an
/// async worker. It takes only this one database's lock, never the service's
/// state lock, so one caller's slow query serialises nothing but its own
/// database.
pub fn run_on(
    handle: &DatabaseHandle,
    sql: &str,
    parameters: &[SqlParameter],
    timeout: Duration,
    bound: &ExecutionBound,
    scope: &ConnectionScope,
) -> Execution {
    let mut database = handle
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    drop_retired_relations(&database, scope);
    let mut budget = Retention::new();
    let mut held = scope.temp_names();
    let mut run = RunContext {
        budget: &mut budget,
        timeout,
        bound,
        scope,
        held: &mut held,
    };
    let mut execution = run_sql(&mut database, sql, parameters, Binding::Every, &mut run);
    close_connection(&database, scope, std::slice::from_mut(&mut execution));
    execution
}

/// Run `sqls` serially, in submission order, on the engine `handle` guards, and/// report one [`Execution`] per submitted statement in that same order.
///
/// BLOCKING for the same reason [`run_on`] is, and it takes the same single
/// database lock — held for the whole batch, which is what lets
/// [`ExecutionMode::Transaction`] run the statements as ONE transaction: they
/// are committed together, or, as soon as one of them fails, the batch stops
/// there and everything it had done is rolled back ("If any SQL statement
/// fails, all work is rolled back"). Under [`ExecutionMode::AutoCommit`] each
/// statement is committed on its own and a failure stops nothing.
///
/// `timeout` bounds the WHOLE batch rather than each statement: each statement
/// is given what is left of it, so a batch of forty statements cannot outlast
/// the request that is waiting for it.
pub fn run_batch_on(
    handle: &DatabaseHandle,
    sqls: &[String],
    parameters: &[SqlParameter],
    mode: ExecutionMode,
    timeout: Duration,
    bound: &ExecutionBound,
    scope: &ConnectionScope,
) -> Vec<Execution> {
    let mut database = handle
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    drop_retired_relations(&database, scope);
    let deadline = Instant::now() + timeout;
    let transactional = mode == ExecutionMode::Transaction;

    // The transaction is opened through the engine itself, under the same
    // authorizer every submitted statement runs under. If it cannot be opened,
    // no statement of the batch ran, and each one reports the engine's own
    // message for why.
    if transactional {
        if let Err(error) = database.connection.execute_batch("BEGIN") {
            let message = scrub_paths(&error.to_string());
            return sqls
                .iter()
                .map(|_| Execution::failed(message.clone()))
                .collect();
        }
    }

    let mut executions: Vec<Execution> = Vec::with_capacity(sqls.len());
    // ONE retained-result budget for the whole batch, so a batch of forty
    // statements cannot keep forty result sets where a single statement may
    // keep one.
    let mut budget = Retention::new();
    // The connection's temporary relations as the batch's statements see them:
    // one of them creates a relation the next can resolve, because they run on
    // the same connection.
    let mut held = scope.temp_names();
    let mut run = RunContext {
        budget: &mut budget,
        timeout,
        bound,
        scope,
        held: &mut held,
    };
    for sql in sqls {
        run.timeout = deadline.saturating_duration_since(Instant::now());
        let execution = run_sql(&mut database, sql, parameters, Binding::Referenced, &mut run);
        let failed = execution.status == StatementStatus::Failed;
        executions.push(execution);
        // Everything this transaction did is about to be rolled back, so there
        // is nothing left for its remaining statements to do.
        if failed && transactional {
            break;
        }
    }

    if transactional {
        let rolled_back = executions
            .iter()
            .any(|execution| execution.status == StatementStatus::Failed);
        let close = if rolled_back { "ROLLBACK" } else { "COMMIT" };
        if let Err(error) = database.connection.execute_batch(close) {
            // The transaction could not be closed, so nothing it held landed:
            // every statement that had succeeded is reported as failed with the
            // engine's message rather than as work that was committed.
            let message = scrub_paths(&error.to_string());
            for execution in &mut executions {
                execution.mark_uncommitted(&message);
            }
        }
        // A transaction left open would leak into the next statement submitted
        // against this database, so it is closed even when the close above
        // reported an error.
        if !database.connection.is_autocommit() {
            let _ = database.connection.execute_batch("ROLLBACK");
        }
    }

    // The statements the batch never reached, because it stopped at the failure
    // above and rolled everything back.
    while executions.len() < sqls.len() {
        executions.push(Execution::never_ran());
    }
    close_connection(&database, scope, &mut executions);
    executions
}

/// What ONE run of a submission's statements runs under: the bounds it shares
/// with the rest of its submission, and the connection its statements run on.
struct RunContext<'a> {
    /// The submission's remaining retained-result budget, which a batch's
    /// statements all draw on.
    budget: &'a mut Retention,
    /// The wall-clock bound left for this run.
    timeout: Duration,
    /// The execution's own bound, which a cancel abandons.
    bound: &'a ExecutionBound,
    /// The connection the statements run on.
    scope: &'a ConnectionScope,
    /// The temporary relations that connection holds, which a statement that
    /// creates one adds to.
    held: &'a mut BTreeSet<String>,
}

/// Run `sql` on `database` under the wall-clock and step bounds, and report what
/// the engine did. A statement the engine rejects or interrupts fails with the
/// engine's own message — the service never guesses at a result it did not get.
fn run_sql(
    database: &mut Database,
    sql: &str,
    parameters: &[SqlParameter],
    binding: Binding,
    run: &mut RunContext<'_>,
) -> Execution {
    let bound = run.bound;
    let timeout = run.timeout;
    // This execution's own bound is installed on the connection for the duration
    // of this execution, replacing the previous one: a bound is never shared
    // between statements, so abandoning one cannot abandon another. A statement
    // whose bound could not be installed does NOT run: it would run unbounded.
    let guard = std::sync::Arc::clone(&bound.0);
    let handler_guard = std::sync::Arc::clone(&guard);
    if let Err(error) = database.connection.progress_handler(
        PROGRESS_HANDLER_OPS,
        Some(move || handler_guard.should_interrupt()),
    ) {
        // The bound is what makes an abandoned statement stop; a statement that could
        // only run without it does not run at all.
        return Execution::failed(scrub_paths(&error.to_string()));
    }

    let started = Instant::now();
    guard.arm(timeout, step_budget(timeout));
    // The text ONE submission carries may hold several statements, which a data
    // warehouse runs in order; each of them is read as Redshift SQL and run on
    // its own ([`redshift_sql`]). The submission reports the LAST statement it
    // ran, and stops at the first one that fails — the statements after it are
    // not run, exactly as they are not on the real service.
    let statements = redshift_sql::split_statements(sql);
    // A submission of several statements binds each one the parameters IT
    // references, as a batch's statements are bound: a parameter the first
    // statement does not reference belongs to another statement of the same
    // text, not to nothing.
    let binding = if statements.len() > 1 {
        Binding::Referenced
    } else {
        binding
    };
    let mut effects = RunEffects {
        scope: run.scope,
        held: &mut *run.held,
        catalogue: Vec::new(),
        connection: ConnectionOutcome::default(),
    };
    let mut produced = Ok(Produced::Affected(0));
    if statements.len() > MAX_STATEMENTS_PER_SUBMISSION {
        produced = Err(bound_reached(format!(
            "the text of one submission may carry at most \
             {MAX_STATEMENTS_PER_SUBMISSION} statements, and this one carries {}",
            statements.len()
        )));
    }
    for (index, statement) in statements.into_iter().enumerate() {
        if produced.is_err() {
            break;
        }
        // The bound belongs to the SUBMISSION, not to one statement of it: a
        // text of many statements cannot outlast the deadline by running them
        // one after another, and a submission a caller cancelled runs no
        // further statement.
        if index > 0 && guard.stopped() {
            produced = Err(ExecutionFailure::Bound(format!(
                "the statements left in this submission were not run: its execution reached this \
                 emulator's bound of {} seconds",
                timeout.as_secs()
            )));
            break;
        }
        produced = run_statement(
            database,
            statement,
            parameters,
            binding,
            &mut *run.budget,
            &mut effects,
        );
        if produced.is_err() {
            break;
        }
    }
    guard.disarm();
    let duration_nanos = i64::try_from(started.elapsed().as_nanos()).unwrap_or(i64::MAX);
    let RunEffects {
        catalogue,
        connection,
        ..
    } = effects;

    match produced {
        Ok(Produced::Rows(result, bytes, retained)) => Execution {
            status: StatementStatus::Finished,
            error: None,
            duration_nanos,
            has_result_set: true,
            result_rows: i64::try_from(result.rows.len()).unwrap_or(i64::MAX),
            result_size: bytes,
            result: Some(result),
            retained_bytes: retained,
            catalogue,
            connection,
        },
        Ok(Produced::Affected(rows)) => Execution {
            status: StatementStatus::Finished,
            error: None,
            duration_nanos,
            has_result_set: false,
            result_rows: rows,
            // No result set, so the size is the modelled null marker.
            result_size: -1,
            result: None,
            retained_bytes: 0,
            catalogue,
            connection,
        },
        Err(failure) => Execution {
            status: StatementStatus::Failed,
            error: Some(failure.message()),
            duration_nanos,
            has_result_set: false,
            result_rows: -1,
            result_size: -1,
            result: None,
            retained_bytes: 0,
            // Nothing a failed submission asked of a catalogue happened: it
            // stopped at the statement that failed.
            catalogue: Vec::new(),
            // What its statements did to their CONNECTION before it failed is
            // reported, because the engine really did it: a temporary relation
            // an earlier statement of the text created is there, and a lock it
            // took is held.
            connection,
        },
    }
}

/// What the statements of ONE run do beyond the engine: the connection they run
/// on, the temporary relations it holds as they see them, and what they ask of
/// the catalogue and of their connection for the state store to carry out.
struct RunEffects<'a> {
    /// The connection the statements run on.
    scope: &'a ConnectionScope,
    /// The temporary relations that connection holds, which a statement that
    /// creates one adds to so the next statement resolves it.
    held: &'a mut BTreeSet<String>,
    /// What the statements ask of the WAREHOUSE's catalogue of databases.
    catalogue: Vec<CatalogueEffect>,
    /// What they did to their connection.
    connection: ConnectionOutcome,
}

impl RunEffects<'_> {
    /// The temporary relations the connection holds, as the next statement read
    /// resolves them.
    fn temp(&self) -> TempRelations<'_> {
        TempRelations::new(self.scope.prefix(), self.held)
    }

    /// Record a temporary relation a statement just created on this connection.
    fn created_temp(&mut self, relation: String) {
        let relation = relation.to_ascii_lowercase();
        self.held.insert(relation.clone());
        self.connection.temp_created.push(relation);
    }

    /// How many locks the statements of THIS run have taken so far, which count
    /// against what one connection may hold beside the ones it already held.
    fn acquired(&self) -> usize {
        self.connection
            .locks
            .iter()
            .map(|effect| match effect {
                LockEffect::Acquire { tables, .. } => tables.len(),
                LockEffect::ReleaseAll => 0,
            })
            .sum()
    }
}

/// Run ONE statement of a submission: read it as Redshift SQL and do what that
/// reads as — run it on the engine, change the database's own catalogue of
/// schemas, take a lock, create a temporary relation of its connection, or
/// record what it asks of the warehouse's catalogue of databases in `effects`
/// for the state store to carry out.
fn run_statement(
    database: &mut Database,
    sql: &str,
    parameters: &[SqlParameter],
    binding: Binding,
    budget: &mut Retention,
    effects: &mut RunEffects<'_>,
) -> Result<Produced, ExecutionFailure> {
    let translated = redshift_sql::translate(sql, &database.schemas, &effects.temp());
    match translated {
        redshift_sql::Translated::Run(text) => {
            execute_once(&database.connection, &text, parameters, binding, budget)
        }
        // A temporary relation belongs to the connection that created it, so it
        // is recorded on that connection: a later statement of the same
        // connection resolves the name, and a statement on any other connection
        // does not reach the relation at all.
        redshift_sql::Translated::RunCreatingTemp { sql: text, relation } => {
            let produced = execute_once(&database.connection, &text, parameters, binding, budget)?;
            effects.created_temp(relation);
            Ok(produced)
        }
        redshift_sql::Translated::CreateSchema {
            name,
            if_not_exists,
        } => create_schema_on(database, &name, if_not_exists),
        redshift_sql::Translated::DropSchema {
            names,
            if_exists,
            cascade,
        } => drop_schema_on(database, &names, if_exists, cascade),
        redshift_sql::Translated::Lock { tables, mode } => {
            // What ONE connection may hold is bounded, as every other thing a
            // caller can multiply is: a statement that would take its
            // connection past that is refused rather than reported as having
            // taken a lock nothing then holds.
            let would_hold = effects
                .scope
                .locks_held()
                .saturating_add(effects.acquired())
                .saturating_add(tables.len());
            if would_hold > MAX_LOCKS_PER_CONNECTION {
                return Err(bound_reached(format!(
                    "this connection may hold at most {MAX_LOCKS_PER_CONNECTION} table locks at \
                     once, and this statement would make it {would_hold}"
                )));
            }
            let produced = lock_tables(&database.connection, &tables)?;
            // A lock is held for the TRANSACTION it was taken in: one taken
            // outside a transaction is released as soon as the statement that
            // took it ends, so only a lock taken inside one is held against the
            // other connections.
            if !database.connection.is_autocommit() {
                effects
                    .connection
                    .locks
                    .push(LockEffect::Acquire { tables, mode });
            }
            Ok(produced)
        }
        redshift_sql::Translated::CreateDatabase { name } => {
            effects.catalogue.push(CatalogueEffect::CreateDatabase(name));
            Ok(Produced::Affected(0))
        }
        // "BEGIN … There is no effect on any transaction that is already in
        // progress", and a `COMMIT` or `ROLLBACK` outside a transaction is
        // likewise answered with a warning rather than an error — so a control
        // with nothing to do reports the statement as run, where the engine
        // would refuse it.
        redshift_sql::Translated::Transaction(control) => {
            let in_transaction = !database.connection.is_autocommit();
            if control.opens() == in_transaction {
                return Ok(Produced::Affected(0));
            }
            database.connection.execute_batch(control.as_sql())?;
            // The transaction is over, and the locks it held are the
            // transaction's: "the locks are held until the transaction ends".
            if !control.opens() {
                effects.connection.locks.push(LockEffect::ReleaseAll);
            }
            Ok(Produced::Affected(0))
        }
    }
}

/// Drop the temporary relations of the connections this service no longer
/// holds, which the execution given this scope is what can reach: a connection
/// that has closed holds nothing, and its relations are nobody's to resolve.
fn drop_retired_relations(database: &Database, scope: &ConnectionScope) {
    for relation in scope.retired() {
        drop_temp_relation(database, relation);
    }
}

/// Close the connection a submission ran on, when that connection does not
/// outlive the submission.
///
/// A statement submitted without a `SessionId` runs on a connection opened for
/// it, so what that connection held goes away with it: the temporary relations
/// it created are dropped, and the locks it took are released with the
/// connection rather than recorded against it.
fn close_connection(database: &Database, scope: &ConnectionScope, executions: &mut [Execution]) {
    if scope.is_session() {
        return;
    }
    for execution in executions {
        for relation in std::mem::take(&mut execution.connection.temp_created) {
            drop_temp_relation(database, &scope.relations().engine_name(&relation));
        }
        execution.connection.locks.clear();
    }
}

/// Drop one temporary relation by its ENGINE name, whichever kind of relation it
/// is. Best effort: a relation that is not there is one the connection no longer
/// holds either way.
fn drop_temp_relation(database: &Database, engine_name: &str) {
    let quoted = engine_name.replace('"', "\"\"");
    let _dropped = database
        .connection
        .execute_batch(&format!("DROP VIEW IF EXISTS \"{quoted}\""));
    let _dropped = database
        .connection
        .execute_batch(&format!("DROP TABLE IF EXISTS \"{quoted}\""));
}

/// `CREATE SCHEMA`: add a schema to the database's own catalogue.
///
/// The catalogue is what decides whether a qualified name reaches a schema's
/// table, so creating one is a real change to this database and not a statement
/// reported as run and then forgotten.
fn create_schema_on(
    database: &mut Database,
    name: &str,
    if_not_exists: bool,
) -> Result<Produced, ExecutionFailure> {
    if redshift_sql::holds_schema(&database.schemas, name) {
        // "IF NOT EXISTS … the command makes no changes and returns a message
        // that the schema exists, rather than terminating with an error."
        if if_not_exists {
            return Ok(Produced::Affected(0));
        }
        return Err(ExecutionFailure::Catalogue(format!(
            "ERROR: schema \"{name}\" already exists"
        )));
    }
    if database.schemas.len() >= MAX_SCHEMAS_PER_DATABASE {
        return Err(ExecutionFailure::Catalogue(format!(
            "ERROR: schema limit exceeded: a database holds at most \
             {MAX_SCHEMAS_PER_DATABASE} schemas"
        )));
    }
    database.schemas.insert(name.to_string());
    Ok(Produced::Affected(0))
}

/// `DROP SCHEMA`: remove schemas from the database's catalogue.
///
/// `CASCADE` drops the tables each schema holds with it, which is what makes the
/// schema's name reusable; the default `RESTRICT` refuses a schema that still
/// holds objects, so nothing is dropped silently.
fn drop_schema_on(
    database: &mut Database,
    names: &[String],
    if_exists: bool,
    cascade: bool,
) -> Result<Produced, ExecutionFailure> {
    for name in names {
        if !redshift_sql::holds_schema(&database.schemas, name) {
            if if_exists {
                continue;
            }
            return Err(ExecutionFailure::Catalogue(format!(
                "ERROR: schema \"{name}\" does not exist"
            )));
        }
        let held = schema_objects(&database.connection, name)?;
        if !held.is_empty() && !cascade {
            return Err(ExecutionFailure::Catalogue(format!(
                "ERROR: cannot drop schema {name} because other objects depend on it"
            )));
        }
        for (object, kind) in held {
            let quoted = object.replace('"', "\"\"");
            let statement = if kind == "view" {
                format!("DROP VIEW \"{quoted}\"")
            } else {
                format!("DROP TABLE \"{quoted}\"")
            };
            database.connection.execute_batch(&statement)?;
        }
        database
            .schemas
            .retain(|schema| !schema.eq_ignore_ascii_case(name));
    }
    Ok(Produced::Affected(0))
}

/// The tables and views one schema holds, by engine name and kind: the objects
/// held under that schema's qualified prefix, read from the engine's own
/// catalog.
fn schema_objects(
    connection: &rusqlite::Connection,
    schema: &str,
) -> rusqlite::Result<Vec<(String, String)>> {
    let prefix = format!("{schema}.");
    let mut listing = connection.prepare(
        "SELECT name, type FROM sqlite_master WHERE type IN ('table', 'view') \
         AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )?;
    let mut held: Vec<(String, String)> = Vec::new();
    let mut rows = listing.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(0)?;
        let kind: String = row.get(1)?;
        // `get` answers None for an index that is not a character boundary, so a
        // name whose bytes do not split there is simply not this schema's.
        if name
            .get(..prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(&prefix))
        {
            held.push((name, kind));
        }
    }
    Ok(held)
}

/// `LOCK`: take the locks the statement asks for.
///
/// Statements of one database run one at a time here (see [`QueueSlot`]), so the
/// lock a statement asks for is already held for the whole of it; what is left
/// is to resolve the relations it names, which the engine does — a `LOCK` on a
/// table that is not there is an error on the real service too.
fn lock_tables(
    connection: &rusqlite::Connection,
    tables: &[String],
) -> Result<Produced, ExecutionFailure> {
    for table in tables {
        let quoted = table.replace('"', "\"\"");
        let _resolved = connection.prepare(&format!("SELECT 1 FROM \"{quoted}\" LIMIT 0"))?;
    }
    Ok(Produced::Affected(0))
}

/// Redact anything path-shaped from an engine message before it goes on the
/// wire. The message is the engine's own and a caller needs it to see why a
/// statement failed, but a host path in it would tell the caller about the
/// machine the emulator runs on.
///
/// Only a token that STARTS a path is redacted — one beginning at the start of
/// the message or after whitespace or a quote, with `/`, `\\` or a `C:\` drive
/// prefix. A slash inside a token, as in the identifier of a syntax error
/// (`near "a/b"`), is left alone, so the diagnostic keeps its detail.
fn scrub_paths(message: &str) -> String {
    let mut scrubbed = String::with_capacity(message.len());
    let mut characters = message.char_indices().peekable();
    let mut at_token_start = true;
    while let Some((index, character)) = characters.next() {
        let rest = &message[index..];
        let path_start = at_token_start
            && (character == '/'
                || rest.starts_with("\\\\")
                || (character.is_ascii_alphabetic() && rest.get(1..3) == Some(":\\")));
        if !path_start {
            scrubbed.push(character);
            at_token_start = character.is_whitespace()
                || character == '\''
                || character == '"'
                || character == '(';
            continue;
        }
        scrubbed.push_str("<path>");
        while characters
            .peek()
            .is_some_and(|(_, next)| !next.is_whitespace() && *next != '\'' && *next != '"')
        {
            characters.next();
        }
        at_token_start = false;
    }
    scrubbed
}

/// Prepare, bind and run one statement on the engine, keeping no more of its
/// result than `budget` — its SUBMISSION's remaining retained-result budget —
/// still allows.
fn execute_once(
    connection: &rusqlite::Connection,
    sql: &str,
    parameters: &[SqlParameter],
    binding: Binding,
    budget: &mut Retention,
) -> Result<Produced, ExecutionFailure> {
    let mut statement = connection.prepare(sql)?;

    // Redshift's `Parameters` are named (`:name`), and so are SQLite's, so the
    // names bind straight through. Which of them this statement gets is
    // [`Binding`]'s decision, and whether a name is in the statement at all is
    // the ENGINE's answer (`parameter_index`), never a scan of the SQL text.
    let names: Vec<String> = parameters
        .iter()
        .map(|parameter| format!(":{}", parameter.name))
        .collect();
    let mut bound: Vec<(&str, &dyn rusqlite::ToSql)> = Vec::with_capacity(names.len());
    for (name, parameter) in names.iter().zip(parameters.iter()) {
        if binding == Binding::Referenced && statement.parameter_index(name)?.is_none() {
            continue;
        }
        bound.push((name.as_str(), &parameter.value as &dyn rusqlite::ToSql));
    }

    if statement.column_count() == 0 {
        let affected = statement.execute(bound.as_slice())?;
        return Ok(Produced::Affected(i64::try_from(affected).unwrap_or(i64::MAX)));
    }

    // Column names and declared types are read before the query starts, so the
    // immutable borrow of the prepared statement ends before `query` takes it
    // mutably.
    let columns: Vec<(String, Option<String>)> = statement
        .columns()
        .map(|column| {
            (
                column.name().to_string(),
                column.decl_type().map(str::to_ascii_lowercase),
            )
        })
        .collect();
    let declared: Vec<Option<ColumnType>> = columns
        .iter()
        .map(|(_, decl)| decl.as_deref().map(ColumnType::from_declared))
        .collect();

    let mut rows = statement.query(bound.as_slice())?;
    let mut records: Vec<Vec<Field>> = Vec::new();
    // The result's own byte size: what `ResultSize` means, and what the
    // retention budget is counted in.
    let mut reported: usize = 0;
    let mut cells: usize = 0;
    let mut observed: Vec<Option<ColumnType>> = declared.clone();
    while let Some(row) = rows.next()? {
        if records.len() >= budget.rows_left {
            return Err(bound_reached(format!(
                "the result set exceeded the {} rows this submission may still retain, of this \
                 emulator's bound of {MAX_RESULT_ROWS}",
                budget.rows_left
            )));
        }
        if cells.saturating_add(columns.len()) > budget.cells_left {
            return Err(bound_reached(format!(
                "the result set exceeded the {} values this submission may still retain, of this \
                 emulator's bound of {MAX_RESULT_CELLS}",
                budget.cells_left
            )));
        }

        // Size the row BEFORE materialising any of it. `get_ref` borrows the
        // engine's own buffer and copies nothing, while `field_from` allocates an
        // owned copy of every value, so a row that would take the result set past
        // its budget is measured and then refused rather than allocated — one
        // very large value cannot be materialised by checking after the fact.
        let mut row_reported: usize = 0;
        for index in 0..columns.len() {
            let value = row.get_ref(index)?;
            row_reported = row_reported.saturating_add(value_bytes(value));
        }
        if reported.saturating_add(row_reported) > budget.bytes_left {
            return Err(bound_reached(format!(
                "the result set exceeded the {} bytes this submission may still retain, of this \
                 emulator's bound of {MAX_RESULT_BYTES}",
                budget.bytes_left
            )));
        }
        reported = reported.saturating_add(row_reported);
        cells += columns.len();

        let mut record = Vec::with_capacity(columns.len());
        for (index, declared_type) in declared.iter().enumerate() {
            let value = row.get_ref(index)?;
            // A column the engine declares a type for reports THAT type; one
            // without is inferred from the values it produces, and the inferred
            // type has to hold every one of them (see
            // [`ColumnType::widened_with`]), so every row is looked at rather
            // than only the first non-NULL one.
            if declared_type.is_none() {
                observed[index] = ColumnType::widened_with(observed[index].take(), value);
            }
            record.push(field_from(value, declared_type.as_ref()));
        }
        records.push(record);
    }

    let column_metadata = columns
        .iter()
        .zip(observed.iter())
        .map(|((name, _), column_type)| column_metadata(name, column_type.as_ref()))
        .collect();

    // The result is kept, so its cost is charged to the submission — only here,
    // on the path that returns it: a statement that failed above dropped every
    // row it had materialised and so spends nothing.
    budget.spend(records.len(), cells, reported);

    Ok(Produced::Rows(
        ResultSet {
            column_metadata,
            rows: records,
        },
        i64::try_from(reported).unwrap_or(i64::MAX),
        reported,
    ))
}

/// Report a retention bound as a statement failure.
fn bound_reached(message: String) -> ExecutionFailure {
    ExecutionFailure::Bound(message)
}

/// The byte size one value contributes to `ResultSize` — the engine's own size
/// for that value, which is what the member means.
fn value_bytes(value: rusqlite::types::ValueRef<'_>) -> usize {
    use rusqlite::types::ValueRef;
    match value {
        ValueRef::Null => 0,
        ValueRef::Integer(_) => std::mem::size_of::<i64>(),
        ValueRef::Real(_) => std::mem::size_of::<f64>(),
        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => bytes.len(),
    }
}

/// Map one engine value onto the modelled `Field` union.
///
/// The column's TYPE decides the member, not the storage class the engine
/// happened to use: `longValue` and `booleanValue` are stored identically by
/// SQLite, and an EXACT-numeric column (`DECIMAL`/`NUMERIC`) is reported under
/// `stringValue` carrying the value at the column's declared scale. A JSON
/// number cannot carry that scale, and rendering it would round-trip the value
/// through binary floating point, so `doubleValue` is for the APPROXIMATE
/// numeric types (`REAL`, `DOUBLE PRECISION`) alone. The exact INTEGER types
/// keep their own member, `longValue`, which loses nothing.
fn field_from(value: rusqlite::types::ValueRef<'_>, column_type: Option<&ColumnType>) -> Field {
    use rusqlite::types::ValueRef;
    if let Some(column_type) = column_type.filter(|column_type| column_type.is_exact_numeric()) {
        if let Some(rendered) = exact_numeric(value, column_type.declared_scale()) {
            return Field::stringValue(rendered);
        }
    }
    match value {
        ValueRef::Null => Field::isNull(true),
        ValueRef::Integer(integer) => {
            if column_type.is_some_and(|column_type| column_type.name == "bool") {
                Field::booleanValue(integer != 0)
            } else {
                Field::longValue(integer)
            }
        }
        ValueRef::Real(double) => Field::doubleValue(double),
        ValueRef::Text(bytes) => Field::stringValue(String::from_utf8_lossy(bytes).into_owned()),
        ValueRef::Blob(bytes) => Field::blobValue(bytes.to_vec()),
    }
}

/// Render one value of an exact-numeric column as the decimal text that
/// column's declaration calls for: the value with exactly `scale` digits after
/// the decimal point.
///
/// `None` for a value that is not one of the column's numbers — a NULL, or text
/// or bytes the engine kept verbatim — which is reported under the member that
/// matches what it actually is rather than dressed up as a number.
fn exact_numeric(value: rusqlite::types::ValueRef<'_>, scale: usize) -> Option<String> {
    use rusqlite::types::ValueRef;
    match value {
        // Scaled from the INTEGER itself rather than through a float, so a
        // value past what a double can hold exactly keeps every digit.
        ValueRef::Integer(integer) if scale == 0 => Some(integer.to_string()),
        ValueRef::Integer(integer) => Some(format!("{integer}.{}", "0".repeat(scale))),
        ValueRef::Real(double) => Some(format!("{double:.scale$}")),
        ValueRef::Null | ValueRef::Text(_) | ValueRef::Blob(_) => None,
    }
}

// ---------------------------------------------------------------------------
// Column metadata
// ---------------------------------------------------------------------------

/// A resolved column type: the name `ColumnMetadata.typeName` reports plus the
/// size facts derived from the declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnType {
    /// The value of `ColumnMetadata.typeName`.
    pub name: String,
    /// `ColumnMetadata.length` / `precision`, from a declared `(n)` or `(p,s)`.
    pub length: i32,
    /// `ColumnMetadata.scale`, from a declared `(p,s)`.
    pub scale: i32,
}

/// `ColumnMetadata.typeName` is "The database-specific data type of the column",
/// and Redshift answers with the database's own (PostgreSQL) type name rather
/// than the SQL keyword the column was declared with. This table is that
/// mapping, for the type keywords in the Amazon Redshift *Data types*
/// documentation
/// (<https://docs.aws.amazon.com/redshift/latest/dg/c_Supported_data_types.html>),
/// whose "Data type" / "Aliases" columns give the left-hand side and whose
/// internal names give the right.
///
/// Longest keyword first is not required: lookup is an exact match on the
/// declared keyword with any `(..)` size suffix already stripped.
const DECLARED_TYPE_NAMES: &[(&str, &str)] = &[
    ("smallint", "int2"),
    ("int2", "int2"),
    ("integer", "int4"),
    ("int", "int4"),
    ("int4", "int4"),
    ("bigint", "int8"),
    ("int8", "int8"),
    ("decimal", "numeric"),
    ("numeric", "numeric"),
    ("real", "float4"),
    ("float4", "float4"),
    ("double precision", "float8"),
    ("float8", "float8"),
    ("float", "float8"),
    ("boolean", "bool"),
    ("bool", "bool"),
    ("char", "bpchar"),
    ("character", "bpchar"),
    ("nchar", "bpchar"),
    ("bpchar", "bpchar"),
    ("varchar", "varchar"),
    ("character varying", "varchar"),
    ("nvarchar", "varchar"),
    ("text", "varchar"),
    ("date", "date"),
    ("time", "time"),
    ("time without time zone", "time"),
    ("timetz", "timetz"),
    ("time with time zone", "timetz"),
    ("timestamp", "timestamp"),
    ("timestamp without time zone", "timestamp"),
    ("timestamptz", "timestamptz"),
    ("timestamp with time zone", "timestamptz"),
    ("varbyte", "varbyte"),
    ("varbinary", "varbyte"),
    ("binary varying", "varbyte"),
    ("super", "super"),
];

/// The type names [`ColumnType`] reports for a numeric column, used for
/// `ColumnMetadata.isSigned`. Every Redshift numeric type is signed.
const SIGNED_TYPE_NAMES: &[&str] = &["int2", "int4", "int8", "numeric", "float4", "float8"];

/// The type names [`ColumnType`] reports for an EXACT-numeric column whose
/// values carry a declared scale: `DECIMAL` and its alias `NUMERIC`, which the
/// Amazon Redshift *Numeric types* documentation
/// (<https://docs.aws.amazon.com/redshift/latest/dg/r_Numeric_types201.html>)
/// separates from the approximate `REAL`/`DOUBLE PRECISION`. The exact INTEGER
/// types are not here: they declare no scale, and `longValue` carries them
/// without loss.
const EXACT_NUMERIC_TYPE_NAMES: &[&str] = &["numeric"];

/// The greatest scale an exact-numeric column can declare: the same
/// documentation gives `DECIMAL` a maximum precision of 38 digits, so no more
/// than 38 of them can sit to the right of the decimal point.
const MAX_NUMERIC_SCALE: i32 = 38;

/// The type names [`ColumnType`] reports for a character column. Redshift
/// compares `CHAR` and `VARCHAR` values case-sensitively, which is what
/// `ColumnMetadata.isCaseSensitive` reports.
const CASE_SENSITIVE_TYPE_NAMES: &[&str] = &["bpchar", "varchar", "super"];

impl ColumnType {
    /// Whether this column is an EXACT-numeric one, whose values are reported
    /// as decimal text so that neither the declared scale nor an exact digit is
    /// lost to binary floating point.
    fn is_exact_numeric(&self) -> bool {
        EXACT_NUMERIC_TYPE_NAMES.contains(&self.name.as_str())
    }

    /// How many digits this column declares after the decimal point, held to
    /// the greatest scale an exact-numeric column can have
    /// ([`MAX_NUMERIC_SCALE`]) so that a declaration naming more than Redshift
    /// allows cannot widen a rendered value without bound.
    fn declared_scale(&self) -> usize {
        usize::try_from(self.scale.clamp(0, MAX_NUMERIC_SCALE)).unwrap_or(0)
    }

    /// Resolve a column's declared type keyword — `VARCHAR(16)`,
    /// `NUMERIC(10,2)`, `BIGINT` — through [`DECLARED_TYPE_NAMES`]. A keyword
    /// the table does not carry keeps its own spelling: it is still the
    /// database-specific name of that column's type.
    fn from_declared(declared: &str) -> Self {
        let declared = declared.trim().to_ascii_lowercase();
        let (keyword, args) = match declared.split_once('(') {
            Some((keyword, rest)) => (keyword.trim(), rest.trim_end_matches(')')),
            None => (declared.as_str(), ""),
        };
        let mut sizes = args
            .split(',')
            .map(|size| size.trim().parse::<i32>().unwrap_or(0));
        let length = sizes.next().unwrap_or(0);
        let scale = sizes.next().unwrap_or(0);
        // A keyword outside the table is reported verbatim: it is still the
        // database-specific name of that column's type, and inventing a
        // different one would misreport it.
        let name = DECLARED_TYPE_NAMES
            .iter()
            .find(|(candidate, _)| *candidate == keyword)
            .map_or_else(|| keyword.to_string(), |(_, name)| (*name).to_string());
        Self {
            name,
            length,
            scale,
        }
    }

    /// Resolve a column with no declared type — an expression such as
    /// `SELECT 1` — from the value the engine produced for it.
    ///
    /// An INTEGER value takes the NARROWEST Redshift integer type whose range
    /// holds it, which is how a Redshift (PostgreSQL-derived) integer constant is
    /// typed: `int4` while the value fits the 4-byte `INTEGER` range
    /// (-2147483648 to +2147483647) and `int8` for anything past it, the ranges
    /// the Amazon Redshift *Data types* documentation publishes for `INTEGER` and
    /// `BIGINT`.
    fn from_value(value: rusqlite::types::ValueRef<'_>) -> Option<Self> {
        use rusqlite::types::ValueRef;
        let name = match value {
            // NULL says nothing about the column's type.
            ValueRef::Null => return None,
            ValueRef::Integer(integer) => {
                if i32::try_from(integer).is_ok() {
                    "int4"
                } else {
                    "int8"
                }
            }
            ValueRef::Real(_) => "float8",
            ValueRef::Text(_) => "varchar",
            ValueRef::Blob(_) => "varbyte",
        };
        Some(Self {
            name: name.to_string(),
            length: 0,
            scale: 0,
        })
    }

    /// The type an expression column must report once it has produced one more
    /// `value`: the type that holds EVERY value the column carried.
    ///
    /// Only widening happens, and only within the integer types, because that is
    /// the only family [`Self::from_value`] can infer differently for two values
    /// of one column: a column whose values all fit `INTEGER` is `int4`, and one
    /// value past that range makes the whole column `int8`, whichever row it
    /// arrives in. The first non-NULL value decides anything else, and a NULL
    /// says nothing.
    fn widened_with(current: Option<Self>, value: rusqlite::types::ValueRef<'_>) -> Option<Self> {
        let observed = Self::from_value(value);
        match (current, observed) {
            (None, observed) => observed,
            (Some(current), None) => Some(current),
            (Some(current), Some(observed)) => {
                match (integer_width(&current.name), integer_width(&observed.name)) {
                    (Some(held), Some(seen)) if seen > held => Some(observed),
                    _ => Some(current),
                }
            }
        }
    }
}

/// The integer type names [`ColumnType::from_value`] infers, NARROWEST FIRST — the
/// order of the ranges the Amazon Redshift *Data types* documentation publishes
/// for `INTEGER` (`int4`, 4 bytes) and `BIGINT` (`int8`, 8 bytes).
///
/// The ordering lives HERE rather than in a pairwise name comparison, so widening
/// an inferred column keeps working when another inferred width joins the list.
const INFERRED_INTEGER_WIDTHS: &[&str] = &["int4", "int8"];

/// How wide an inferred integer type is, as its position in
/// [`INFERRED_INTEGER_WIDTHS`]; `None` for a type that is not one of them.
fn integer_width(name: &str) -> Option<usize> {
    INFERRED_INTEGER_WIDTHS
        .iter()
        .position(|candidate| *candidate == name)
}

/// Build the modelled `ColumnMetadata` for one result column.
fn column_metadata(name: &str, column_type: Option<&ColumnType>) -> ColumnMetadata {
    let type_name = column_type.map(|column_type| column_type.name.clone());
    let signed = column_type
        .is_some_and(|column_type| SIGNED_TYPE_NAMES.contains(&column_type.name.as_str()));
    let case_sensitive = column_type
        .is_some_and(|column_type| CASE_SENSITIVE_TYPE_NAMES.contains(&column_type.name.as_str()));
    let length = column_type.map_or(0, |column_type| column_type.length);
    let scale = column_type.map_or(0, |column_type| column_type.scale);
    ColumnMetadata {
        // SQLite's prepared-statement metadata does not expose a result
        // column's NOT NULL constraint, so every column is reported with the
        // weakest claim: nullable.
        nullable: 1,
        is_case_sensitive: case_sensitive,
        // Redshift's supported data types carry no money type, so no column is
        // a currency column.
        is_currency: false,
        is_signed: signed,
        // Redshift reports the column's output name as both name and label; a
        // statement that aliases a column changes both together.
        label: Some(name.to_string()),
        name: Some(name.to_string()),
        // "The precision value of a decimal number column, or the column length
        // for a non-numeric column" — both are the declared size.
        precision: length,
        scale,
        length,
        type_name,
        // A result column's origin schema and table, and its default, are not
        // derivable from a prepared statement here, so they are reported absent
        // rather than guessed.
        schema_name: None,
        table_name: None,
        column_default: None,
    }
}

/// The schema a stored table is reported in.
///
/// Redshift creates a user table in `public` unless the statement names another
/// schema, and this emulator's engine holds every table of a database in one
/// namespace, so `public` is the schema the catalog reports a table in. Nothing
/// is invented by it: a table a caller reaches through the Data API's metadata
/// operations is in the database's default schema.
pub const DEFAULT_SCHEMA: &str = "public";

/// Build the modelled `ColumnMetadata` for one column of a STORED table, which
/// — unlike a column of a result set, whose origin a prepared statement does
/// not expose — has a schema and a table of origin to report.
fn table_column_metadata(
    name: &str,
    column_type: Option<&ColumnType>,
    table: &str,
) -> ColumnMetadata {
    ColumnMetadata {
        schema_name: Some(DEFAULT_SCHEMA.to_string()),
        table_name: Some(table.to_string()),
        ..column_metadata(name, column_type)
    }
}

/// One table of a database's catalog: its name, and the columns it declares in
/// declaration order.
#[derive(Debug, Clone)]
pub struct TableColumns {
    /// The table's name, as the engine's catalog spells it.
    pub table: String,
    /// One entry per declared column, in declaration order.
    pub columns: Vec<ColumnMetadata>,
}

/// Read the catalog of the database `handle` guards: the tables it holds and the
/// columns each one declares, optionally narrowed to one `table`.
///
/// BLOCKING for the same reason [`run_on`] is, and bounded the same way: the
/// catalog is read with real SQL on the engine's own `sqlite_master`, and each
/// table's columns come from the engine's answer to a zero-row query over it,
/// so a reported column is one the engine really declares.
pub fn catalog_on(
    handle: &DatabaseHandle,
    table: Option<&str>,
    timeout: Duration,
    bound: &ExecutionBound,
) -> Result<Vec<TableColumns>, String> {
    let mut database = handle
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let connection = &mut database.connection;

    let guard = std::sync::Arc::clone(&bound.0);
    let handler_guard = std::sync::Arc::clone(&guard);
    connection
        .progress_handler(
            PROGRESS_HANDLER_OPS,
            Some(move || handler_guard.should_interrupt()),
        )
        .map_err(|error| scrub_paths(&error.to_string()))?;
    guard.arm(timeout, step_budget(timeout));
    let read = read_catalog(connection, table);
    guard.disarm();
    read.map_err(|error| scrub_paths(&error.to_string()))
}

/// The catalog read [`catalog_on`] bounds: the engine's own list of tables and
/// views, and the columns each of them declares.
fn read_catalog(
    connection: &rusqlite::Connection,
    table: Option<&str>,
) -> rusqlite::Result<Vec<TableColumns>> {
    // The engine's own catalog, in name order. Views are tables a client can
    // read columns from, so they are described too; the engine's internal
    // objects are not part of the database's catalog.
    let mut listing = connection.prepare(
        "SELECT name FROM sqlite_master WHERE type IN ('table', 'view') \
         AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )?;
    let mut names: Vec<String> = Vec::new();
    let mut rows = listing.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(0)?;
        // Redshift matches an unquoted identifier case-insensitively, and so
        // does the engine, so the filter does too.
        if table.is_none_or(|wanted| wanted.eq_ignore_ascii_case(&name)) {
            names.push(name);
        }
    }
    drop(rows);
    drop(listing);

    let mut described = Vec::with_capacity(names.len());
    for name in names {
        // A zero-row query over the table: the engine answers with the table's
        // columns and their declared types, which is where every reported
        // column comes from.
        let quoted = name.replace('"', "\"\"");
        let probe = connection.prepare(&format!("SELECT * FROM \"{quoted}\" LIMIT 0"))?;
        let columns = probe
            .columns()
            .map(|column| {
                let declared = column
                    .decl_type()
                    .map(|decl| ColumnType::from_declared(&decl.to_ascii_lowercase()));
                table_column_metadata(column.name(), declared.as_ref(), &name)
            })
            .collect();
        described.push(TableColumns {
            table: name,
            columns,
        });
    }
    Ok(described)
}

// ---------------------------------------------------------------------------
// Pagination cursors
// ---------------------------------------------------------------------------

/// Cursor kind naming a position in `ListStatements`' ordering.
pub const CURSOR_STATEMENTS: &str = "statements";
/// Cursor kind naming a row offset in a statement's result set.
pub const CURSOR_ROWS: &str = "rows";
/// Cursor kind naming a position in `ListSessions`' ordering.
pub const CURSOR_SESSIONS: &str = "sessions";
/// Cursor kind naming a column offset in one table's `ColumnList`.
pub const CURSOR_COLUMNS: &str = "columns";
/// Cursor kind naming an offset into the databases of one data warehouse.
pub const CURSOR_DATABASES: &str = "databases";

/// Where the page of a statement's result that starts at `offset` ends.
///
/// A page carries at most [`RESULT_PAGE_ROWS`] rows and at most
/// [`MAX_RESULT_PAGE_BYTES`] of result data, whichever comes first, and always
/// at least ONE row: the service's documented ceiling is on what one result
/// read answers with, so a result set larger than it is served across pages
/// rather than refused, and a single row larger than one page is still served.
///
/// Both readers of a statement's result page by this, so the JSON and the CSV
/// form break their pages at the same row.
pub fn result_page_end(rows: &[Vec<Field>], offset: usize) -> usize {
    let mut bytes: usize = 0;
    let mut end = offset;
    while end < rows.len() && end.saturating_sub(offset) < RESULT_PAGE_ROWS {
        let row: usize = rows[end].iter().map(field_bytes).sum();
        if end > offset && bytes.saturating_add(row) > MAX_RESULT_PAGE_BYTES {
            break;
        }
        bytes = bytes.saturating_add(row);
        end += 1;
    }
    end
}

/// The bytes of RESULT DATA one retained field holds: the value itself, which
/// is what [`value_bytes`] charges for the same value as the engine produced
/// it.
fn field_bytes(field: &Field) -> usize {
    match field {
        Field::isNull(_) => 0,
        Field::booleanValue(_) => std::mem::size_of::<bool>(),
        Field::longValue(_) => std::mem::size_of::<i64>(),
        Field::doubleValue(_) => std::mem::size_of::<f64>(),
        Field::stringValue(value) => value.len(),
        Field::blobValue(value) => value.len(),
    }
}

/// Encode an opaque continuation token for `kind` at `position`.///
/// Hex-encoded so a client cannot read a meaning into it and depend on the
/// encoding; the kind is inside the token so a token minted for one operation
/// is rejected by the other.
pub fn encode_cursor(kind: &str, position: u64) -> String {
    hex::encode(format!("{kind}:{position}"))
}

/// Decode a continuation token minted by [`encode_cursor`] for `kind`.
/// `None` for anything else, which a handler refuses as a `ValidationException`.
pub fn decode_cursor(kind: &str, token: &str) -> Option<u64> {
    let decoded = hex::decode(token).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    text.strip_prefix(kind)?.strip_prefix(':')?.parse().ok()
}

// ---------------------------------------------------------------------------
// Timestamps
// ---------------------------------------------------------------------------

/// Render a wall-clock instant as the modelled `Timestamp`.
pub fn to_timestamp(instant: SystemTime) -> ::aws_smithy_types::DateTime {
    let millis = instant
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|since_epoch| since_epoch.as_millis())
        .unwrap_or(0);
    ::aws_smithy_types::DateTime::from_millis(i64::try_from(millis).unwrap_or(i64::MAX))
}

// ---------------------------------------------------------------------------
// The service's state
// ---------------------------------------------------------------------------

/// What was submitted: one statement, or a batch of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmissionKind {
    /// One statement, submitted by `ExecuteStatement`. It is committed on its
    /// own, which is what the modelled `AUTO_COMMIT` means.
    Single,
    /// A batch, submitted by `BatchExecuteStatement` in the `ExecutionMode` it
    /// asked for.
    Batch(ExecutionMode),
}

impl SubmissionKind {
    /// Whether this submission is a batch, which is reported as one statement
    /// with one `SubStatements` entry per statement it submitted.
    pub fn is_batch(self) -> bool {
        matches!(self, Self::Batch(_))
    }

    /// The `ExecutionMode` this submission's statements were committed under.
    pub fn execution_mode(self) -> ExecutionMode {
        match self {
            Self::Single => ExecutionMode::AutoCommit,
            Self::Batch(mode) => mode,
        }
    }
}

/// Everything a submit operation needs to record what it ran.
#[derive(Debug)]
pub struct Submission {
    /// The statements that were run, in submission order: one for
    /// `ExecuteStatement`, a batch's `Sqls` for `BatchExecuteStatement`.
    pub statements: Vec<String>,
    /// Whether this was one statement or a batch, and how a batch committed.
    pub kind: SubmissionKind,
    /// The cluster or workgroup to run it against.
    pub target: ConnectionTarget,
    /// The database to run it in.
    pub database: String,
    /// The `DbUser`, when authenticating with temporary credentials.
    pub db_user: Option<String>,
    /// The `SecretArn`, when authenticating with Secrets Manager.
    pub secret_arn: Option<String>,
    /// The caller's `StatementName`.
    pub statement_name: Option<String>,
    /// The caller's `Parameters`.
    pub parameters: Vec<SqlParameter>,
    /// The caller's `WithEvent`.
    pub with_event: bool,
    /// The Region the submit request was signed for.
    pub region: String,
    /// The IAM principal that made the submit request.
    pub principal: String,
    /// The ARN of the warehouse the submission runs on, as the service that
    /// owns it names it.
    pub resource_arn: Option<String>,
    /// The resolved `ResultFormat`.
    pub result_format: String,
    /// The session the statement runs in, when the caller uses one.
    pub session_id: Option<String>,
    /// `SessionKeepAliveSeconds`: how long to keep the session alive after the
    /// statement finishes, when the caller asked for a session.
    pub session_keep_alive_seconds: Option<i32>,
    /// The caller's `ClientToken`, for an idempotent replay.
    pub client_token: Option<String>,
}

/// The connection members of a submit request, as the wire carried them.
///
/// `ExecuteStatement` and `BatchExecuteStatement` document the SAME
/// authorization combinations, so both resolve them here rather than each
/// deciding for itself.
#[derive(Debug, Default, Clone, Copy)]
pub struct ConnectionRequest<'a> {
    /// `ClusterIdentifier`: a provisioned cluster.
    pub cluster_identifier: Option<&'a String>,
    /// `WorkgroupName`: a serverless workgroup.
    pub workgroup_name: Option<&'a String>,
    /// `SessionId`: a session a previous statement opened.
    pub session_id: Option<&'a String>,
    /// `Database`.
    pub database: Option<&'a String>,
    /// `DbUser`: temporary-credentials authorization.
    pub db_user: Option<&'a String>,
    /// `SecretArn`: Secrets Manager authorization.
    pub secret_arn: Option<&'a String>,
}

/// The data warehouse, database and authorization method a submission runs
/// under.
#[derive(Debug)]
pub struct Connection {
    /// The cluster or workgroup.
    pub target: ConnectionTarget,
    /// The database name.
    pub database: String,
    /// `DbUser`, when authenticating with temporary credentials.
    pub db_user: Option<String>,
    /// `SecretArn`, when authenticating with Secrets Manager.
    pub secret_arn: Option<String>,
}

/// Why a request's connection members do not name exactly one data warehouse,
/// one database and one authorization method.
///
/// The two target faults are kept apart although the service answers both with
/// the same sentence: they are different requests — one selects nothing, the
/// other selects two endpoints — so a reader of this enum can tell them apart
/// even though the refusal does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadConnection {
    /// The request names NO compute target at all: no warehouse and no session.
    NoTarget,
    /// The request names more than one compute target at once, so which endpoint
    /// it addresses is not decided.
    SeveralTargets,
    /// No warehouse, but a `SessionId`: the warehouse and database this
    /// request runs against are the ones its SESSION was opened with, so they
    /// are resolved from the session store
    /// ([`ComAmazonawsRedshiftdataRedshiftdataState::session_connection`])
    /// rather than from the request.
    SessionOnly,
    /// No database.
    NoDatabase,
    /// Two authorization methods at once.
    BothAuthMethods,
}

impl BadConnection {
    /// What is wrong with the request, as a `ValidationException` message.
    /// [`BadConnection::SessionOnly`] is NOT a malformed request, so its text
    /// says what is missing from the emulator instead.
    ///
    /// `selectors` are the compute-target selectors the CALLING operation
    /// accepts, which is the only part of the target-selection sentence that
    /// differs between operations ([`TargetSelectors`]).
    pub fn message(self, selectors: TargetSelectors) -> String {
        match self {
            // Nothing the request named could resolve to an endpoint, so there
            // is nothing but the SELECTION to report: the canonical sentence,
            // which is also what naming two warehouses gets.
            Self::NoTarget | Self::SeveralTargets => selectors.one_target_message(),
            Self::SessionOnly => {
                "SessionId does not name a live session: the warehouse and database a statement \
                 runs against are the ones its session was opened with"
                    .to_string()
            }
            Self::NoDatabase => "Database is required".to_string(),
            Self::BothAuthMethods => {
                "DbUser and SecretArn cannot both be supplied: DbUser authenticates with \
                 temporary credentials and SecretArn with Secrets Manager"
                    .to_string()
            }
        }
    }

}

impl ConnectionRequest<'_> {
    /// Resolve the one data warehouse, database and authorization method the
    /// documented combinations name.
    ///
    /// A statement runs against ONE target. The service enumerates the targets
    /// the CALLING operation accepts in the one sentence it refuses a bad
    /// selection with — a provisioned cluster (`ClusterIdentifier`), a
    /// serverless workgroup (`WorkgroupName`), and, where that operation models
    /// them, a Glue resource or a session a previous statement opened
    /// (`SessionId`) — so a request that names two of them does not decide
    /// which endpoint it addresses ([`BadConnection::SeveralTargets`]), and one
    /// that names none of them selects nothing ([`BadConnection::NoTarget`]);
    /// both carry that message ([`TargetSelectors::one_target_message`]). Every
    /// documented authorization combination then
    /// supplies the database; `SecretArn` selects Secrets Manager authorization
    /// and `DbUser` temporary credentials, which are alternatives, so supplying
    /// both names two methods for one submission.
    ///
    /// The session-reuse form, where the single target is the `SessionId`,
    /// carries its connection on the SESSION: it is reported as
    /// [`BadConnection::SessionOnly`], which a handler resolves against the
    /// session store
    /// ([`ComAmazonawsRedshiftdataRedshiftdataState::session_connection`])
    /// before deciding anything — only a `SessionId` that names no live session
    /// is a request this cannot serve.
    pub fn resolve(self) -> Result<Connection, BadConnection> {
        let cluster = non_empty(self.cluster_identifier);
        let workgroup = non_empty(self.workgroup_name);
        let session = non_empty(self.session_id);
        // ONE target, counted over every target the service's own message
        // enumerates: a provisioned cluster, a serverless workgroup and a
        // session a previous statement opened are each an endpoint of their own,
        // so a request naming two of them does not decide which it addresses and
        // one naming none of them selects nothing.
        let selected = [cluster.is_some(), workgroup.is_some(), session.is_some()]
            .into_iter()
            .filter(|named| *named)
            .count();
        if selected > 1 {
            return Err(BadConnection::SeveralTargets);
        }
        if selected == 0 {
            return Err(BadConnection::NoTarget);
        }
        let target = match (cluster, workgroup) {
            (Some(cluster), None) => ConnectionTarget::Cluster(cluster),
            (None, Some(workgroup)) => ConnectionTarget::Workgroup(workgroup),
            // The one remaining selection is the session, whose connection the
            // session store holds rather than the request.
            _ => return Err(BadConnection::SessionOnly),
        };
        let Some(database) = non_empty(self.database) else {
            return Err(BadConnection::NoDatabase);
        };
        let db_user = non_empty(self.db_user);
        let secret_arn = non_empty(self.secret_arn);
        if db_user.is_some() && secret_arn.is_some() {
            return Err(BadConnection::BothAuthMethods);
        }
        Ok(Connection {
            target,
            database,
            db_user,
            secret_arn,
        })
    }

}

/// A lockable handle to one database's engine.
///
/// The engine lives behind its OWN lock rather than inside the service's state
/// lock, so a handler takes the state lock only to resolve the handle and again
/// to record the result — never for the duration of an execution.
pub type DatabaseHandle = std::sync::Arc<Mutex<Database>>;

/// Why a submission could not be given a database to run on.
#[derive(Debug)]
pub enum NoDatabase {
    /// The SQL engine itself could not open or confine a database.
    EngineUnavailable(String),
    /// This emulator already holds [`MAX_OPEN_DATABASES`] databases.
    TooManyOpen(usize),
}

/// One open database, its connection's immutable `RedshiftPid`, and when a
/// statement last ran on it so an idle one can be closed instead of being held
/// for the life of the process.
///
/// The pid is cached HERE rather than read back off the connection: reading it
/// would mean taking the engine's mutex while the service's state lock is held,
/// and that mutex is held for a whole execution — so one slow statement would
/// stall every other operation of this service behind the state lock.
#[derive(Debug)]
struct OpenDatabase {
    handle: DatabaseHandle,
    pid: i64,
    last_used: SystemTime,
    /// The warehouse this database belongs to, held beside the handle so the
    /// databases of ONE warehouse can be enumerated without parsing the map's
    /// key back apart. See [`ComAmazonawsRedshiftdataRedshiftdataState::warehouse_handles`].
    target: ConnectionTarget,
    /// This database's name within that warehouse, held for the same reason.
    database: String,
    /// Resolves when the submission queued LAST on this database has reported,
    /// so the next one queued waits for it and the database's submissions run
    /// in submission order. See [`QueueSlot`].
    tail: Option<tokio::sync::oneshot::Receiver<()>>,
}

/// The place one submission holds in its database's queue.
///
/// Statements of one database run ONE AT A TIME — a transactional batch holds
/// the database for all of its statements — and they run in the order they were
/// submitted: a submission waits for [`wait_for_turn`](Self::wait_for_turn),
/// which resolves once every submission queued earlier on that database has
/// finished with it. The chain is built while the state lock is held, so the
/// order is the SUBMISSION order and never the order in which the executions
/// happen to be scheduled.
///
/// The turn is handed on when the slot is DROPPED, so every way out releases
/// it: an execution that finished, one whose task panicked or was cancelled,
/// and a submission that was never admitted at all. Nothing waits behind a slot
/// whose holder is gone.
#[derive(Debug)]
pub struct QueueSlot {
    /// Resolves when the submission queued immediately before this one on the
    /// same database has finished. `None` for the first submission on it.
    after: Option<tokio::sync::oneshot::Receiver<()>>,
    /// Dropped when this slot is, which releases the next submission queued on
    /// the same database.
    _done: tokio::sync::oneshot::Sender<()>,
}

impl QueueSlot {
    /// Wait for this submission's turn on its database.
    ///
    /// A sender dropped without a send (a submission that was never admitted,
    /// or an execution whose task panicked) is the same answer as a send: the
    /// queue moves on rather than stalling behind a submission that will never
    /// report.
    pub async fn wait_for_turn(&mut self) {
        if let Some(after) = self.after.take() {
            let _ = after.await;
        }
    }
}

/// Take the next place in a database's queue: this submission waits for the one
/// queued before it, and becomes the one the next submission waits for.
fn queue_slot(tail: &mut Option<tokio::sync::oneshot::Receiver<()>>) -> QueueSlot {
    let (done, next) = tokio::sync::oneshot::channel();
    QueueSlot {
        after: tail.replace(next),
        _done: done,
    }
}

/// Per-service state for com-amazonaws-redshiftdata-redshiftdata. A handler reaches it through [`service_state`].
///
/// Every map here grows on request, so every map here is bounded:
/// [`MAX_RETAINED_STATEMENTS`] and [`STATEMENT_RETENTION`] bound the statement
/// history (and the result sets it holds, and the client tokens that point into
/// it), [`MAX_ACTIVE_STATEMENTS`] bounds the submissions — and so the executions
/// and the tasks carrying them — in flight at once, and [`MAX_OPEN_DATABASES`]
/// bounds the engines and handles a caller can make the emulator open.
#[derive(Debug, Default)]
pub struct ComAmazonawsRedshiftdataRedshiftdataState {
    /// The retained statement history, by `Id`.
    statements: HashMap<String, Statement>,
    /// The live and recently closed sessions, by `SessionId`.
    sessions: HashMap<String, Session>,
    /// `ClientToken` -> `Id`, so an idempotent replay answers with the first
    /// statement rather than submitting a second one. An entry is dropped with
    /// the statement it points at, so this cannot outgrow the history.
    client_tokens: HashMap<String, String>,
    /// The SQL engine, one database per connection target and database name.
    databases: HashMap<String, OpenDatabase>,
    /// The databases each warehouse holds because a caller CREATED them, by
    /// warehouse key ([`ConnectionTarget::key`]) and in name order.
    ///
    /// A warehouse's catalogue is the database its own control plane provisioned
    /// it with plus these, and that catalogue is what a statement's `Database`
    /// is resolved against — so it outlives the ENGINES in `databases`, which
    /// are opened on first use and closed again when nothing has used them: a
    /// database a caller created does not stop existing because it went idle.
    catalogue: HashMap<String, BTreeSet<String>>,
    /// The next submission sequence number.
    next_seq: u64,
    /// The next session sequence number.
    next_session_seq: u64,
    /// The next `RedshiftQueryId`.
    next_query_id: i64,
    /// The next `RedshiftPid`.
    next_pid: i64,
    /// The status changes waiting to be published to EventBridge, oldest first:
    /// one per submission that asked for an event and has stopped.
    ///
    /// Bounded by [`statement_events::MAX_PENDING_EVENTS`] and drained by the
    /// call that produced its entries
    /// ([`statement_events::publish_pending`]), so it holds only what has not
    /// been published yet.
    statement_events: Vec<statement_events::StatementEvent>,
    /// What the CONNECTIONS of this service's statements hold: each live
    /// session's own temporary relations, and the locks the sessions hold (see
    /// [`connections`]).
    connections: Connections,
}

impl ComAmazonawsRedshiftdataRedshiftdataState {
    /// Queue the status change `event` reports for publication, dropping the
    /// OLDEST queued event once the queue is full.
    ///
    /// Every terminal transition of a submission that asked for an event goes
    /// through here, so one status change is queued exactly once however it was
    /// reached, and the queue cannot grow without bound.
    fn queue_statement_event(&mut self, event: statement_events::StatementEvent) {
        self.statement_events.push(event);
        let queued = self.statement_events.len();
        if queued > statement_events::MAX_PENDING_EVENTS {
            self.statement_events
                .drain(..queued - statement_events::MAX_PENDING_EVENTS);
        }
    }

    /// Take at most `limit` queued status changes out, oldest first, for
    /// publication — which happens with no lock held.
    pub fn take_statement_events(
        &mut self,
        limit: usize,
    ) -> Vec<statement_events::StatementEvent> {
        let taken = self.statement_events.len().min(limit);
        self.statement_events.drain(..taken).collect()
    }

    /// How many status changes are still waiting to be published.
    pub fn pending_statement_event_count(&self) -> usize {
        self.statement_events.len()
    }

    /// Materialise every outcome a submission's execution has already reported.
    ///
    /// Every operation calls this before it reads, so an outcome the engine
    /// produced is reported from the instant it produced it, whether or not
    /// anyone called in between.
    pub fn materialise_due_transitions(&mut self) {
        let now = Instant::now();
        for statement in self.statements.values_mut() {
            statement.advance(now);
        }
        // A session the clock has closed can no longer run SQL statements, so
        // its connection is closed and holds none of its locks: a release the
        // clock owes at the same instant whether or not anybody called.
        self.release_closed_connections();
    }

    /// How many submissions are running or queued: those whose execution has not
    /// reported yet. Counts SUBMISSIONS, since a batch's statements run inside
    /// the one execution its batch carries.
    pub fn active_statements(&self) -> usize {
        self.statements
            .values()
            .filter(|statement| !statement.status.is_terminal())
            .count()
    }

    /// Report that the submission's turn on its database is now its: the query
    /// "has been chosen to be run", which is PICKED.
    pub fn pick(&mut self, id: &str) {
        if let Some(statement) = self.statements.get_mut(id) {
            statement.reach(StatementStatus::Picked, SystemTime::now());
        }
    }

    /// Report that the engine has begun running the submission: the query "run
    /// has started", which is STARTED. A statement reports it for exactly as
    /// long as the engine is really running its work, which is what makes a long
    /// query observable as running — and cancellable — rather than only
    /// reported once it has stopped.
    pub fn start(&mut self, id: &str) {
        if let Some(statement) = self.statements.get_mut(id) {
            statement.reach(StatementStatus::Started, SystemTime::now());
        }
    }

    /// A `RedshiftPid` for a connection that is not an open database of this
    /// emulator: the backend a statement whose connection FAILS was answered
    /// by. Drawn from the same sequence every open database draws from, so no
    /// two connections ever report one pid.
    pub fn mint_pid(&mut self) -> i64 {
        if self.next_pid == 0 {
            self.next_pid = FIRST_PID;
        }
        let pid = self.next_pid;
        self.next_pid += 1;
        pid
    }

    /// The statement with this `Id`.
    ///
    /// The `Id` may name ONE statement of a batch: `SubStatementData.Id`
    /// documents that "a suffix indicates the number of the SQL statement" —
    /// `<batch id>:2` "indicates the second SQL statement of a batch query" —
    /// and the `UUID` pattern allows exactly that suffix. So the suffix is
    /// resolved against the batch's statements, in submission order, and a
    /// batch's statement is readable through every operation that takes an
    /// `Id`.
    pub fn statement(&self, id: &str) -> Option<&Statement> {
        if let Some(statement) = self.statements.get(id) {
            return Some(statement);
        }
        let (batch_id, position) = split_batch_member(id)?;
        self.statements
            .get(batch_id)
            .filter(|batch| batch.is_batch)
            .and_then(|batch| batch.sub_statements.get(position.checked_sub(1)?))
    }

    /// The `Id` a previous `ExecuteStatement` with this `ClientToken` minted.
    pub fn statement_id_for_client_token(&self, token: &str) -> Option<&str> {
        self.client_tokens.get(token).map(String::as_str)
    }

    /// Re-record the SQL and the `Parameters` a submission carries onto the
    /// statement an earlier submission with the same `ClientToken` already
    /// minted.
    ///
    /// A replayed token RUNS nothing: the statement keeps the execution the
    /// first submission gave it, so its status, timestamps, duration, counts and
    /// result set all stay as that execution left them. What the record reports
    /// as the statement's text and parameters is the LAST request submitted
    /// under the token, which is what the service reports for a token replayed
    /// with a different request — so a caller reading the record back sees the
    /// request it last sent rather than one the service silently ignored.
    ///
    /// A replay mints no statement either, so a submission is re-recorded only
    /// when it carries exactly the statements the record already holds — one for
    /// a single statement, one per statement of a batch. A submission naming a
    /// different number of them is not a re-submission of those statements, so
    /// the record keeps the text and the parameters it has rather than reporting
    /// some statements from one request and the rest from another.
    pub fn rewrite_submitted_sql(
        &mut self,
        id: &str,
        statements: &[String],
        parameters: &[SqlParameter],
    ) {
        let Some(statement) = self.statements.get_mut(id) else {
            return;
        };
        let recorded = if statement.is_batch {
            statement.sub_statements.len()
        } else {
            1
        };
        if statements.len() != recorded {
            return;
        }
        statement.parameters = parameters.to_vec();
        if statement.is_batch {
            for (sub_statement, sql) in statement.sub_statements.iter_mut().zip(statements) {
                sub_statement.sql = sql.clone();
                sub_statement.parameters = parameters.to_vec();
            }
        } else if let Some(sql) = statements.first() {
            statement.sql = sql.clone();
        }
    }

    /// Every retained statement, newest submission first — the order
    /// `ListStatements` enumerates in.
    pub fn statements_newest_first(&self) -> Vec<&Statement> {
        let mut statements: Vec<&Statement> = self.statements.values().collect();
        statements.sort_unstable_by_key(|statement| std::cmp::Reverse(statement.seq));
        statements
    }

    /// Stop a still-running statement, reporting what this call found: one
    /// lookup decides the outcome AND carries the status that was observed, so
    /// no caller re-reads the statement to describe what happened.
    pub fn cancel(&mut self, id: &str) -> Cancellation {
        if let Some(statement) = self.statements.get_mut(id) {
            let observed = statement.status;
            if !statement.cancel(SystemTime::now()) {
                return Cancellation::AlreadyStopped(observed);
            }
            // Stopping a statement is a status change of it too, so a submission
            // that asked to be told how it ended is told that it was aborted.
            let event = if statement.with_event {
                Some(statement_events::StatementEvent::of(statement))
            } else {
                None
            };
            if let Some(event) = event {
                self.queue_statement_event(event);
            }
            return Cancellation::Cancelled;
        }
        // One statement of a batch does not run on its own: the batch was
        // submitted as one request and its statements run serially inside it,
        // so what a caller stops is the batch.
        if let Some((batch_id, position)) = split_batch_member(id) {
            let names_a_member = self.statements.get(batch_id).is_some_and(|batch| {
                batch.is_batch && position >= 1 && position <= batch.sub_statements.len()
            });
            if names_a_member {
                return Cancellation::BatchMember(batch_id.to_string());
            }
        }
        Cancellation::Unknown
    }

    /// Every session this emulator holds, newest first — the order
    /// `ListSessions` enumerates in.
    pub fn sessions_newest_first(&self) -> Vec<&Session> {
        let mut sessions: Vec<&Session> = self.sessions.values().collect();
        sessions.sort_unstable_by_key(|session| std::cmp::Reverse(session.seq));
        sessions
    }

    /// The session with this `SessionId`, live or closed.
    pub fn session(&self, id: &str) -> Option<&Session> {
        self.sessions.get(id)
    }

    /// The connection a LIVE session was opened with — what a statement naming
    /// only a `SessionId` runs against. `None` for a `SessionId` no session
    /// carries, and for one the clock has already closed: a closed session can
    /// no longer run SQL statements.
    pub fn session_connection(&self, id: &str) -> Option<Connection> {
        let session = self
            .sessions
            .get(id)
            .filter(|session| !session.is_closed(SystemTime::now()))?;
        Some(Connection {
            target: session.target.clone(),
            database: session.database.clone(),
            db_user: session.db_user.clone(),
            secret_arn: session.secret_arn.clone(),
        })
    }

    /// The status a session reports now, and the statement that makes it BUSY.
    ///
    /// Derived, never stored: a session is CLOSED once the clock has passed its
    /// `SessionTtl`, BUSY while a statement submitted in it has not reached its
    /// terminal status, and AVAILABLE otherwise. Call
    /// [`Self::materialise_due_transitions`] first, so the statuses this reads
    /// are the ones the clock already owes.
    pub fn session_activity(&self, session: &Session) -> (SessionStatus, Option<String>) {
        if session.is_closed(SystemTime::now()) {
            return (SessionStatus::Closed, None);
        }
        let running = self
            .statements
            .values()
            .filter(|statement| statement.session_id.as_deref() == Some(session.id.as_str()))
            .filter(|statement| !statement.status.is_terminal())
            .max_by_key(|statement| statement.seq)
            .map(|statement| statement.id.clone());
        match running {
            // "This element is returned only when the session status is BUSY."
            Some(id) => (SessionStatus::Busy, Some(id)),
            None => (SessionStatus::Available, None),
        }
    }

    /// Open the session a submission asked to keep alive, or extend the one it
    /// ran in, and report the `SessionId` its statements record.
    ///
    /// A submission that named neither gets none: a session exists only because
    /// a caller asked for one.
    fn touch_session(&mut self, submission: &Submission, now: SystemTime) -> Option<String> {
        if let Some(id) = submission.session_id.clone() {
            if let Some(session) = self.sessions.get_mut(&id) {
                // "The number of seconds to keep the session alive after the
                // query finishes": the statement has run, so the keep-alive is
                // measured from here, and a statement that asked for a new one
                // changes the session's keep-alive to it.
                if let Some(seconds) = submission.session_keep_alive_seconds {
                    session.keep_alive_seconds = seconds;
                }
                session.updated_at = now;
                session.expires_at = now + session.keep_alive();
            }
            return Some(id);
        }

        let keep_alive_seconds = submission.session_keep_alive_seconds?;
        self.next_session_seq += 1;
        let id = new_uuid();
        let session = Session {
            id: id.clone(),
            seq: self.next_session_seq,
            target: submission.target.clone(),
            database: submission.database.clone(),
            db_user: submission.db_user.clone(),
            secret_arn: submission.secret_arn.clone(),
            created_at: now,
            updated_at: now,
            keep_alive_seconds,
            expires_at: now + Duration::from_secs(keep_alive_seconds.unsigned_abs().into()),
        };
        self.sessions.insert(id.clone(), session);
        self.evict_expired_sessions(now);
        Some(id)
    }

    /// Drop the sessions this service no longer reports: those closed longer
    /// ago than [`SESSION_RETENTION`] — "Lists the sessions that the caller
    /// created in the last 24 hours" — and then the oldest while more than
    /// [`MAX_RETAINED_SESSIONS`] remain, so a caller repeating
    /// `ExecuteStatement` cannot make the emulator hold sessions without limit.
    fn evict_expired_sessions(&mut self, now: SystemTime) {
        self.sessions.retain(|_, session| {
            !now.duration_since(session.created_at)
                .is_ok_and(|age| age > SESSION_RETENTION)
        });
        while self.sessions.len() > MAX_RETAINED_SESSIONS {
            let Some(oldest) = self
                .sessions
                .values()
                .min_by_key(|session| session.seq)
                .map(|session| session.id.clone())
            else {
                break;
            };
            self.sessions.remove(&oldest);
        }
    }

    /// A handle on one database of `target` this emulator ALREADY holds open,
    /// for a reporting operation: `None` when no statement has opened it, so a
    /// read never opens an engine a later caller would then find empty — and
    /// never spends one of the [`MAX_OPEN_DATABASES`] a submission may need.
    pub fn open_database(
        &self,
        target: &ConnectionTarget,
        database: &str,
    ) -> Option<DatabaseHandle> {
        self.databases
            .get(&target.database_key(database))
            .map(|open| std::sync::Arc::clone(&open.handle))
    }

    /// The names of the databases of `target` this emulator HOLDS, in name
    /// order.
    ///
    /// One part of the warehouse's catalogue: a database is held here because a
    /// statement ran against it (see [`Self::reserve_database`]), while the
    /// database the warehouse was PROVISIONED with belongs to the control plane
    /// that owns the warehouse — [`warehouse::catalogue_databases`] is where the
    /// two become the one list `ListDatabases` enumerates.
    ///
    /// A reporting enumeration, so it opens nothing: a read that opened a
    /// database would add one to the warehouse every later caller then found
    /// there.
    pub fn database_names(&self, target: &ConnectionTarget) -> Vec<String> {
        let mut names: Vec<String> = self
            .warehouse_handles(target)
            .into_iter()
            .map(|(database, _handle)| database)
            .collect();
        names.extend(
            self.catalogue
                .get(&target.key())
                .into_iter()
                .flatten()
                .cloned(),
        );
        names.sort();
        names.dedup();
        names
    }

    /// Record the databases the warehouse's own control plane reports it was
    /// provisioned with, so this service's catalogue of that warehouse names
    /// every database of it: the ones it was created with and the ones callers
    /// have created since.
    ///
    /// Best effort by design: a catalogue already at
    /// [`MAX_CATALOGUE_DATABASES`] records nothing more, and a provisioned
    /// database is a database of the warehouse whether or not it was recorded —
    /// its owner is what says so.
    pub fn hold_databases(&mut self, target: &ConnectionTarget, provisioned: &[String]) {
        for database in provisioned {
            if self.holds_database(target, database) || self.catalogued() >= MAX_CATALOGUE_DATABASES
            {
                continue;
            }
            self.catalogue
                .entry(target.key())
                .or_default()
                .insert(database.clone());
        }
    }

    /// Whether this service's catalogue of `target` names a database of this
    /// name: one it holds an engine for, or one its catalogue records.
    pub fn holds_database(&self, target: &ConnectionTarget, database: &str) -> bool {
        self.databases
            .contains_key(&target.database_key(database))
            || self
                .catalogue
                .get(&target.key())
                .is_some_and(|names| names.contains(database))
    }

    /// How many databases the catalogue names across every warehouse — what
    /// [`MAX_CATALOGUE_DATABASES`] bounds.
    fn catalogued(&self) -> usize {
        self.catalogue.values().map(BTreeSet::len).sum()
    }

    /// Carry out what each of a submission's statements asked of the
    /// WAREHOUSE's catalogue of databases, and report each statement as the
    /// catalogue answered it.
    ///
    /// Applied HERE, under the state lock that holds the catalogue, because the
    /// engine a statement runs on is one database and has no notion of the
    /// warehouse's others — and applied before the statement's outcome is
    /// materialised, so no caller reads a `CREATE DATABASE` as FINISHED before
    /// the database it creates is in the catalogue.
    fn apply_catalogue(
        &mut self,
        target: &ConnectionTarget,
        executions: Vec<Execution>,
    ) -> Vec<Execution> {
        executions
            .into_iter()
            .map(|mut execution| {
                let effects = std::mem::take(&mut execution.catalogue);
                if execution.status != StatementStatus::Finished {
                    return execution;
                }
                for effect in &effects {
                    if let Err(message) = self.apply_effect(target, effect) {
                        let mut refused = Execution::failed(message);
                        // The statement ran for as long as it ran, whatever the
                        // catalogue then answered.
                        refused.duration_nanos = execution.duration_nanos;
                        return refused;
                    }
                }
                execution
            })
            .collect()
    }

    /// Carry out ONE catalogue effect, or report the message the data warehouse
    /// refuses it with.
    fn apply_effect(
        &mut self,
        target: &ConnectionTarget,
        effect: &CatalogueEffect,
    ) -> Result<(), String> {
        match effect {
            CatalogueEffect::CreateDatabase(name) => {
                if self.holds_database(target, name) {
                    return Err(format!("ERROR: database \"{name}\" already exists"));
                }
                if self.catalogued() >= MAX_CATALOGUE_DATABASES {
                    return Err(format!(
                        "ERROR: database limit exceeded: this emulator's warehouses hold at most \
                         {MAX_CATALOGUE_DATABASES} databases between them"
                    ));
                }
                self.catalogue
                    .entry(target.key())
                    .or_default()
                    .insert(name.clone());
                Ok(())
            }
        }
    }

    /// Resolve the database a submission will run on, opening and confining it on
    /// first use, and report the `RedshiftPid` of its connection and the
    /// submission's place in that database's queue.
    ///
    /// Returns a handle, not a borrow, so the caller can release the state lock
    /// before running anything on it.
    pub fn reserve_database(
        &mut self,
        target: &ConnectionTarget,
        database: &str,
    ) -> Result<(DatabaseHandle, i64, QueueSlot), NoDatabase> {
        let now = SystemTime::now();
        // Close what nothing has used for a full retention span first, so memory
        // a statement committed inside the engine is reclaimed rather than held
        // for the life of the process.
        self.close_idle_databases(now);

        let key = target.database_key(database);
        if let Some(open) = self.databases.get_mut(&key) {
            open.last_used = now;
            // No engine lock is taken here: the pid is cached on the entry, so
            // resolving a database never waits on an execution.
            return Ok((
                std::sync::Arc::clone(&open.handle),
                open.pid,
                queue_slot(&mut open.tail),
            ));
        }
        // The key is built from caller-chosen names, so a new name must not be
        // able to open another engine without limit.
        if self.databases.len() >= MAX_OPEN_DATABASES {
            return Err(NoDatabase::TooManyOpen(MAX_OPEN_DATABASES));
        }
        let opened = Database::open()
            .map_err(|error| NoDatabase::EngineUnavailable(scrub_paths(&error.to_string())))?;
        let pid = self.mint_pid();
        let handle: DatabaseHandle = std::sync::Arc::new(Mutex::new(opened));
        let mut entry = OpenDatabase {
            handle: std::sync::Arc::clone(&handle),
            pid,
            last_used: now,
            target: target.clone(),
            database: database.to_string(),
            tail: None,
        };
        let slot = queue_slot(&mut entry.tail);
        self.databases.insert(key, entry);
        Ok((handle, pid, slot))
    }

    /// Close every database no statement has run on for
    /// [`DATABASE_IDLE_TIMEOUT`]. Dropping the entry drops the connection, which
    /// releases both of its page-bounded schemas back to the allocator.
    fn close_idle_databases(&mut self, now: SystemTime) {
        self.databases.retain(|_, open| {
            !now.duration_since(open.last_used)
                .is_ok_and(|idle| idle > DATABASE_IDLE_TIMEOUT)
        });
    }

    /// A handle on every database of `target` this emulator holds open, by
    /// database name and in name order.
    ///
    /// Handles, not borrows, and nothing is locked here: a caller releases the
    /// state lock before it touches a database, because an execution holds its
    /// database's mutex and only THEN takes this lock to record its result — so
    /// waiting for a database mutex with this lock held would deadlock the pair.
    fn warehouse_handles(&self, target: &ConnectionTarget) -> Vec<(String, DatabaseHandle)> {
        let mut handles: Vec<(String, DatabaseHandle)> = self
            .databases
            .values()
            .filter(|open| &open.target == target)
            .map(|open| (open.database.clone(), std::sync::Arc::clone(&open.handle)))
            .collect();
        handles.sort_by(|(left, _), (right, _)| left.cmp(right));
        handles
    }

    /// Whether this emulator can hold `databases` of `target` open at once, asked
    /// BEFORE a restore copies anything.
    ///
    /// The databases of `target` are replaced wholesale by a restore, so they do
    /// not count against [`MAX_OPEN_DATABASES`]; every other database the emulator
    /// holds open does.
    fn room_for_databases(
        &self,
        target: &ConnectionTarget,
        databases: usize,
    ) -> Result<(), NoDatabase> {
        let others = self
            .databases
            .values()
            .filter(|open| &open.target != target)
            .count();
        if others + databases > MAX_OPEN_DATABASES {
            return Err(NoDatabase::TooManyOpen(MAX_OPEN_DATABASES));
        }
        Ok(())
    }

    /// Replace the databases of `target` with `restored`, in ONE step that cannot
    /// fail: the warehouse goes from the databases it held straight to exactly
    /// these, so no caller observes a half-restored warehouse and a database the
    /// restore does not name is not in the restored warehouse either.
    ///
    /// Each restored database is a connection of its own, so it carries a new
    /// `RedshiftPid` — a restore gives the warehouse new connections. A statement
    /// still running holds the handle it was given and finishes on it; the next
    /// statement sees the restored database.
    fn install_databases(&mut self, target: &ConnectionTarget, restored: Vec<(String, Database)>) {
        let now = SystemTime::now();
        self.databases.retain(|_, open| &open.target != target);
        for (database, engine) in restored {
            let pid = self.mint_pid();
            self.databases.insert(
                target.database_key(&database),
                OpenDatabase {
                    handle: std::sync::Arc::new(Mutex::new(engine)),
                    pid,
                    last_used: now,
                    target: target.clone(),
                    database,
                    tail: None,
                },
            );
        }
    }

    /// Record a submission whose execution is about to run, as SUBMITTED, and
    /// return its `Id`.
    ///
    /// Nothing about the outcome is recorded here, because nothing has run yet:
    /// the statement reports SUBMITTED until its execution reports through
    /// [`Self::complete`]. A batch is recorded as ONE statement with one
    /// substatement per statement it submitted, so every statement of it is
    /// readable from the moment the batch is accepted.
    ///
    /// `interrupt` abandons that execution, so `CancelStatement` stops the
    /// engine rather than only relabelling the record.
    pub fn record_pending(
        &mut self,
        submission: Submission,
        pid: i64,
        interrupt: Interrupt,
    ) -> String {
        let id = new_uuid();
        let created_at = SystemTime::now();
        self.next_seq += 1;

        // The session this submission runs in: the one it asked to keep alive,
        // opened here, or the one it named, whose expiry now runs from this
        // statement. The statements record what the SESSION STORE says, so a
        // `SessionId` on a statement is one this service really holds.
        let session_id = self.touch_session(&submission, created_at);

        let shared = Shared {
            submission: &submission,
            session_id,
            created_at,
            seq: self.next_seq,
            pid,
        };

        let statement = if submission.kind.is_batch() {
            let mut sub_statements = Vec::with_capacity(submission.statements.len());
            for (index, sql) in submission.statements.iter().enumerate() {
                self.next_query_id += 1;
                sub_statements.push(build_statement(
                    &shared,
                    Recorded {
                        // "A suffix indicates the number of the SQL statement",
                        // counted from one in submission order.
                        id: format!("{id}:{}", index + 1),
                        sql: sql.clone(),
                        query_uuid: Some(new_uuid()),
                        redshift_query_id: self.next_query_id,
                        client_token: None,
                        is_batch: false,
                        sub_statements: Vec::new(),
                        interrupt: None,
                    },
                ));
            }
            build_statement(
                &shared,
                Recorded {
                    id: id.clone(),
                    // A batch has no single query string; each of its
                    // statements carries its own.
                    sql: String::new(),
                    // Nor is a batch itself a query the cluster ran, so it
                    // carries none of the identifiers a query gets.
                    query_uuid: None,
                    redshift_query_id: 0,
                    client_token: submission.client_token.clone(),
                    is_batch: true,
                    sub_statements,
                    interrupt: Some(interrupt),
                },
            )
        } else {
            self.next_query_id += 1;
            build_statement(
                &shared,
                Recorded {
                    id: id.clone(),
                    sql: submission.statements.first().cloned().unwrap_or_default(),
                    query_uuid: Some(new_uuid()),
                    redshift_query_id: self.next_query_id,
                    client_token: submission.client_token.clone(),
                    is_batch: false,
                    sub_statements: Vec::new(),
                    interrupt: Some(interrupt),
                },
            )
        };

        if let Some(token) = submission.client_token {
            self.client_tokens.insert(token, id.clone());
        }
        self.statements.insert(id.clone(), statement);
        self.evict_expired_statements(created_at);
        id
    }

    /// Record what a submission's execution produced.
    ///
    /// `executions` carries one entry per statement of the submission, in
    /// submission order, exactly as [`run_on`] and [`run_batch_on`] report them.
    /// A batch's own status, error, duration and `HasResultSet` are its
    /// statements' — never values of its own.
    ///
    /// A statement a caller already cancelled keeps the ABORTED it was given:
    /// the engine was abandoned, so whatever it managed to report is not an
    /// outcome the caller may read as this statement's.
    pub fn complete(&mut self, id: &str, executions: Vec<Execution>) {
        let Some(target) = self
            .statements
            .get(id)
            .filter(|statement| !statement.status.is_terminal())
            .map(|statement| statement.target.clone())
        else {
            return;
        };
        // What a statement asked of the warehouse's catalogue is carried out
        // before its outcome is materialised, and the statement reports what the
        // catalogue answered.
        let executions = self.apply_catalogue(&target, executions);
        // And what its statements did to their CONNECTION: the temporary
        // relations they created are the connection's from here on, and the
        // locks they took are held against the other connections — both
        // recorded before the outcome is materialised, so no caller reads the
        // statement as FINISHED before what it built is reachable.
        let mut created: Vec<String> = Vec::new();
        let mut locks: Vec<LockEffect> = Vec::new();
        for execution in &executions {
            created.extend(execution.connection.temp_created.iter().cloned());
            locks.extend(execution.connection.locks.iter().cloned());
        }
        self.hold_temp_relations(id, &created);
        self.apply_lock_effects(id, &locks);
        let Some(statement) = self.statements.get_mut(id) else {
            return;
        };
        if statement.status.is_terminal() {
            return;
        }
        let now = Instant::now();
        let now_wall = SystemTime::now();

        // One entry per submitted statement, whatever the engine reported.
        let wanted = if statement.is_batch {
            statement.sub_statements.len()
        } else {
            1
        };
        let mut executions = executions;
        executions.truncate(wanted);
        while executions.len() < wanted {
            executions.push(Execution::never_ran());
        }

        if statement.is_batch {
            let outcome = batch_outcome(&executions);
            for (sub_statement, execution) in
                statement.sub_statements.iter_mut().zip(executions)
            {
                sub_statement.apply(execution, now, now_wall);
            }
            statement.apply(outcome, now, now_wall);
        } else {
            let execution = executions
                .into_iter()
                .next()
                .unwrap_or_else(Execution::never_ran);
            statement.apply(execution, now, now_wall);
        }

        // A submission that asked to be TOLD how it ended has the status change
        // it has just made queued for publication, read off the record this
        // call wrote; one that asked for nothing queues nothing.
        let event = if statement.with_event {
            Some(statement_events::StatementEvent::of(statement))
        } else {
            None
        };
        if let Some(event) = event {
            self.queue_statement_event(event);
        }

        // The result this submission retains is part of what the history holds,
        // so the history is brought back inside its bounds as the outcome lands
        // rather than only when something else is submitted.
        self.evict_expired_statements(now_wall);
    }

    /// Drop the statements this service no longer retains: those past
    /// [`STATEMENT_RETENTION`], then the oldest submissions that have REPORTED
    /// while more than [`MAX_RETAINED_STATEMENTS`] statements remain — a batch
    /// counting as itself plus each of its statements, since each of those holds
    /// a result set of its own. Each eviction drops that submission's result
    /// sets and its client token with it, so neither map outgrows the history.
    ///
    /// A submission whose execution has not reported is not evicted to make
    /// room: it is the record that reports and cancels that execution, and it is
    /// what [`MAX_ACTIVE_STATEMENTS`] counts, so dropping it would leave work
    /// running that nothing could report, cancel or count. Past the retention
    /// span it IS dropped, and [`Self::forget`] abandons its execution with it.
    fn evict_expired_statements(&mut self, now: SystemTime) {
        let expired: Vec<String> = self
            .statements
            .values()
            .filter(|statement| {
                now.duration_since(statement.created_at)
                    .is_ok_and(|age| age > STATEMENT_RETENTION)
            })
            .map(|statement| statement.id.clone())
            .collect();
        for id in expired {
            self.forget(&id);
        }

        while self.retained_statement_count() > MAX_RETAINED_STATEMENTS {
            let Some(oldest) = self
                .statements
                .values()
                .filter(|statement| statement.status.is_terminal())
                .min_by_key(|statement| statement.seq)
                .map(|statement| statement.id.clone())
            else {
                break;
            };
            self.forget(&oldest);
        }

        // And the RESULTS the history holds: a caller repeating a large query
        // would otherwise make the emulator hold result sets up to the number of
        // statements it retains times what one of them may retain, so the oldest
        // reported submissions are dropped until the history is inside the
        // bound.
        while self.retained_result_bytes() > MAX_RETAINED_RESULT_BYTES {
            let Some(oldest) = self
                .statements
                .values()
                .filter(|statement| {
                    statement.status.is_terminal() && statement.retained_bytes() > 0
                })
                .min_by_key(|statement| statement.seq)
                .map(|statement| statement.id.clone())
            else {
                break;
            };
            self.forget(&oldest);
        }
    }

    /// What holding the retained statements' results costs this process — what
    /// [`MAX_RETAINED_RESULT_BYTES`] bounds.
    fn retained_result_bytes(&self) -> usize {
        self.statements
            .values()
            .fold(0_usize, |total, statement| {
                total.saturating_add(statement.retained_bytes())
            })
    }

    /// How many statements the history holds: every retained submission, plus
    /// each statement of a batch, because each of those retains a result set of
    /// its own.
    fn retained_statement_count(&self) -> usize {
        self.statements
            .values()
            .map(|statement| 1 + statement.sub_statements.len())
            .sum()
    }

    /// Remove one statement and everything that points at it. A batch's
    /// statements go with the batch they belong to, since they are held on it.
    ///
    /// A submission dropped while its execution has not reported has that
    /// execution ABANDONED: the engine stops it at its next progress callback,
    /// so no work keeps running once the record that could report or cancel it
    /// is gone.
    fn forget(&mut self, id: &str) {
        if let Some(mut statement) = self.statements.remove(id) {
            if let Some(interrupt) = statement.interrupt.take() {
                interrupt.now();
            }
            if let Some(token) = statement.client_token {
                self.client_tokens.remove(&token);
            }
        }
    }
}

/// What every statement of ONE submission shares: its connection, its database,
/// its parameters and its submission instant.
struct Shared<'a> {
    submission: &'a Submission,
    session_id: Option<String>,
    created_at: SystemTime,
    seq: u64,
    pid: i64,
}

/// What is a statement's OWN: its `Id`, its text, the identifiers Redshift
/// generated for it, the handle that abandons its execution, and — for a batch —
/// the statements it submitted.
struct Recorded {
    id: String,
    sql: String,
    query_uuid: Option<String>,
    redshift_query_id: i64,
    client_token: Option<String>,
    is_batch: bool,
    sub_statements: Vec<Statement>,
    interrupt: Option<Interrupt>,
}

/// Assemble one stored statement, as SUBMITTED and with nothing reported about
/// its outcome, from what its submission shares and what is its own. A batch and
/// each statement of it are built through here, so neither can drift from the
/// other on the connection they run under.
fn build_statement(shared: &Shared<'_>, recorded: Recorded) -> Statement {
    let submission = shared.submission;
    Statement {
        id: recorded.id,
        query_uuid: recorded.query_uuid,
        seq: shared.seq,
        sql: recorded.sql,
        statement_name: submission.statement_name.clone(),
        database: Some(submission.database.clone()),
        db_user: submission.db_user.clone(),
        secret_arn: submission.secret_arn.clone(),
        target: submission.target.clone(),
        status: StatementStatus::Submitted,
        terminal_status: StatementStatus::Submitted,
        due_at: None,
        completed_at: shared.created_at,
        interrupt: recorded.interrupt,
        error: None,
        created_at: shared.created_at,
        updated_at: shared.created_at,
        duration_nanos: 0,
        redshift_pid: shared.pid,
        redshift_query_id: recorded.redshift_query_id,
        has_result_set: false,
        result_rows: 0,
        result_size: 0,
        with_event: submission.with_event,
        region: submission.region.clone(),
        principal: submission.principal.clone(),
        resource_arn: submission.resource_arn.clone(),
        result_format: submission.result_format.clone(),
        parameters: submission.parameters.clone(),
        session_id: shared.session_id.clone(),
        client_token: recorded.client_token,
        result: None,
        retained_bytes: 0,
        is_batch: recorded.is_batch,
        execution_mode: submission.kind.execution_mode(),
        sub_statements: recorded.sub_statements,
    }
}

/// What a batch as a whole did, from what its statements' executions reported.
///
/// A batch computes nothing of its own: it FAILED if any of its statements
/// failed, it stopped short if any of them never ran, and "the value [of
/// `HasResultSet`] is true if any substatement returns a result set". Its
/// `ResultRows` and `ResultSize` belong to its statements, so the batch reports
/// the modelled `-1` null marker for both.
fn batch_outcome(executions: &[Execution]) -> Execution {
    let failed = executions
        .iter()
        .any(|execution| execution.status == StatementStatus::Failed);
    let stopped = executions
        .iter()
        .any(|execution| execution.status == StatementStatus::Aborted);
    let status = match (failed, stopped) {
        (true, _) => StatementStatus::Failed,
        (false, true) => StatementStatus::Aborted,
        (false, false) => StatementStatus::Finished,
    };
    Execution {
        status,
        // The engine's own message from the statement that failed — never a
        // message this service wrote about the batch.
        error: executions
            .iter()
            .find_map(|execution| execution.error.clone()),
        duration_nanos: executions.iter().fold(0_i64, |total, execution| {
            total.saturating_add(execution.duration_nanos)
        }),
        has_result_set: executions.iter().any(|execution| execution.has_result_set),
        result_rows: -1,
        result_size: -1,
        result: None,
        // A batch holds no result of its own either: its statements each hold
        // theirs, and each of those is charged for itself.
        retained_bytes: 0,
        // A batch does nothing of its own to a catalogue either: each statement
        // of it carries its own effect, and each one is applied for itself.
        catalogue: Vec::new(),
        // Nor to the connection they all ran on: each statement of the batch
        // reports what it did to that connection.
        connection: ConnectionOutcome::default(),
    }
}

/// The batch `Id` and the 1-based statement number an `Id` of the form
/// `<batch id>:<n>` names, or `None` for an `Id` that carries no such suffix.
fn split_batch_member(id: &str) -> Option<(&str, usize)> {
    let (batch_id, suffix) = id.rsplit_once(':')?;
    Some((batch_id, suffix.parse().ok()?))
}

/// How long a request waits for the statement it submitted before answering.
///
/// `WaitTimeSeconds` is modelled with the range 1..30, and its documentation
/// ("the number of seconds to wait for the SQL statement to complete execution
/// before returning the response. ... The maximum value is 30 seconds") makes it
/// the caller's own ceiling, so a caller that named one is held to it. A request
/// that named none waits for NOTHING: it answers with the statement as
/// SUBMITTED and the execution finishes on its own, which is what makes the
/// Data API asynchronous and two concurrent submissions concurrent.
pub fn wait_budget(wait_time_seconds: Option<i32>) -> Option<Duration> {
    wait_time_seconds
        .map(|seconds| Duration::from_secs(u64::from(seconds.clamp(1, 30).unsigned_abs())))
}

/// The engine's own wall-clock deadline for one submission's execution.
///
/// [`MAX_ENGINE_DEADLINE`] for every submission: the execution runs
/// independently of the request that submitted it, so what bounds it is the
/// engine's own deadline and never a caller's patience.
pub fn engine_timeout() -> Duration {
    MAX_ENGINE_DEADLINE
}

/// How long a REQUEST waits for a catalog read: the engine's own deadline plus a
/// grace period, so what ends the read is that deadline and not the request's
/// patience — a catalog read answers from the database the request named, so it
/// is served inside the request rather than left to finish on its own.
pub const CATALOG_REQUEST_BUDGET: Duration =
    MAX_ENGINE_DEADLINE.saturating_add(Duration::from_secs(3));

/// The step budget one execution runs under, given its wall-clock `deadline`.
///
/// Derived from the deadline at [`SQL_STEPS_PER_SECOND`], so the two bounds
/// scale together and the instruction count stays the backstop for a clock that
/// does not advance rather than a second, tighter limit of its own.
fn step_budget(deadline: Duration) -> u64 {
    let millis = u64::try_from(deadline.as_millis()).unwrap_or(u64::MAX);
    millis
        .saturating_mul(SQL_STEPS_PER_SECOND / 1_000)
        .max(u64::try_from(PROGRESS_HANDLER_OPS).unwrap_or(1))
}

/// An optional string member's value, treating an all-whitespace value as
/// absent. The Data API's connection and filter members are names, so a member
/// carrying only whitespace names nothing and is the same as not sending it.
pub fn non_empty(value: Option<&String>) -> Option<String> {
    let value = value?.trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// A fresh identifier in the form the `UUID` shape's pattern
/// `^[a-z0-9]{8}(-[a-z0-9]{4}){3}-[a-z0-9]{12}(:\d{0,2})?$` accepts.
pub fn new_uuid() -> String {
    uuid::Uuid::new_v4().as_hyphenated().to_string()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsRedshiftdataRedshiftdataState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsRedshiftdataRedshiftdataState>)
}
