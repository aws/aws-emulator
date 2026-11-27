// SPDX-License-Identifier: CC0-1.0

//! The SQL engine behind a DB instance's engine endpoint: one in-process SQLite
//! database per DATABASE of a DB instance.
//!
//! A statement a client sends over the engine's native wire is RUN here and the rows
//! this module returns are the rows the wire reports — nothing is synthesised from the
//! statement's text. The engine is SQLite (`rusqlite`, the `bundled` build already in
//! this workspace), which is not PostgreSQL and not MySQL: what it cannot do it refuses
//! with its own error, which the wire renders as that engine's error response, rather
//! than answering a fabricated success.
//!
//! A DATABASE (a MySQL schema, a PostgreSQL database) is a namespace of its own on both
//! engines, so each one this instance holds is its own SQLite database — the same shape
//! the RDS Data API's engine keeps (`service-com-amazonaws-rdsdata-rdsdataservice`), one
//! database per name. A name is created when the client's DDL creates it
//! ([`InstanceDatabase::create_database`]) and opened on first use for a name the instance
//! already has (the one `DBName` named, the engine's own default, a MySQL system schema),
//! up to [`MAX_DATABASES_PER_INSTANCE`].
//!
//! One connection per database, taken under a mutex for the duration of a single
//! statement:
//!
//! * every database is in memory and private to the instance, so one instance's rows never
//!   reach another's session and nothing is written to the host's disk;
//! * a TRANSACTION belongs to the SESSION that opened it rather than to the database's
//!   connection: each session's transaction keeps the database as it stood before its first
//!   statement and the rows it has changed since, so its `ROLLBACK` puts exactly those rows
//!   back ([`super::session_transactions`]), and the rows it has written or reserved are
//!   held against every other transaction until it ends ([`super::row_locks`]);
//! * every statement runs with the connection's own bounds in force (below) and under a
//!   DEFAULT-DENY authorizer ([`confine_to_the_instance`]), and the result set a session
//!   can draw out of one statement is capped in rows and in bytes.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use rusqlite::functions::FunctionFlags;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::limits::Limit;
use rusqlite::types::ValueRef;
use rusqlite::Connection;

use super::row_locks::LockTable;
use super::session_transactions::{self, ClosedTransaction, OpenTransactions};

/// How many rows one statement may hand back to a session.
pub const MAX_RESULT_ROWS: usize = 10_000;

/// How many rows one `COPY … TO STDOUT` may STREAM to a session.
///
/// A copy streams a whole relation rather than handing back a result set a client reads with
/// a cursor — "COPY TO copies the contents of a table" — so it is not held to the result-set
/// cap above; what bounds it is the bytes those rows render to
/// ([`MAX_RESULT_BYTES`]) and this ceiling, which is well above the rows one bulk load
/// carries and far below where the rendering would grow the process.
pub const MAX_STREAMED_ROWS: usize = 100_000;

/// How many bytes of result values one statement may hand back to a session, charged as
/// each value is read out of the engine, so a statement that would build a larger answer
/// is refused instead of allocating it.
pub const MAX_RESULT_BYTES: usize = 8 * 1024 * 1024;

/// The wall-clock budget one statement gets. Enforced by the engine's own progress
/// handler, which runs on the thread executing the statement — a timeout on the task
/// could not interrupt a CPU-bound statement, and this can.
pub const STATEMENT_DEADLINE: Duration = Duration::from_secs(5);

/// What PostgreSQL tells a client whose statement its `statement_timeout` cancelled. The
/// engine's own sentence for the cancellation, which is what the wire reports with
/// SQLSTATE `57014`.
pub const STATEMENT_TIMEOUT_CANCELLATION: &str =
    "canceling statement due to statement timeout";

/// What this endpoint tells a client whose statement ran past [`STATEMENT_DEADLINE`] with
/// no `statement_timeout` of its own in force.
pub const ENDPOINT_DEADLINE_CANCELLATION: &str =
    "canceling statement: it ran longer than this endpoint serves";

/// How often the engine asks the progress handler whether to carry on.
const PROGRESS_HANDLER_OPS: std::os::raw::c_int = 1_000;

/// The engine's own limits, which bound what one statement can ask of it. The values
/// mirror the ones this emulator's other SQL data plane uses
/// (`service-com-amazonaws-rdsdata-rdsdataservice`), so the two cannot drift on what a
/// statement may cost.
///
/// `MAX_VALUE_BYTES` is the largest single value the engine will HOLD or COMPUTE, and it is
/// the same ceiling as the result budget below rather than a smaller one: concatenating or
/// aggregating a table's text (`string_agg`, `||`, a digest over either) builds one value out
/// of many rows, which is ordinary work on a PostgreSQL server — "the longest possible
/// character string that can be stored is about 1 GB" — so what bounds such a statement is
/// what this endpoint can hand a session back ([`MAX_RESULT_BYTES`]) and not a tighter bound
/// on one value. Nothing a client sends can be that large anyway: a bound parameter is
/// bounded by the protocol's own message cap, and the statement text by `MAX_SQL_BYTES`.
pub const MAX_VALUE_BYTES: i32 = 8 * 1024 * 1024;
const MAX_SQL_BYTES: i32 = 65_536;
const MAX_EXPRESSION_DEPTH: i32 = 200;
const MAX_COMPOUND_SELECT: i32 = 20;
pub const MAX_BOUND_PARAMETERS: i32 = 1_000;

/// The page ceiling on each database the connection has, so the rows one instance holds
/// cannot grow the process without limit (16,384 pages of 4 KiB, 64 MiB per schema).
const MAX_DATABASE_PAGES: i64 = 16_384;

/// How many pages one step of an online backup copies; see
/// [`InstanceDatabase::load_from`]. The copy is driven a step at a time rather than in one
/// call so its deadline is checked between steps, which is the cancellation point a
/// bounded request needs.
const BACKUP_PAGES_PER_STEP: std::os::raw::c_int = 256;

/// The wall-clock budget ONE copy of a database gets: the same budget one statement gets,
/// because a copy is engine work of the same kind. Checked while waiting for each end of
/// the copy and between its steps, so a copy always gives up rather than holding either
/// database or spinning on a step that does not progress.
pub const COPY_DEADLINE: Duration = STATEMENT_DEADLINE;

/// How long a copy waits before asking again for a database another thread is using.
const COPY_LOCK_POLL: Duration = Duration::from_millis(10);

/// `PRAGMA temp_store = MEMORY`, so the engine's temporary database is in memory rather
/// than a file in the host's temp directory.
const TEMP_STORE_MEMORY: i32 = 2;

/// `PRAGMA cache_size`, in KiB (the pragma reads a negative value as a byte budget): what
/// ONE instance's connection may hold in its page cache. One emulator serves many DB
/// instances at once, each with its own connection, so the per-connection cache is set
/// well below the engine's default rather than left to multiply.
const MAX_PAGE_CACHE_KIB: i64 = -256;

/// The schemas the page ceiling is applied to: the instance's own tables and the
/// temporary database a sort or a materialised CTE spills into.
const CAPPED_SCHEMAS: [&str; 2] = ["main", "temp"];

/// How many databases one DB instance's engine holds at once.
///
/// Each one is a connection of its own with its own page ceiling, and naming a database is
/// what opens it, so a session that keeps naming fresh ones is bounded here rather than
/// growing the process: past the cap a statement that would open another is refused.
pub const MAX_DATABASES_PER_INSTANCE: usize = 64;

/// What went wrong with a statement, in terms both wires can render: each one maps a kind
/// to its own engine's code (a PostgreSQL SQLSTATE, a MySQL error number) and sends the
/// engine's real message with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineErrorKind {
    /// The statement does not parse, or names something the engine does not define.
    Syntax,
    /// A table, column or function the statement names does not exist.
    UndefinedObject,
    /// A database the statement names does not exist.
    UndefinedDatabase,
    /// A unique or primary-key constraint refused the row.
    UniqueViolation,
    /// A NOT NULL constraint refused the row.
    NotNullViolation,
    /// A CHECK constraint refused the row.
    CheckViolation,
    /// A foreign-key constraint refused the row.
    ForeignKeyViolation,
    /// A row the statement asked for is held by another transaction and the statement asked
    /// not to wait for it.
    LockNotAvailable,
    /// The statement WAITED for a row another transaction holds for as long as the session
    /// permits a wait for one, and the row was still held: see
    /// [`super::row_locks::lock_wait_limit`].
    LockWaitTimeout,
    /// Waiting for a row would close a cycle of transactions each waiting for the other, so
    /// the engine ended this one.
    Deadlock,
    /// A value is outside the range the type it is being stored as or cast to carries.
    NumericValueOutOfRange,
    /// A divisor is zero.
    DivisionByZero,
    /// This transaction's reads and another transaction's writes cannot be ordered one
    /// before the other, so the engine ended this one: the condition a SERIALIZABLE
    /// transaction is refused with.
    SerializationFailure,
    /// A value is not one its column accepts.
    DataError,
    /// The statement ran past [`STATEMENT_DEADLINE`].
    Cancelled,
    /// The statement asked for more than this emulator's endpoint serves: a result set
    /// past its caps, or more session state than one session may hold.
    LimitExceeded,
    /// The caller is not allowed to do this.
    InsufficientPrivilege,
    /// The statement would write and the session is on an endpoint that serves no writes:
    /// see [`super::read_only`].
    ReadOnlySqlTransaction,
    /// An object the statement would create is already there.
    DuplicateObject,
    /// The object the statement would remove is still in use, or something still depends
    /// on it.
    ObjectInUse,
    /// The engine does not serve this capability at all.
    FeatureNotSupported,
    /// The engine cannot serve this at all.
    Internal,
}

/// One failed statement: the kind of failure and the ENGINE's own message for it.
#[derive(Debug, Clone)]
pub struct EngineError {
    pub kind: EngineErrorKind,
    pub message: String,
    /// Where in the statement the engine stopped reading it, for the wire that reports a
    /// position in its own parse error.
    pub offset: Option<usize>,
    /// The SQLSTATE the PostgreSQL wire reports this refusal under, where the CONDITION
    /// names one of its own rather than the kind's ([`EngineError::with_sqlstate`]).
    ///
    /// Owned, because the code is not always one of this crate's own literals: a code
    /// block's `RAISE … USING ERRCODE = '<code>'` names the SQLSTATE the client is to branch
    /// on, and that value is the client's ([`super::pg_code_block`]). A boxed string rather
    /// than a `String`, because this type is the `Err` of every statement and is moved on
    /// every refusal.
    pub sqlstate: Option<Box<str>>,
    /// The error number and SQLSTATE the MySQL wire reports this refusal under, where the
    /// CONDITION names one of its own ([`EngineError::with_mysql_error`]).
    pub mysql_error: Option<(u16, &'static str)>,
    /// What this refusal is ABOUT beside its sentence: the constraint the row violated and
    /// where the server was when it failed, which PostgreSQL carries in FIELDS of its own
    /// rather than inside the message ([`RefusalFields`]).
    ///
    /// Behind a box, so a refusal that is about neither of them costs one pointer rather than
    /// two strings — this type is the `Err` of every statement and is moved on every refusal.
    pub about: Option<Box<RefusalFields>>,
    /// What the server SAID about the statement before it failed: the messages a code
    /// block's earlier `RAISE` statements produced ([`super::pg_code_block`]), or `None` for
    /// a failure with nothing said before it — which is every other one.
    ///
    /// They travel WITH the refusal because the statement they belong to is the one that
    /// failed: PostgreSQL sends those notices before the `ErrorResponse` and writes the ones
    /// its log carries, so dropping them with the answer would lose messages the server
    /// really produced. Behind a box, so a failure that said nothing costs one pointer
    /// rather than two vectors — this type is the `Err` of every statement.
    pub said: Option<Box<SaidBefore>>,
}

/// What a refusal is ABOUT beside its sentence, as the fields PostgreSQL carries each of them
/// in ([`EngineError::about`]). A client reads them out of those fields rather than out of the
/// message.
#[derive(Debug, Clone, Default)]
pub struct RefusalFields {
    /// `n`, the CONSTRAINT: the rule the row violated — which unique or foreign-key rule it
    /// broke ([`EngineError::with_constraint`]).
    pub constraint: Option<String>,
    /// `W`, the CONTEXT: where the server was when the statement failed, which a bulk load
    /// fills with the table, the line and the column it stopped on
    /// ([`EngineError::with_context`]).
    pub context: Option<String>,
}

/// What the server said about a statement BEFORE it failed, on each of its two channels
/// ([`EngineError::said`]).
#[derive(Debug, Clone, Default)]
pub struct SaidBefore {
    /// The messages as the client is sent them, in the order they were said.
    pub notices: Vec<super::wire_session::WireNotice>,
    /// The same messages as the lines the engine's LOG carries, for the severities it logs
    /// ([`super::log_export`]).
    pub logged: Vec<super::log_export::LogLine>,
}

impl EngineError {
    /// A failure of this emulator's own engine, with `message` as the engine worded it.
    pub fn new(kind: EngineErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            offset: None,
            sqlstate: None,
            mysql_error: None,
            about: None,
            said: None,
        }
    }

    /// The CONSTRAINT this refusal is about, where it is about one.
    #[must_use]
    pub fn constraint(&self) -> Option<&str> {
        self.about
            .as_ref()
            .and_then(|about| about.constraint.as_deref())
    }

    /// WHERE the server was when the statement failed, where that is a place of its own.
    #[must_use]
    pub fn context(&self) -> Option<&str> {
        self.about
            .as_ref()
            .and_then(|about| about.context.as_deref())
    }

    /// The messages the server said before this failure, in the order it said them.
    #[must_use]
    pub fn notices_said(&self) -> &[super::wire_session::WireNotice] {
        self.said
            .as_ref()
            .map_or(&[], |said| said.notices.as_slice())
    }

    /// The same messages as the lines the engine's log carries.
    #[must_use]
    pub fn lines_logged(&self) -> &[super::log_export::LogLine] {
        self.said.as_ref().map_or(&[], |said| said.logged.as_slice())
    }

    /// The same failure, carrying what the server said before it ([`Self::said`]).
    ///
    /// The messages given here come BEFORE any the failure already carries, because an outer
    /// block's statements ran before the inner one that failed.
    #[must_use]
    pub fn with_messages(
        mut self,
        notices: Vec<super::wire_session::WireNotice>,
        logged: Vec<super::log_export::LogLine>,
    ) -> Self {
        if notices.is_empty() && logged.is_empty() {
            return self;
        }
        let already = self.said.take().unwrap_or_default();
        self.said = Some(Box::new(SaidBefore {
            notices: [notices, already.notices].concat(),
            logged: [logged, already.logged].concat(),
        }));
        self
    }

    /// The same failure, naming the CONSTRAINT the row violated.
    #[must_use]
    pub fn with_constraint(mut self, constraint: impl Into<String>) -> Self {
        self.about.get_or_insert_with(Box::default).constraint = Some(constraint.into());
        self
    }

    /// The same failure, naming WHERE the server was when it happened ([`RefusalFields`]).
    #[must_use]
    pub fn with_context(mut self, context: impl Into<String>) -> Self {
        self.about.get_or_insert_with(Box::default).context = Some(context.into());
        self
    }

    /// The same failure, reported on the PostgreSQL wire under `sqlstate`.
    ///
    /// A statement class has its own SQLSTATE where the kind alone does not name it — a
    /// duplicate DATABASE (`42P04`) is not a duplicate object in general (`42710`) — and
    /// what a client branches on is the one the condition carries.
    #[must_use]
    pub fn with_sqlstate(mut self, sqlstate: impl Into<Box<str>>) -> Self {
        self.sqlstate = Some(sqlstate.into());
        self
    }

    /// The same failure, reported on the MySQL wire under `number` and `state`: the
    /// server error number the statement class defines for this condition.
    #[must_use]
    pub fn with_mysql_error(mut self, number: u16, state: &'static str) -> Self {
        self.mysql_error = Some((number, state));
        self
    }

    /// The same failure, carrying where in the statement the engine stopped.
    #[must_use]
    pub fn at_offset(mut self, offset: Option<usize>) -> Self {
        self.offset = offset;
        self
    }
}

/// One column of a result set: the name the engine reports for it and the type its
/// declaration gave, which is what decides the type code the wire reports.
#[derive(Debug, Clone)]
pub struct ColumnInfo {
    pub name: String,
    pub declared_type: Option<String>,
}

/// One value in a result set, as the engine holds it.
#[derive(Debug, Clone, PartialEq)]
pub enum Cell {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl Cell {
    /// The bytes this value occupies: what it costs against [`MAX_RESULT_BYTES`] when the
    /// engine hands it to a client, and what a stored copy of it holds — a NULL is the one
    /// byte its type code takes, a number the eight its value does, and text or a blob
    /// exactly its own length.
    pub fn byte_cost(&self) -> usize {
        match self {
            Self::Null => 1,
            Self::Integer(_) | Self::Real(_) => 8,
            Self::Text(text) => text.len(),
            Self::Blob(bytes) => bytes.len(),
        }
    }
}

/// What one statement produced: its columns and rows (empty for a statement that returns
/// none), how many rows it changed, and the key the engine assigned on an insert.
#[derive(Debug, Clone, Default)]
pub struct ResultSet {
    pub columns: Vec<ColumnInfo>,
    pub rows: Vec<Vec<Cell>>,
    pub affected: u64,
    pub last_insert_id: i64,
}

impl ResultSet {
    /// A result set with no columns at all — what a statement that returns no rows
    /// produced, carrying only what it changed.
    pub fn changed(affected: u64, last_insert_id: i64) -> Self {
        Self {
            columns: Vec::new(),
            rows: Vec::new(),
            affected,
            last_insert_id,
        }
    }

    /// A result set built here rather than by the engine: the columns and rows a SERVER
    /// command answers with (`SHOW`), which are the server's own state and not a query
    /// over the instance's tables.
    pub fn of_rows(columns: Vec<ColumnInfo>, rows: Vec<Vec<Cell>>) -> Self {
        Self {
            columns,
            rows,
            affected: 0,
            last_insert_id: 0,
        }
    }
}

/// What the server functions registered on the engine's connections answer from: the
/// instance's own runtime and the session running the statement.
///
/// Set under the engine's lock for the duration of ONE statement
/// ([`InstanceDatabase::execute_as`]), which is the only time a function can run, so a
/// function always reads the session that called it.
#[derive(Debug, Clone, Default)]
pub struct ServerView {
    /// The configuration parameters in force for this session, by lowercased name: what
    /// `current_setting` resolves against.
    pub settings: BTreeMap<String, String>,
    /// When the instance's engine last came up, as epoch seconds:
    /// `pg_postmaster_start_time()`.
    pub postmaster_start_time: f64,
    /// Whether the instance is replaying from a primary: `pg_is_in_recovery()`.
    pub in_recovery: bool,
    /// The session's own backend pid: `pg_backend_pid()`.
    pub backend_pid: u32,
    /// The address of the server end this session is served by, which is what
    /// `inet_server_addr()` reports; empty where the session's server published no address
    /// of its own, which the function then answers NULL for, as PostgreSQL does for a
    /// connection over a Unix-domain socket.
    pub server_address: String,
    /// The port of that same server end: `inet_server_port()`.
    pub server_port: u16,
    /// `statement_timeout` for this statement, in milliseconds; 0 disables it, as the
    /// parameter documents.
    pub statement_timeout_ms: u64,
    /// Which WIRE the session running this statement speaks, and so which dialect's
    /// functions the statement may call ([`super::engine_time`]). `None` for one of the
    /// server's own statements, which runs under no session.
    pub wire: Option<super::EngineWire>,
    /// Whether the transaction the session running this statement has open reads a snapshot
    /// per STATEMENT rather than one per transaction — the READ COMMITTED levels, where "a
    /// statement can only see rows committed before it began"
    /// ([`super::wire_session::SessionFacts::snapshot_per_statement`]).
    ///
    /// `false` for one of the server's own statements, which runs inside whatever snapshot
    /// the session's own statement is reading: a catalog the server rebuilds for a statement
    /// is part of that statement, not a statement of its own.
    pub snapshot_per_statement: bool,
    /// When the statement running now must be cancelled, which is what `pg_sleep` waits
    /// against.
    pub deadline: Option<Instant>,
    /// The value `LAST_INSERT_ID()` answers with for the session running this statement,
    /// shared with that session so the one-argument form's write reaches it
    /// ([`super::mysql_insert_id`]). `None` for one of the server's own statements, which
    /// runs under no session.
    pub insert_id: Option<super::mysql_insert_id::SessionInsertId>,
}

/// How many rows ONE statement's row ids are read for, which is how many rows it may take
/// row locks on.
///
/// A statement over more rows than this is run with no row locks rather than with a lock
/// table the size of the table it reads: the same ceiling the result of one statement is
/// held to ([`MAX_RESULT_ROWS`]). It is a bound on this endpoint's own bookkeeping and never
/// a refusal — a statement past it is answered by the engine exactly as one under it is.
pub const MAX_LOCKED_ROWS: usize = MAX_RESULT_ROWS;

/// The wall-clock budget ONE commit gets: the same budget one statement gets, because
/// carrying a transaction's rows over into the shared database is engine work of the same
/// kind. Checked between the rows and enforced inside each statement by the engine's own
/// progress handler, so a commit always gives up rather than holding the engine.
pub const COMMIT_DEADLINE: Duration = STATEMENT_DEADLINE;

/// The size of one page of the engine's databases, which is what the budget below is
/// counted in: SQLite's own default page size.
const PAGE_BYTES: u32 = 4096;

/// How many PAGES the states this endpoint's open transactions hold may add up to.
///
/// A transaction keeps the database as it stood before its first statement ([`copy_database`])
/// so its `ROLLBACK` can put its rows back, and the sessions of one endpoint can hold many at
/// once: this is what bounds the memory they add up to, rather than leaving it to the product
/// of the per-database page ceiling, the databases per instance and the sessions a server
/// admits. 256 MiB of pages, checked before each copy.
const MAX_UNCOMMITTED_COPY_PAGES: i64 = 65_536;

/// One DB instance's engine: its databases, each with its own connection, the transactions
/// its sessions have open and the rows those hold.
pub struct InstanceDatabase {
    databases: Mutex<Databases>,
    /// Shared with the server functions registered on every one of those connections; see
    /// [`ServerView`].
    view: Arc<Mutex<ServerView>>,
    /// The transaction each session has open, and the rows it has not committed, shared
    /// with the recording hook on every one of those connections; see
    /// [`super::session_transactions`].
    transactions: Arc<OpenTransactions>,
    /// The rows those transactions hold; see [`super::row_locks`].
    ///
    /// The ONE table of the storage this engine serves: for a DB CLUSTER's volume it is the
    /// cluster's own, shared with every other surface of that cluster, so a row a transaction
    /// on one door holds is a row a transaction on another waits for.
    locks: Arc<LockTable>,
    /// The VOLUME this engine's databases are held in, for an engine that serves a DB
    /// CLUSTER's storage ([`InstanceDatabase::open_on_volume`]).
    ///
    /// A DB cluster is ONE database behind SEVERAL doors: its engine endpoints, and — for an
    /// Aurora cluster whose HTTP endpoint is on — the RDS Data API, whose `ExecuteStatement`
    /// "runs a SQL statement against a database" of that same cluster. So each database of
    /// such a cluster is held in one backing that every surface of the cluster opens a
    /// connection onto ([`::service_common::cluster_sql_volumes`]), and a table a client
    /// created through one door is a table a session on another selects.
    ///
    /// `None` for an engine whose databases are its own: a DB INSTANCE's engine, and every
    /// COPY of a database (a snapshot's, a transaction's working set), which nothing else
    /// addresses.
    volume: Option<::service_common::cluster_sql_volumes::ClusterVolume>,
}

/// The databases one instance holds.
#[derive(Default)]
struct Databases {
    /// One connection per database name, keyed by the name as it was created: the SHARED
    /// database, which every session that is not inside a transaction reads and writes.
    open: BTreeMap<String, Connection>,
    /// One connection per database a SESSION'S OPEN TRANSACTION has its own copy of, keyed
    /// by that session and the database's name: the snapshot its statements run against
    /// until it commits or rolls back ([`super::session_transactions`]).
    working: BTreeMap<(u32, String), Connection>,
    /// Whether the databases the INSTANCE comes with ([`InstanceDatabase::seed`]) have
    /// been created yet. They are created once, on the first login, after which this
    /// engine is the only thing that says which databases the instance has — so a
    /// database a client created is there for every later session, and one a client
    /// dropped stays gone.
    seeded: bool,
}

/// Written by hand rather than derived: a connection is not `Debug`, and what a reader
/// needs here is which databases the instance holds.
impl std::fmt::Debug for InstanceDatabase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InstanceDatabase")
            .field("databases", &self.database_names())
            .finish()
    }
}

impl InstanceDatabase {
    /// Bring the engine up for a DB instance that has no database open yet.
    ///
    /// The engine itself is started once here — on a host where it cannot start at all,
    /// the caller answers the create with a failure rather than publishing an endpoint
    /// nothing serves — and each database is opened empty when it is first named.
    pub fn open() -> Result<Self, EngineError> {
        Self::open_on_volume(None)
    }

    /// The same, for an engine that serves the storage of a DB CLUSTER: its databases are
    /// held in that cluster's own volume ([`InstanceDatabase::volume`]), so every surface of
    /// the cluster — this endpoint, the cluster's other endpoints and its Data API HTTP
    /// endpoint — reads and writes one database per name.
    ///
    /// `None` is [`Self::open`]: an engine whose databases are its own.
    pub fn open_on_volume(
        volume: Option<::service_common::cluster_sql_volumes::ClusterVolume>,
    ) -> Result<Self, EngineError> {
        let view = Arc::new(Mutex::new(ServerView::default()));
        probe_the_engine_starts(&view)?;
        Ok(Self {
            databases: Mutex::new(Databases::default()),
            view,
            transactions: Arc::new(OpenTransactions::default()),
            // The lock table of the storage: a cluster volume's own, which every surface of
            // that cluster holds rows in, and a table of this engine's own where the storage
            // is.
            locks: volume.as_ref().map_or_else(
                || Arc::new(LockTable::default()),
                |volume| Arc::clone(volume.locks()),
            ),
            volume,
        })
    }

    /// The backing the database `name` of this engine is held in, and `None` for a database
    /// of an engine whose storage is its own; see [`InstanceDatabase::volume`].
    fn backing(&self, name: &str) -> Option<String> {
        self.volume.as_ref().map(|volume| volume.backing(name))
    }

    /// The rows the transactions on this engine hold, which is what decides whether one
    /// session's statement can have the rows another session's transaction is holding.
    #[must_use]
    pub fn locks(&self) -> &LockTable {
        &self.locks
    }

    /// The instance's databases, recovering a lock a panicking session poisoned: a
    /// long-lived emulator has to keep serving every later session.
    fn lock(&self) -> std::sync::MutexGuard<'_, Databases> {
        self.databases.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The instance's databases, waited for only until `deadline`: `None` where another
    /// thread held them for that long.
    ///
    /// What [`Self::load_from`] takes each end of a copy with, so a copy can neither park
    /// a request on an engine that is busy nor hold one it could not finish with. A lock
    /// a panicking session poisoned is recovered exactly as [`Self::lock`] recovers it.
    fn lock_until(&self, deadline: Instant) -> Option<std::sync::MutexGuard<'_, Databases>> {
        loop {
            match self.databases.try_lock() {
                Ok(guard) => return Some(guard),
                Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                    return Some(poisoned.into_inner())
                }
                Err(std::sync::TryLockError::WouldBlock) => {
                    if Instant::now() >= deadline {
                        return None;
                    }
                    std::thread::sleep(COPY_LOCK_POLL);
                }
            }
        }
    }

    /// Create the databases the INSTANCE itself comes with — the one `DBName` named, the
    /// engine's own default, a MySQL server's system schemas — once.
    ///
    /// Called as a session is admitted, because that is the first moment this emulator
    /// knows what the instance's record says they are. After it, the engine is the only
    /// thing that says which databases the instance has: a later call adds nothing, so a
    /// database a client dropped is not quietly created again.
    pub fn seed(&self, names: &[String]) {
        let mut databases = self.lock();
        if databases.seeded {
            return;
        }
        databases.seeded = true;
        for name in names {
            if name.is_empty() || resolve(&databases.open, name).is_some() {
                continue;
            }
            if admit_another_database(&databases).is_err() {
                return;
            }
            match open_recorded_database(
                &self.view,
                &self.transactions,
                name,
                self.backing(name).as_deref(),
            ) {
                Ok(conn) => {
                    databases.open.insert(name.clone(), conn);
                }
                Err(_) => return,
            }
        }
    }

    /// Run `sql` with `params` bound to its placeholders in order against the database
    /// `database`, and answer with what the engine produced.
    ///
    /// Blocking, CPU-bound work: a wire handler calls it on a blocking task so a long
    /// statement cannot starve the async runtime, and the statement itself is bounded by
    /// [`STATEMENT_DEADLINE`] and by the result caps.
    ///
    /// This is the form the server's OWN statements use (the catalog rebuilds, the
    /// statements a server command runs for a session): it runs with no session view, so a
    /// server function in it would see none. A client's own statement goes through
    /// [`Self::execute_as`].
    pub fn execute(
        &self,
        database: &str,
        sql: &str,
        params: &[Option<String>],
    ) -> Result<ResultSet, EngineError> {
        self.execute_as(database, &ServerView::default(), sql, params)
    }

    /// Run `sql` against `database` as the session `view` describes: the server functions
    /// it calls answer from that session, and the statement is cancelled at the earlier of
    /// this endpoint's own deadline and the session's `statement_timeout`.
    ///
    /// The cancellation is reported the way the engine that cancelled it words it — a
    /// `statement_timeout` cancellation is PostgreSQL's own sentence — because that is the
    /// text a client branches on.
    pub fn execute_as(
        &self,
        database: &str,
        view: &ServerView,
        sql: &str,
        params: &[Option<String>],
    ) -> Result<ResultSet, EngineError> {
        self.run_for(None, database, view, sql, params, MAX_RESULT_ROWS)
    }

    /// Run `sql` for the SESSION `session`, exactly as [`Self::execute_as`] runs it, and
    /// record the rows it changes against that session's transaction where it has one open —
    /// so its own `ROLLBACK` drops the rows IT wrote and its `COMMIT` carries exactly those
    /// over.
    pub fn execute_for(
        &self,
        session: u32,
        database: &str,
        view: &ServerView,
        sql: &str,
        params: &[Option<String>],
    ) -> Result<ResultSet, EngineError> {
        self.run_for(Some(session), database, view, sql, params, MAX_RESULT_ROWS)
    }

    /// Run the query one `COPY … TO STDOUT` streams, exactly as [`Self::execute_for`] runs a
    /// statement — bounded by the rows a copy streams ([`MAX_STREAMED_ROWS`]) rather than by
    /// the result-set cap, because a copy streams a relation rather than handing back a
    /// result set.
    pub fn stream_for(
        &self,
        session: u32,
        database: &str,
        view: &ServerView,
        sql: &str,
        params: &[Option<String>],
    ) -> Result<ResultSet, EngineError> {
        self.run_for(
            Some(session),
            database,
            view,
            sql,
            params,
            MAX_STREAMED_ROWS,
        )
    }

    /// Run a statement of the SERVER's own for the session `session`: the catalog rebuilds
    /// and the catalog rows a server command writes.
    ///
    /// It runs with no session view, as [`Self::execute`] does, and against the same
    /// database that session's own statements run against — its transaction's copy where it
    /// has one open — so a transaction reads the catalogs in the database its statements
    /// read, and the rows a server command wrote inside it are carried over by its commit
    /// like any other.
    pub fn execute_in(
        &self,
        session: u32,
        database: &str,
        sql: &str,
        params: &[Option<String>],
    ) -> Result<ResultSet, EngineError> {
        self.run_for(
            Some(session),
            database,
            &ServerView::default(),
            sql,
            params,
            MAX_RESULT_ROWS,
        )
    }

    /// One statement, with its row changes recorded against `session`'s transaction when it
    /// has one, and holding the rows it answers with to `row_ceiling`.
    ///
    /// The recording is turned on and off INSIDE the engine's own lock, which serialises the
    /// statements of every session on this endpoint: the rows one statement changes can
    /// therefore only be recorded against the session that ran it.
    fn run_for(
        &self,
        session: Option<u32>,
        database: &str,
        view: &ServerView,
        sql: &str,
        params: &[Option<String>],
        row_ceiling: usize,
    ) -> Result<ResultSet, EngineError> {
        let mut databases = self.lock();
        self.take_the_sessions_snapshot(&mut databases, session, database)?;
        // ... and, for a transaction that reads a snapshot per STATEMENT, a snapshot taken
        // now: what READ COMMITTED means is that this statement reads what is committed when
        // it runs ([`Self::take_this_statements_snapshot`]).
        if view.snapshot_per_statement {
            self.take_this_statements_snapshot(&mut databases, session, database)?;
        }
        self.transactions.recording_for(session);
        let (deadline, timeout_first) = statement_deadline(view);
        {
            let mut current = self.view.lock().unwrap_or_else(PoisonError::into_inner);
            *current = view.clone();
            current.deadline = Some(deadline);
        }
        let conn = connection_for(
            &mut databases,
            &self.view,
            &self.transactions,
            self.volume.as_ref(),
            session,
            database,
        )?;
        conn.progress_handler(PROGRESS_HANDLER_OPS, Some(move || Instant::now() >= deadline))
            .map_err(from_engine)?;
        let answer = run_statement(conn, sql, params, row_ceiling);
        // The bound is removed once the statement is over. The next statement installs its
        // own, so a failure to remove this one cannot leave anything running unbounded.
        let _ = conn.progress_handler(PROGRESS_HANDLER_OPS, None::<fn() -> bool>);
        self.transactions.recording_for(None);
        match answer {
            // The statement did not finish inside its budget: whatever the engine said
            // about the interruption, what happened is the cancellation, worded by
            // whichever deadline came first.
            Err(_) if Instant::now() >= deadline => Err(EngineError::new(
                EngineErrorKind::Cancelled,
                if timeout_first {
                    STATEMENT_TIMEOUT_CANCELLATION
                } else {
                    ENDPOINT_DEADLINE_CANCELLATION
                },
            )),
            other => other,
        }
    }

    /// Give `session`'s transaction its OWN copy of the database `database`, where it has a
    /// transaction open and no copy of that database yet.
    ///
    /// That copy is the transaction's snapshot AND the database its statements run against:
    /// it is the engine's own page-for-page backup ([`copy_database`]) taken ONCE per
    /// database per transaction, at the transaction's first statement in it, whether that
    /// statement reads or writes — a transaction that reads has to read the same rows
    /// throughout, and one that writes has to write where no other session can see it until
    /// it commits.
    ///
    /// The copies the OPEN transactions of this endpoint hold are charged against
    /// [`MAX_UNCOMMITTED_COPY_PAGES`]: the budget is checked before each copy, so what the
    /// uncommitted state of every session together can hold is that budget plus the page
    /// ceiling of one database, and a statement that would exceed it is refused instead of
    /// growing the process.
    fn take_the_sessions_snapshot(
        &self,
        databases: &mut Databases,
        session: Option<u32>,
        database: &str,
    ) -> Result<(), EngineError> {
        let Some(session) = session else {
            return Ok(());
        };
        let key = resolve(&databases.open, database).unwrap_or_else(|| database.to_string());
        if !self.transactions.is_open(session) || self.transactions.holds_a_copy_of(session, &key)
        {
            return Ok(());
        }
        self.admit_another_copy()?;
        let deadline = Instant::now() + COPY_DEADLINE;
        // The copy records the rows it changes against the transactions of this endpoint,
        // exactly as the shared database does: the rows the transaction writes in its own
        // copy are the rows its commit carries over. PRIVATE: a session's working set is
        // nobody else's to read, so it is never opened on the cluster's volume.
        let mut copy = open_confined_database(&self.view, None)?;
        session_transactions::record_changes_of(&mut copy, &key, &self.transactions)
            .map_err(unavailable)?;
        let conn = connection(
            databases,
            &self.view,
            &self.transactions,
            self.volume.as_ref(),
            &key,
        )?;
        let read_at = changes_mark(conn);
        let pages = copy_database(conn, &mut copy, deadline)?;
        // The SCHEMA the copy declares right now, which is what its commit carries its own
        // changes over against ([`session_transactions::apply_schema`]).
        let schema = session_transactions::schema_objects(&copy)?;
        databases.working.insert((session, key.clone()), copy);
        self.transactions
            .note_copy(session, &key, pages, schema, read_at);
        Ok(())
    }

    /// Whether this endpoint has room for one more copy of a database, which is the budget
    /// every snapshot — a first one and one taken again — is checked against.
    fn admit_another_copy(&self) -> Result<(), EngineError> {
        if self.transactions.charged_pages() >= MAX_UNCOMMITTED_COPY_PAGES {
            return Err(EngineError::new(
                EngineErrorKind::LimitExceeded,
                format!(
                    "this endpoint holds at most {} MiB of uncommitted state across its open \
                     transactions",
                    MAX_UNCOMMITTED_COPY_PAGES * i64::from(PAGE_BYTES) / (1024 * 1024)
                ),
            ));
        }
        Ok(())
    }

    /// Give `session`'s transaction a snapshot of the database `database` taken NOW, where it
    /// reads one per statement and the shared database has changed since the copy it holds
    /// was taken.
    ///
    /// This is what READ COMMITTED means: "a statement can only see rows committed before it
    /// began", so each statement of such a transaction reads what is committed when IT runs —
    /// alongside the transaction's own uncommitted rows, which nobody else can see yet. The
    /// copy taken again is therefore the shared database as it stands plus this transaction's
    /// own work put back onto it ([`Self::carry_the_work_over`], the same carry a commit
    /// makes), and the rows the transaction had created come back under whatever row ids the
    /// fresh copy gives them ([`session_transactions::RemappedRow`]).
    ///
    /// Nothing is taken again for a transaction at REPEATABLE READ or SERIALIZABLE, which is
    /// what reading one snapshot throughout means, nor while the shared database stands where
    /// it stood when the copy was taken — so a transaction whose neighbours commit nothing
    /// pays for no copy at all.
    ///
    /// The transaction keeps the copy it has on every failure here: a snapshot that could not
    /// be taken leaves the statement refused, never the transaction's own work dropped.
    fn take_this_statements_snapshot(
        &self,
        databases: &mut Databases,
        session: Option<u32>,
        database: &str,
    ) -> Result<(), EngineError> {
        let Some(session) = session else {
            return Ok(());
        };
        let key = resolve(&databases.open, database).unwrap_or_else(|| database.to_string());
        let Some(read_at) = databases.open.get(&key).map(changes_mark) else {
            return Ok(());
        };
        if !self.transactions.copy_is_stale(session, &key, read_at) {
            return Ok(());
        }
        let Some(own) = self.transactions.own_work_in(session, &key) else {
            return Ok(());
        };
        self.admit_another_copy()?;
        let deadline = Instant::now() + COPY_DEADLINE;
        let mut fresh = open_confined_database(&self.view, None)?;
        session_transactions::record_changes_of(&mut fresh, &key, &self.transactions)
            .map_err(unavailable)?;
        let pages = match databases.open.get(&key) {
            Some(live) => copy_database(live, &mut fresh, deadline)?,
            None => return Ok(()),
        };
        // The rows the carry below writes belong to the statements that wrote them in the
        // first place and are already recorded against this transaction, so nothing of it is
        // recorded again — as a commit's own carry records nothing.
        self.transactions.recording_for(None);
        let Some(held) = databases.working.get(&(session, key.clone())) else {
            return Ok(());
        };
        let remapped = Self::carry_the_work_over(
            &mut fresh,
            held,
            &own.schema,
            &own.changed,
            &own.created,
            deadline,
        )?;
        let schema = session_transactions::schema_objects(&fresh)?;
        databases.working.insert((session, key.clone()), fresh);
        self.transactions
            .note_recopy(session, &key, pages, schema, read_at);
        self.transactions.remap_rows(session, &key, &remapped);
        Ok(())
    }

    /// Carry the work ONE transaction did to ONE database — the schema it declared and the
    /// rows it changed — out of `source`, its own copy of that database, and into
    /// `destination`.
    ///
    /// The destination is the SHARED database when a transaction commits
    /// ([`Self::carry_the_rows_over`]) and a copy taken again when a statement of it reads its
    /// own snapshot ([`Self::take_this_statements_snapshot`]): one carry either way, so the
    /// two cannot disagree about what a transaction has done.
    ///
    /// The SCHEMA the transaction declared comes first: a row of a table it created has
    /// nowhere to go until that table is there, and a table whose declaration it CHANGED is
    /// carried over whole, because its rows no longer have the columns the destination held
    /// them under. What is carried over is the transaction's OWN delta, against the schema
    /// `before` — the schema its copy declared when the copy was taken.
    ///
    /// The authorizer is lifted for the duration: carrying a row over is the SERVER's own work
    /// on the session's behalf, and it needs the one thing a session may not ask for — the
    /// engine's `defer_foreign_keys`, so the constraints are checked once the whole
    /// transaction is in rather than between its rows. No statement of the client's runs under
    /// it: what runs is built from the rows the engine itself reported changed and the values
    /// the transaction's own copy holds for them. The lift is a GUARD, so the connection is
    /// confined again on every path out, an unwind included.
    ///
    /// Bounded by `deadline` twice over: the engine's own progress handler is installed on the
    /// destination for the duration, so no single statement of the carry can run past it, and
    /// the deadline is checked between rows.
    fn carry_the_work_over(
        destination: &mut Connection,
        source: &Connection,
        before: &session_transactions::SchemaObjects,
        changed: &std::collections::BTreeSet<session_transactions::ChangedRow>,
        created: &std::collections::BTreeSet<session_transactions::ChangedRow>,
        deadline: Instant,
    ) -> Result<Vec<session_transactions::RemappedRow>, EngineError> {
        let mut unconfined = Unconfined::of(destination);
        let destination = unconfined.connection();
        destination
            .progress_handler(
                PROGRESS_HANDLER_OPS,
                Some(move || Instant::now() >= deadline),
            )
            .map_err(from_engine)?;
        let carried = session_transactions::apply_schema(destination, source, before, deadline)
            .and_then(|redeclared| {
                for table in &redeclared {
                    session_transactions::copy_table(destination, source, table, deadline)?;
                }
                let rest: std::collections::BTreeSet<_> = changed
                    .iter()
                    .filter(|(table, _)| !redeclared.contains(table))
                    .cloned()
                    .collect();
                session_transactions::apply_rows(destination, source, &rest, created, deadline)
            });
        // The bound is removed once the carry is over. The next statement on this connection
        // installs its own, so a failure to remove this one cannot leave anything running
        // unbounded.
        let _ = destination.progress_handler(PROGRESS_HANDLER_OPS, None::<fn() -> bool>);
        carried
    }

    /// Record the rows `rows` as changed by `session`'s transaction, where it has one open.
    ///
    /// What the statement itself knows and the engine's hook does not report: see
    /// [`session_transactions::OpenTransactions::note_rows_for`].
    pub fn note_changed_rows(&self, session: u32, database: &str, rows: &[(String, String, i64)]) {
        if rows.is_empty() || !self.transactions.is_open(session) {
            return;
        }
        self.transactions.note_rows_for(
            session,
            database,
            rows.iter()
                .map(|(_, table, rowid)| (table.clone(), *rowid)),
        );
    }

    /// Whether the SESSION `session` is inside a transaction it opened, which is what a
    /// PostgreSQL `ReadyForQuery` and a MySQL status flag report.
    #[must_use]
    pub fn in_transaction(&self, session: u32) -> bool {
        self.transactions.is_open(session)
    }

    /// Open a transaction for `session`, answering whether it had none open already.
    pub fn begin_transaction(&self, session: u32) -> bool {
        self.transactions.open(session)
    }

    /// Commit `session`'s transaction: the rows it wrote become the shared database's rows
    /// for every session that reads afterwards, and the rows it held are let go. Answers
    /// whether it had one open.
    ///
    /// `serializable` is whether the transaction ran at the SERIALIZABLE isolation level,
    /// which is the level that cannot leave a read/write dependency unordered: such a
    /// transaction is REFUSED where a transaction that committed while it was open wrote a
    /// relation it read ([`session_transactions::OpenTransactions::conflicting_commit`]) —
    /// PostgreSQL's own `could not serialize access` — and nothing it wrote reaches the
    /// shared database. At every lower level the same pair commits, each transaction's own
    /// write standing, which is what those levels permit.
    ///
    /// Blocking, CPU-bound work bounded by [`COMMIT_DEADLINE`]: a caller runs it on a
    /// blocking task, as a wire handler runs a statement.
    pub fn commit_transaction(
        &self,
        session: u32,
        serializable: bool,
    ) -> Result<bool, EngineError> {
        let closed = self.transactions.close(session);
        let Some(closed) = closed else {
            self.locks.release(session);
            return Ok(false);
        };
        // The session's own copies come OUT of the store first, so every path out of this
        // call — the refusal below, a failure while carrying the rows over, an unwind —
        // drops them with it: the transaction is closed, so nothing else would.
        let copies = self.take_the_sessions_copies(session);
        let conflict = serializable
            .then(|| {
                self.transactions
                    .conflicting_commit(closed.began_after, &closed.read)
            })
            .flatten();
        if let Some((_, relation)) = conflict {
            // The transaction ends either way: its copies go, nothing it wrote is carried
            // over, and the client is told why in the words its engine uses. Which relation
            // the dependency is over is recorded rather than reported, because the sentence
            // a client branches on is the one the engine words.
            ::tracing::info!(
                audit = true,
                operation = "DbInstanceEngineStatement",
                outcome = "serialization-failure",
                connection_id = session,
                object = %relation,
                "a serializable transaction's reads could not be ordered after a commit"
            );
            drop(copies);
            self.locks.release(session);
            return Err(EngineError::new(
                EngineErrorKind::SerializationFailure,
                "could not serialize access due to read/write dependencies among transactions",
            ));
        }
        let carried = self.carry_the_rows_over(&closed, copies);
        if carried.is_ok() {
            self.transactions.note_commit(closed.wrote());
        }
        self.locks.release(session);
        carried.map(|()| true)
    }

    /// Take every copy `session` holds out of the store, as the databases they are copies
    /// of: dropping what this answers is what closes them.
    ///
    /// Keyed by the session alone rather than by the databases a transaction recorded, so a
    /// copy cannot outlive the transaction it belongs to however that transaction ended.
    fn take_the_sessions_copies(&self, session: u32) -> Vec<(String, Connection)> {
        let mut databases = self.lock();
        let held: Vec<(u32, String)> = databases
            .working
            .keys()
            .filter(|(whose, _)| *whose == session)
            .cloned()
            .collect();
        held.into_iter()
            .filter_map(|key| {
                let name = key.1.clone();
                databases.working.remove(&key).map(|copy| (name, copy))
            })
            .collect()
    }

    /// Record that `session`'s transaction has READ the relations `relations` of the
    /// database `database`, where it has one open: what its commit at the SERIALIZABLE
    /// level is checked against ([`Self::commit_transaction`]).
    pub fn note_read_relations(
        &self,
        session: u32,
        database: &str,
        relations: impl IntoIterator<Item = String>,
    ) {
        self.transactions.note_read(session, database, relations);
    }

    /// Roll back `session`'s transaction: nothing it wrote reaches the shared database, and
    /// the rows it held are let go. Answers whether it had one open.
    ///
    /// Its own copy of each database it touched is DROPPED, which is what leaves the shared
    /// database exactly as the transaction found it: the rows it wrote were only ever in that
    /// copy ([`super::session_transactions`]).
    pub fn rollback_transaction(&self, session: u32) -> Result<bool, EngineError> {
        let closed = self.transactions.close(session);
        // Dropping what this takes out of the store is what closes the copies, whether the
        // session had a transaction open or not.
        drop(self.take_the_sessions_copies(session));
        self.locks.release(session);
        Ok(closed.is_some())
    }

    /// Carry one committing transaction's rows over into the shared databases, database by
    /// database, and drop its copies.
    ///
    /// Bounded by [`COMMIT_DEADLINE`] twice over: the engine's own progress handler is
    /// installed for the duration, so no single statement of it can run past it, and the
    /// Bounded by [`COMMIT_DEADLINE`] twice over, as every carry is
    /// ([`Self::carry_the_work_over`], which this hands each database's work to). A commit the
    /// deadline cuts short is reported as the cancellation it is rather than left to run.
    ///
    /// `copies` is the transaction's own copy of each database it touched, taken out of the
    /// store by the caller ([`Self::take_the_sessions_copies`]) so that no path out of a
    /// commit can leave one behind.
    fn carry_the_rows_over(
        &self,
        closed: &ClosedTransaction,
        copies: Vec<(String, Connection)>,
    ) -> Result<(), EngineError> {
        if closed.overflowed {
            return Err(EngineError::new(
                EngineErrorKind::Internal,
                format!(
                    "this endpoint holds at most {} uncommitted rows per transaction, so this \
                     transaction could not be committed",
                    session_transactions::MAX_UNCOMMITTED_ROWS
                ),
            ));
        }
        let deadline = Instant::now() + COMMIT_DEADLINE;
        let mut databases = self.lock();
        self.transactions.recording_for(None);
        let mut carried = Ok(());
        for (name, copy) in &copies {
            if carried.is_err() {
                continue;
            }
            let empty = std::collections::BTreeSet::new();
            let rows = closed.changed.get(name).unwrap_or(&empty);
            // Which of them the transaction CREATED, which its commit adds to the shared
            // database rather than installs over
            // ([`session_transactions::apply_rows`]).
            let created = closed.created.get(name).unwrap_or(&empty);
            let before = closed.copies.get(name).cloned().unwrap_or_default();
            let live = connection(
                &mut databases,
                &self.view,
                &self.transactions,
                self.volume.as_ref(),
                name,
            )?;
            // Where a row the transaction created came back at another row id, the
            // transaction is over: nothing names those rows any more.
            carried =
                Self::carry_the_work_over(live, copy, &before, rows, created, deadline).map(drop);
        }
        carried
    }

    /// The engine's own row ids of the rows `rows_sql` selects, for the statement `sql` the
    /// caller is about to run with `params` bound to it.
    ///
    /// The row query is the CALLER's predicate, so it is bounded exactly as the caller's own
    /// statement is: this session's view, carrying the same deadline [`Self::execute_as`]
    /// gives a statement (this endpoint's budget, clamped by the session's
    /// `statement_timeout`), is published on the engine BEFORE the scan runs, and the
    /// engine's progress handler is installed with that deadline. The publication is what
    /// bounds a server function the progress handler cannot reach into — one that waits —
    /// because such a function reads its budget from the published view; the handler bounds
    /// the scan itself. So the scan cannot hold this endpoint's engine for longer than one
    /// statement may.
    ///
    /// `sql` is the statement the ENGINE is to run, which is what the row query's
    /// placeholders are matched against: a locking clause has no engine grammar, so a caller
    /// hands over the statement with its clause already taken off
    /// ([`super::lock_plan::LockPlan::engine_sql`]).
    ///
    /// `None` where the rows cannot be identified: a row query the engine will not run or
    /// could not finish inside that budget, a statement whose bound parameters cannot be
    /// matched to the row query by name, or more rows than [`MAX_LOCKED_ROWS`]. The caller
    /// then takes NO row locks rather than the wrong ones, and reports nothing of its own
    /// for the statement: whatever the engine answers for it is what the client reads.
    pub fn row_ids(
        &self,
        session: u32,
        database: &str,
        view: &ServerView,
        rows_sql: &str,
        sql: &str,
        params: &[Option<String>],
    ) -> Option<Vec<i64>> {
        let (deadline, _) = statement_deadline(view);
        let mut databases = self.lock();
        // The rows the statement will lock are the rows the statement will SEE, so the
        // snapshot it reads them off is the one its own statement reads: a transaction that
        // takes one per statement has it taken here, before the predicate runs.
        if view.snapshot_per_statement {
            self.take_this_statements_snapshot(&mut databases, Some(session), database)
                .ok()?;
        }
        {
            let mut current = self.view.lock().unwrap_or_else(PoisonError::into_inner);
            *current = view.clone();
            current.deadline = Some(deadline);
        }
        // The rows the statement asks for are the rows IT will see: a session inside a
        // transaction reads its own copy of the database, so the predicate runs there too.
        let conn: &mut Connection = connection_for(
            &mut databases,
            &self.view,
            &self.transactions,
            self.volume.as_ref(),
            Some(session),
            database,
        )
        .ok()?;
        conn.progress_handler(PROGRESS_HANDLER_OPS, Some(move || Instant::now() >= deadline))
            .ok()?;
        let ids = identified_rows(conn, rows_sql, sql, params);
        // As above: the next statement installs its own bound.
        let _ = conn.progress_handler(PROGRESS_HANDLER_OPS, None::<fn() -> bool>);
        ids
    }

    /// The columns `sql` would produce against `database`, without changing anything: what
    /// the extended query protocol's `Describe` answers.
    ///
    /// A column the engine produced from a TABLE carries the type its declaration names. A
    /// COMPUTED column (an aggregate, an expression, a cast) has no declaration, and its
    /// type is the type of the value it carries — so for a statement the engine itself
    /// reports as READ-ONLY (`sqlite3_stmt_readonly`), the engine is asked for its first row
    /// and the type it holds each undeclared value as is the type that column is described
    /// under. Nothing else changes: a statement that WRITES is never run to describe it, and
    /// a column the engine holds as text or as NULL keeps the type a client decodes text
    /// with, which is what this described before. The row is read under this endpoint's own
    /// statement budget, so describing cannot hold the engine for longer than running the
    /// statement may. A session inside a transaction is described against its OWN copy of
    /// the database, which is where its statements run, and a portal is described with the
    /// values the client bound to it.
    pub fn describe(
        &self,
        session: u32,
        database: &str,
        sql: &str,
        params: &[Option<String>],
    ) -> Result<Vec<ColumnInfo>, EngineError> {
        let mut databases = self.lock();
        let conn = connection_for(
            &mut databases,
            &self.view,
            &self.transactions,
            self.volume.as_ref(),
            Some(session),
            database,
        )?;
        let deadline = Instant::now() + STATEMENT_DEADLINE;
        conn.progress_handler(PROGRESS_HANDLER_OPS, Some(move || Instant::now() >= deadline))
            .map_err(from_engine)?;
        let described = described_columns(conn, sql, params);
        // As everywhere else: the bound is removed once this is over, and the next statement
        // installs its own.
        let _ = conn.progress_handler(PROGRESS_HANDLER_OPS, None::<fn() -> bool>);
        described
    }

    /// Copy every database this instance holds into a NEW engine, which is what TAKING a
    /// snapshot of a DB instance's storage is: the copy holds the databases, the schemas
    /// and the rows as they stand at this instant, and nothing written to this instance
    /// afterwards reaches it.
    ///
    /// Blocking, CPU-bound work, bounded exactly as [`Self::load_from`] is: a caller runs
    /// it on a blocking task, as a wire handler runs a statement.
    pub fn capture(&self) -> Result<Self, EngineError> {
        let copy = Self::open()?;
        copy.load_from(self)?;
        Ok(copy)
    }

    /// Replace this engine's databases with `source`'s, which is what RESTORING a snapshot
    /// into a new DB instance's storage is.
    ///
    /// This instance ends up holding exactly the databases the source holds, each with
    /// exactly its contents: a restore REPLACES what the engine had rather than merging
    /// into it. Each database is copied by the engine's own online backup
    /// ([`copy_database`]), and the databases the instance comes with are the source's, so
    /// a restored instance does not seed its own over them.
    ///
    /// Bounded and interruptible, so no request can wedge either engine: `source` must be
    /// a DIFFERENT instance and is REFUSED when it is this one, each end is taken with
    /// [`Self::lock_until`] under [`COPY_DEADLINE`] rather than waited on, and each copy is
    /// driven one step at a time against the same deadline. The number of databases is
    /// bounded by [`MAX_DATABASES_PER_INSTANCE`], which the source itself is held to.
    ///
    /// Blocking, CPU-bound work: a caller runs it on a blocking task.
    pub fn load_from(&self, source: &Self) -> Result<(), EngineError> {
        // One engine cannot be both ends of a copy: the second lock would be the one this
        // thread already holds.
        if std::ptr::eq(self, source) {
            return Err(EngineError::new(
                EngineErrorKind::Internal,
                "an instance's databases cannot be copied over themselves",
            ));
        }
        let deadline = Instant::now() + COPY_DEADLINE;
        let busy = |end: &str| {
            EngineError::new(
                EngineErrorKind::Cancelled,
                format!(
                    "the {end} of a copy was in use for longer than {} seconds",
                    COPY_DEADLINE.as_secs()
                ),
            )
        };
        // Both ends are taken under the one deadline, so two copies that each hold what
        // the other needs both give up instead of deadlocking.
        let source = source.lock_until(deadline).ok_or_else(|| busy("source"))?;
        let mut destination = self.lock_until(deadline).ok_or_else(|| busy("destination"))?;
        destination.open.clear();
        // The databases the INSTANCE comes with are the ones the source had: a restored
        // instance that has already been handed a database must not seed another over it.
        destination.seeded = source.seeded;
        for (name, held) in &source.open {
            let mut copy = open_recorded_database(
                &self.view,
                &self.transactions,
                name,
                self.backing(name).as_deref(),
            )?;
            let _pages = copy_database(held, &mut copy, deadline)?;
            destination.open.insert(name.clone(), copy);
        }
        Ok(())
    }

    /// The databases this instance has, in name order — what a login may name and what
    /// `SHOW DATABASES` lists.
    ///
    /// A database with no name is not one of them: a MySQL session opened without naming
    /// one runs its unqualified statements against a database of its own, which no client
    /// can name and no listing shows.
    #[must_use]
    pub fn database_names(&self) -> Vec<String> {
        self.lock()
            .open
            .keys()
            .filter(|name| !name.is_empty())
            .cloned()
            .collect()
    }

    /// Whether this instance holds the database `name`.
    #[must_use]
    pub fn has_database(&self, name: &str) -> bool {
        !name.is_empty() && resolve(&self.lock().open, name).is_some()
    }

    /// Create the database `name` empty, and answer whether it was created — `false`
    /// where the instance already held it, which is what makes an `IF NOT EXISTS` create a
    /// no-op rather than a second database.
    pub fn create_database(&self, name: &str) -> Result<bool, EngineError> {
        let mut databases = self.lock();
        if resolve(&databases.open, name).is_some() {
            return Ok(false);
        }
        admit_another_database(&databases)?;
        let opened = open_recorded_database(
            &self.view,
            &self.transactions,
            name,
            self.backing(name).as_deref(),
        )?;
        databases.open.insert(name.to_string(), opened);
        Ok(true)
    }

    /// Carry the TABLES of `source` into the database `name` on this engine, creating that
    /// database when the engine does not hold it yet, and answer how many tables were carried.
    ///
    /// [`Self::load_from`] REPLACES an engine's whole set of databases, which is what
    /// restoring a snapshot into a new resource's storage is. This ADDS to one of them and
    /// replaces nothing — a table the database already holds is left exactly as it is — which
    /// is what carrying in the data another ENDPOINT of the same resource holds is: a DB
    /// cluster's Data API HTTP endpoint is served by another service, which keeps the rows
    /// written through it in its own engine, in a database of the same name (see
    /// [`super::super::data_api_rows`]).
    ///
    /// Each object is created from the DECLARATION its source holds and each table's rows are
    /// carried over as the engine's own typed values, so every column keeps the type and the
    /// bytes it had; the indexes, views and triggers over those tables follow them. The carry
    /// reads `source` with ordinary SQL — its catalog and a `SELECT` per table — so that
    /// connection has to admit those reads.
    ///
    /// Bounded like [`Self::load_from`]: the destination is taken with [`Self::lock_until`]
    /// under [`COPY_DEADLINE`] rather than waited on, the carry is checked against that same
    /// deadline as it goes, and the databases stay bounded by [`MAX_DATABASES_PER_INSTANCE`].
    /// Blocking, CPU-bound work: a caller runs it on a blocking task, as a wire handler runs a
    /// statement.
    pub fn carry_tables_from(&self, name: &str, source: &Connection) -> Result<usize, EngineError> {
        let deadline = Instant::now() + COPY_DEADLINE;
        let mut databases = self.lock_until(deadline).ok_or_else(|| {
            EngineError::new(
                EngineErrorKind::Cancelled,
                format!(
                    "the destination of a copy was in use for longer than {} seconds",
                    COPY_DEADLINE.as_secs()
                ),
            )
        })?;
        // The name the engine already holds this database under, which the carry writes into:
        // the engines resolve a database name ignoring case, so this does not open a second
        // database beside one that only differs in case.
        let destination = match resolve(&databases.open, name) {
            Some(held) => match databases.open.get_mut(&held) {
                Some(destination) => destination,
                // `resolve` answers with a key of this very map, so there is nothing to carry
                // into only if it holds no such database after all.
                None => return Ok(0),
            },
            None => {
                admit_another_database(&databases)?;
                let opened = open_recorded_database(
                    &self.view,
                    &self.transactions,
                    name,
                    self.backing(name).as_deref(),
                )?;
                databases.open.entry(name.to_string()).or_insert(opened)
            }
        };
        carry_tables(source, destination, deadline)
    }

    /// Drop the database `name` with every table in it, and answer whether it was there.
    pub fn drop_database(&self, name: &str) -> bool {
        let mut databases = self.lock();
        match resolve(&databases.open, name) {
            Some(name) => databases.open.remove(&name).is_some(),
            None => false,
        }
    }

    /// Roll back the transaction every session that still holds one left open.
    ///
    /// What the end of an engine's RUN does: a reboot or a stop severs every session, and
    /// work none of them committed does not survive it. A session that ends on its own rolls
    /// back its own transaction ([`Self::rollback_transaction`]).
    pub fn rollback_open_transactions(&self) {
        for session in self.transactions.open_sessions() {
            let _ = self.rollback_transaction(session);
        }
    }

    /// How many BYTES the data this instance holds OCCUPIES, as the engine itself accounts for
    /// it: the pages each of its databases has allocated, at the size the engine writes a page
    /// at.
    ///
    /// The engine's own measurement, not an estimate over the values: `page_count` is what the
    /// database has grown to and `page_size` the size of each of those pages, which is the
    /// space the stored data takes — the figure the storage a DB instance has allocated is
    /// consumed by ([`crate::state::storage_autoscaling`]). Reading it costs one pragma per
    /// database, so it can be asked on the path that materialises what the clock owes rather
    /// than only when a copy is taken ([`crate::state::snapshot_data_bytes`] walks the values
    /// instead, because a SNAPSHOT reports the size of the data it carries).
    ///
    /// Read with the confinement lifted for the span of the read ([`Unconfined`]), because a
    /// pragma is engine configuration and no statement a client sends may reach one; a
    /// database whose pragma the engine refuses contributes nothing rather than failing the
    /// measurement.
    #[must_use]
    pub fn stored_bytes(&self) -> i64 {
        let mut databases = self.lock();
        let mut bytes: i64 = 0;
        for connection in databases.open.values_mut() {
            let mut lifted = Unconfined::of(connection);
            let connection = lifted.connection();
            let pages: i64 = connection
                .pragma_query_value(None, "page_count", |row| row.get(0))
                .unwrap_or_default();
            let page_size: i64 = connection
                .pragma_query_value(None, "page_size", |row| row.get(0))
                .unwrap_or_default();
            bytes = bytes.saturating_add(pages.saturating_mul(page_size));
        }
        bytes
    }
}
/// The name `name` names among `databases`, which is the one it matches ignoring case:
/// the engines resolve a database name case-insensitively on the platforms RDS runs them
/// on, so `USE APPDB` reaches the database `appdb` rather than opening a second one.
fn resolve(databases: &BTreeMap<String, Connection>, name: &str) -> Option<String> {
    databases
        .keys()
        .find(|known| known.eq_ignore_ascii_case(name))
        .cloned()
}

/// Whether one more database may be opened on this instance; see
/// [`MAX_DATABASES_PER_INSTANCE`].
fn admit_another_database(databases: &Databases) -> Result<(), EngineError> {
    if databases.open.len() >= MAX_DATABASES_PER_INSTANCE {
        return Err(EngineError::new(
            EngineErrorKind::LimitExceeded,
            format!(
                "this endpoint serves at most {MAX_DATABASES_PER_INSTANCE} databases per DB \
                 instance"
            ),
        ));
    }
    Ok(())
}

/// The connection the database `name` runs on, opened empty on first use.
///
/// A name reaches this only after the wire layer has admitted it (a login, a `USE`, the
/// DDL that created it), so opening on first use is what gives the instance's own
/// databases — the one `DBName` named, the engine's default, a MySQL system schema — a
/// database each without the engine having to be told their names when it starts.
fn connection<'a>(
    databases: &'a mut Databases,
    view: &Arc<Mutex<ServerView>>,
    transactions: &Arc<OpenTransactions>,
    volume: Option<&::service_common::cluster_sql_volumes::ClusterVolume>,
    name: &str,
) -> Result<&'a mut Connection, EngineError> {
    let key = resolve(&databases.open, name).unwrap_or_else(|| name.to_string());
    if !databases.open.contains_key(&key) {
        admit_another_database(databases)?;
        let backing = volume.map(|volume| volume.backing(&key));
        let opened =
            open_recorded_database(view, transactions, &key, backing.as_deref())?;
        databases.open.insert(key.clone(), opened);
    }
    databases
        .open
        .get_mut(&key)
        .ok_or_else(|| EngineError::new(EngineErrorKind::Internal, "the database is not open"))
}

/// The connection a statement of `session`'s runs on: that session's own copy of the
/// database where its transaction has one ([`InstanceDatabase::take_the_sessions_snapshot`]),
/// and the shared database otherwise.
///
/// This is the ONE place that choice is made, so every statement of a session inside a
/// transaction — the client's own, the predicate that identifies the rows it locks, and the
/// server's catalog statements for it — runs against the same database, and no statement of
/// a session outside one can reach another session's uncommitted rows.
fn connection_for<'a>(
    databases: &'a mut Databases,
    view: &Arc<Mutex<ServerView>>,
    transactions: &Arc<OpenTransactions>,
    volume: Option<&::service_common::cluster_sql_volumes::ClusterVolume>,
    session: Option<u32>,
    name: &str,
) -> Result<&'a mut Connection, EngineError> {
    let key = resolve(&databases.open, name).unwrap_or_else(|| name.to_string());
    if let Some(session) = session {
        if databases.working.contains_key(&(session, key.clone())) {
            return databases
                .working
                .get_mut(&(session, key))
                .ok_or_else(|| {
                    EngineError::new(EngineErrorKind::Internal, "the database is not open")
                });
        }
    }
    connection(databases, view, transactions, volume, &key)
}

/// One database of the instance, with the rows its connection changes reported to
/// `transactions` — which is what makes a session's `ROLLBACK` able to put its own rows
/// back.
/// `backing` is where its pages live: the backing of this database in the DB cluster's own
/// volume for an engine that serves a cluster's storage ([`InstanceDatabase::volume`]), and
/// `None` for a database of this engine's own.
fn open_recorded_database(
    view: &Arc<Mutex<ServerView>>,
    transactions: &Arc<OpenTransactions>,
    name: &str,
    backing: Option<&str>,
) -> Result<Connection, EngineError> {
    let mut conn = open_confined_database(view, backing)?;
    session_transactions::record_changes_of(&mut conn, name, transactions).map_err(unavailable)?;
    Ok(conn)
}

/// Whether the engine can be started on this host at all: one database is opened under
/// the bounds above and closed again, so a DB instance whose engine cannot run is refused
/// at create time rather than publishing an endpoint nothing serves.
fn probe_the_engine_starts(view: &Arc<Mutex<ServerView>>) -> Result<(), EngineError> {
    drop(open_confined_database(view, None)?);
    Ok(())
}

/// When a statement the session `view` describes must be cancelled, and whether it is the
/// session's own `statement_timeout` that decides it rather than this endpoint's budget.
///
/// The ONE reading of a statement's budget: a statement the engine runs, the row query that
/// identifies the rows it locks and the wait for a row another transaction holds are bounded
/// by the same instant, so nothing a session asks for outlives the budget that session's
/// statement has. It is also the one place the session's own number is read, so the instant
/// is computed with a CHECKED addition: a timeout no clock can represent leaves this
/// endpoint's own budget in force instead of failing the thread that is running the
/// statement.
pub fn statement_deadline(view: &ServerView) -> (Instant, bool) {
    let endpoint_deadline = Instant::now() + STATEMENT_DEADLINE;
    // `statement_timeout` is a value the SESSION sets over the wire, and
    // `Instant + Duration` PANICS on overflow rather than saturating, so the addition is
    // checked: a timeout too large to represent cannot be smaller than this endpoint's
    // budget, so it loses the comparison below exactly as a representable large one does.
    let timeout = (view.statement_timeout_ms > 0)
        .then(|| Instant::now().checked_add(Duration::from_millis(view.statement_timeout_ms)))
        .flatten();
    match timeout {
        Some(timeout) if timeout <= endpoint_deadline => (timeout, true),
        _ => (endpoint_deadline, false),
    }
}

/// The row ids `rows_sql` selects, under whatever bound the caller installed on `conn`; see
/// [`InstanceDatabase::row_ids`], which is the only caller.
fn identified_rows(
    conn: &Connection,
    rows_sql: &str,
    sql: &str,
    params: &[Option<String>],
) -> Option<Vec<i64>> {
    let mut rows = conn.prepare(rows_sql).ok()?;
    if rows.parameter_count() > 0 {
        // The row query carries the statement's own placeholders, so each value is bound to
        // the placeholder of the same NAME in the statement the client sent — the positions
        // of the two differ.
        let original = conn.prepare(sql).ok()?;
        for index in 1..=rows.parameter_count() {
            let name = rows.parameter_name(index)?.to_string();
            let at = original.parameter_index(&name).ok()??;
            let value = params.get(at.checked_sub(1)?)?;
            rows.raw_bind_parameter(index, value.as_deref()).ok()?;
        }
    }
    let mut ids = Vec::new();
    let mut cursor = rows.raw_query();
    while let Some(row) = cursor.next().ok()? {
        if ids.len() >= MAX_LOCKED_ROWS {
            return None;
        }
        ids.push(row.get::<usize, i64>(0).ok()?);
    }
    Some(ids)
}

/// The connection with its authorizer lifted, which CONFINES IT AGAIN when dropped.
///
/// The one span this emulator runs a statement of its own outside the default-deny authorizer
/// is putting a rolled-back transaction's rows back, which needs the engine's
/// `defer_foreign_keys` ([`InstanceDatabase::put_the_rows_back`]). Holding the lift in a guard
/// is what makes the confinement come back on every path out of that span, an early return and
/// an unwind included — a connection left unconfined would stay so for every later session.
struct Unconfined<'a>(&'a mut Connection);

impl<'a> Unconfined<'a> {
    /// Lift the authorizer on `conn` until the guard is dropped.
    ///
    /// A lift the engine refuses leaves the connection CONFINED, which the restore below
    /// then fails on with the engine's own refusal — never running unconfined.
    fn of(conn: &'a mut Connection) -> Self {
        let _ = conn.authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
        Self(conn)
    }

    /// The lifted connection, for the server's own work inside this span. Reached through
    /// the guard so the connection cannot be used while the lift is not held.
    fn connection(&mut self) -> &mut Connection {
        self.0
    }
}

impl Drop for Unconfined<'_> {
    fn drop(&mut self) {
        // The confinement comes back on every path out of the span. The engine refuses to
        // set an authorizer only on a connection that is no longer usable at all — every
        // later statement on it fails — and a drop has nothing to report, so the refusal is
        // not propagated here.
        let _ = self.0.authorizer(Some(confine_to_the_instance));
    }
}

/// One empty in-memory database, with every bound above in force — and with the server
/// functions this endpoint defines registered on it ([`register_server_functions`]) —
/// before any client statement can reach it.
/// `backing` says WHERE its pages live, and nothing else: the backing of one database of a
/// DB cluster's volume, which every surface of that cluster opens its own connection onto
/// ([`::service_common::cluster_sql_volumes::open_backing`]), or `None` for a database of
/// this engine's own. Either way the bounds, the confinement and the functions below are
/// this connection's.
fn open_confined_database(
    view: &Arc<Mutex<ServerView>>,
    backing: Option<&str>,
) -> Result<Connection, EngineError> {
    let mut conn =
        ::service_common::cluster_sql_volumes::open_backing(backing).map_err(unavailable)?;
    // Temp database in memory FIRST, so naming `temp` below opens it in memory and
    // not as a host file; then the page ceiling on both schemas.
    conn.pragma_update(None, "temp_store", TEMP_STORE_MEMORY)
        .map_err(unavailable)?;
    conn.pragma_update(None, "cache_size", MAX_PAGE_CACHE_KIB)
        .map_err(unavailable)?;
    for schema in CAPPED_SCHEMAS {
        conn.pragma_update(Some(schema), "max_page_count", MAX_DATABASE_PAGES)
            .map_err(unavailable)?;
    }
    for (limit, value) in [
        // No second database, ever: this also takes `VACUUM INTO` away, which the
        // engine implements by attaching its output file.
        (Limit::SQLITE_LIMIT_ATTACHED, 0),
        (Limit::SQLITE_LIMIT_LENGTH, MAX_VALUE_BYTES),
        (Limit::SQLITE_LIMIT_SQL_LENGTH, MAX_SQL_BYTES),
        (Limit::SQLITE_LIMIT_EXPR_DEPTH, MAX_EXPRESSION_DEPTH),
        (Limit::SQLITE_LIMIT_COMPOUND_SELECT, MAX_COMPOUND_SELECT),
        (Limit::SQLITE_LIMIT_VARIABLE_NUMBER, MAX_BOUND_PARAMETERS),
    ] {
        conn.set_limit(limit, value).map_err(unavailable)?;
    }
    for (config, enabled) in [
        (rusqlite::config::DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false),
        (rusqlite::config::DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true),
    ] {
        conn.set_db_config(config, enabled).map_err(unavailable)?;
    }
    // Installed LAST, so the settings above — themselves pragmas — are not refused
    // by it.
    conn.authorizer(Some(confine_to_the_instance))
        .map_err(unavailable)?;
    register_server_functions(&conn, view)?;
    super::engine_time::register_time_functions(&conn, view)?;
    super::mysql_insert_id::register_insert_id_function(&conn, view)?;
    super::pg_numeric::register_numeric_functions(&conn)?;
    Ok(conn)
}

/// WHERE the shared database stood when it was last looked at: the engine's own count of the
/// rows changed on the connection this endpoint holds that database open on.
///
/// Every write any session of this endpoint has committed into the shared database goes
/// through that one connection — a statement outside a transaction, the rows a commit carries
/// over, the catalog rows the server rebuilds — so a mark that has MOVED says the database a
/// transaction's copy was taken from is no longer the database it was. What it does not say is
/// how much changed or when: it is a mark to compare, never a count to report
/// ([`InstanceDatabase::take_this_statements_snapshot`]).
fn changes_mark(conn: &Connection) -> i64 {
    i64::try_from(conn.total_changes()).unwrap_or_default()
}

/// Copy `source` over `destination` with the engine's own page-level copy.
///
/// `sqlite3_backup_*` between two OPEN connections, so no path is named and no file is
/// opened — unlike `VACUUM INTO`, which the authorizer denies. It also runs beneath SQL,
/// so it copies every table, index, view and trigger of the source exactly, with no
/// dialect or quoting of its own to get wrong.
///
/// Driven one step at a time rather than through `run_to_completion`, so `deadline` is
/// checked between steps: that is the cancellation point a bounded request needs. The
/// page ceiling is a property of the destination CONNECTION and not of the pages copied
/// into it, so the destination keeps the bound [`open_confined_database`] installed and the
/// copy cannot exceed it — the engine answers `SQLITE_FULL` if a source ever did.
///
/// Answers with how many PAGES the copy holds, which is what a caller keeping it charges
/// against its own budget.
fn copy_database(
    source: &Connection,
    destination: &mut Connection,
    deadline: Instant,
) -> Result<i64, EngineError> {
    let backup = ::rusqlite::backup::Backup::new(source, destination).map_err(unavailable)?;
    loop {
        if Instant::now() >= deadline {
            return Err(EngineError::new(
                EngineErrorKind::Cancelled,
                format!(
                    "copying a database took longer than {} seconds",
                    COPY_DEADLINE.as_secs()
                ),
            ));
        }
        match backup.step(BACKUP_PAGES_PER_STEP).map_err(unavailable)? {
            ::rusqlite::backup::StepResult::Done => {
                return Ok(i64::from(backup.progress().pagecount))
            }
            ::rusqlite::backup::StepResult::More => {}
            // Neither end is shared with anything but this copy, which holds both ends'
            // engines, so a busy or locked step is the engine reporting a state this copy
            // cannot resolve by waiting.
            unresolvable => {
                return Err(EngineError::new(
                    EngineErrorKind::Internal,
                    format!("the engine answered {unresolvable:?} while copying a database"),
                ))
            }
        }
    }
}

/// Carry every table of `source` that `destination` does not already hold into it, with its
/// rows and then the indexes, views and triggers defined over those tables, and answer how
/// many TABLES were carried.
///
/// Table by table rather than page by page ([`copy_database`]), because this ADDS to a
/// database that already holds its own objects: a page copy would replace them. Each object is
/// created from the DECLARATION its own engine holds for it and each table's rows are carried
/// over by the same carry a commit makes ([`super::session_transactions::copy_table`]), as the
/// engine's own typed values and in the kind order that carry declares
/// ([`super::session_transactions::SCHEMA_ORDER`]) — so nothing about a column's type or a
/// value's bytes is re-rendered here, and there is one carry of one engine's schema into
/// another's rather than two.
///
/// A table's DECLARATION and its ROWS are carried inside one transaction of the destination's
/// own, so a carry the engine refuses part way through — a row it will not take, the deadline
/// — leaves the destination exactly as it was rather than a table holding some of its rows:
/// an export must never report a row count no database holds. An object defined OVER those
/// tables is logged and passed over instead, since it carries no rows of its own.
///
/// `deadline` is checked between objects and, by that same carry, between rows; the
/// destination keeps the page ceiling its connection was opened with, so the engine answers
/// `SQLITE_FULL` rather than growing past it.
fn carry_tables(
    source: &Connection,
    destination: &mut Connection,
    deadline: Instant,
) -> Result<usize, EngineError> {
    let objects = super::session_transactions::schema_objects(source)?;
    // What the destination declares already, read once: a carry never writes over an object
    // the database it carries into has of its own. A name is unique across an engine's
    // schema, so the kind it was declared as does not enter the question.
    let held: ::std::collections::BTreeSet<String> =
        super::session_transactions::schema_objects(destination)?
            .into_keys()
            .map(|(name, _)| name)
            .collect();
    let mut carried = 0;
    for kind in super::session_transactions::SCHEMA_ORDER {
        for ((name, declared_as), declaration) in &objects {
            if declared_as != kind || held.contains(name) {
                continue;
            }
            if Instant::now() >= deadline {
                return Err(carry_cancelled());
            }
            if kind != "table" {
                if let Err(error) = destination.execute_batch(declaration) {
                    ::tracing::warn!(
                        object = %name,
                        kind = %kind,
                        %error,
                        "an object defined over a carried table could not be created"
                    );
                }
                continue;
            }
            let carrying = destination.transaction().map_err(from_engine)?;
            carrying.execute_batch(declaration).map_err(from_engine)?;
            super::session_transactions::copy_table(&carrying, source, name, deadline)?;
            carrying.commit().map_err(from_engine)?;
            carried += 1;
        }
    }
    Ok(carried)
}

/// A carry that ran past [`COPY_DEADLINE`], worded as the copy it is.
fn carry_cancelled() -> EngineError {
    EngineError::new(
        EngineErrorKind::Cancelled,
        format!(
            "carrying a database's tables took longer than {} seconds",
            COPY_DEADLINE.as_secs()
        ),
    )
}

/// An engine that could not be brought up or configured at all.
fn unavailable(error: rusqlite::Error) -> EngineError {
    EngineError::new(EngineErrorKind::Internal, error.to_string())
}

/// How long `pg_sleep` waits between checks of its statement's deadline, so a sleep that is
/// cut short by a `statement_timeout` is cut short promptly rather than at its end.
const SLEEP_SLICE: Duration = Duration::from_millis(20);

/// Register the PostgreSQL server functions on this connection, so a statement that calls
/// one is planned and run by the engine like any other.
///
/// Each answers from the session view the statement is running under
/// ([`InstanceDatabase::execute_as`]) — the instance's own runtime and the calling session
/// — and nothing else; the catalogue of which functions exist and what each returns is
/// [`super::pg_server::SERVER_FUNCTIONS`].
fn register_server_functions(
    conn: &Connection,
    view: &Arc<Mutex<ServerView>>,
) -> Result<(), EngineError> {
    let read = |view: &Arc<Mutex<ServerView>>| -> ServerView {
        view.lock().unwrap_or_else(PoisonError::into_inner).clone()
    };

    let start_time = Arc::clone(view);
    conn.create_scalar_function(
        "pg_postmaster_start_time",
        0,
        FunctionFlags::SQLITE_UTF8,
        move |_| Ok(super::pg_server::timestamptz_text(read(&start_time).postmaster_start_time)),
    )
    .map_err(unavailable)?;

    let recovery = Arc::clone(view);
    conn.create_scalar_function(
        "pg_is_in_recovery",
        0,
        FunctionFlags::SQLITE_UTF8,
        move |_| Ok(i64::from(read(&recovery).in_recovery)),
    )
    .map_err(unavailable)?;

    let backend = Arc::clone(view);
    conn.create_scalar_function("pg_backend_pid", 0, FunctionFlags::SQLITE_UTF8, move |_| {
        Ok(i64::from(read(&backend).backend_pid))
    })
    .map_err(unavailable)?;

    // `inet_server_addr()` — "Returns the IP address on which the server accepted the current
    // connection" — and `inet_server_port()`, its port. They are the SERVER end of the
    // session: for a session on a database's own endpoint that is the listener it arrived on,
    // and for one pooled through a DB proxy it is the endpoint of the DATABASE serving it,
    // since the backend answering the statement is that database's and not the proxy's.
    // `NULL` for a server that published no address, which is what PostgreSQL answers for a
    // connection made over a Unix-domain socket.
    let server_address = Arc::clone(view);
    conn.create_scalar_function(
        "inet_server_addr",
        0,
        FunctionFlags::SQLITE_UTF8,
        move |_| {
            let address = read(&server_address).server_address;
            Ok((!address.is_empty()).then_some(address))
        },
    )
    .map_err(unavailable)?;

    let server_port = Arc::clone(view);
    conn.create_scalar_function(
        "inet_server_port",
        0,
        FunctionFlags::SQLITE_UTF8,
        move |_| {
            let view = read(&server_port);
            Ok((!view.server_address.is_empty()).then(|| i64::from(view.server_port)))
        },
    )
    .map_err(unavailable)?;

    // `pg_sleep(seconds)` — "Makes the current session's process sleep until the given
    // number of seconds have elapsed". It really waits, and it stops waiting when the
    // statement's deadline passes, which is what makes a `statement_timeout` observable
    // through it. The wait is bounded three times over: by the deadline the statement runs
    // under, by this endpoint's own budget from the moment the wait starts — which is the
    // bound that holds when no statement has published a deadline on this connection yet —
    // and by an hour for the argument itself, so the connection this holds, and the
    // instance's engine lock with it, cannot be held by a caller's own number.
    let sleeping = Arc::clone(view);
    conn.create_scalar_function("pg_sleep", 1, FunctionFlags::SQLITE_UTF8, move |context| {
        let seconds = context.get::<f64>(0).unwrap_or(0.0).max(0.0);
        let view = read(&sleeping);
        let entered = Instant::now();
        let endpoint_ceiling = entered + STATEMENT_DEADLINE;
        let ceiling = view
            .deadline
            .map_or(endpoint_ceiling, |deadline| deadline.min(endpoint_ceiling));
        let until = entered
            .checked_add(Duration::from_secs_f64(seconds.min(3_600.0)))
            .unwrap_or(ceiling);
        loop {
            let now = Instant::now();
            if now >= until {
                return Ok(Option::<String>::None);
            }
            if now >= ceiling {
                return Err(rusqlite::Error::UserFunctionError(
                    STATEMENT_TIMEOUT_CANCELLATION.into(),
                ));
            }
            std::thread::sleep(SLEEP_SLICE.min(until.saturating_duration_since(now)));
        }
    })
    .map_err(unavailable)?;

    // `current_setting(name [, missing_ok])` — "Returns the current value of the setting",
    // erroring on an unrecognised name unless the second argument asks for NULL instead.
    let settings = Arc::clone(view);
    conn.create_scalar_function(
        "current_setting",
        -1,
        FunctionFlags::SQLITE_UTF8,
        move |context| {
            let name = context.get::<String>(0).unwrap_or_default();
            let missing_ok = context.len() > 1 && context.get::<i64>(1).unwrap_or(0) != 0;
            match read(&settings).settings.get(&name.to_ascii_lowercase()) {
                Some(value) => Ok(Some(value.clone())),
                None if missing_ok => Ok(None),
                None => Err(rusqlite::Error::UserFunctionError(
                    format!("unrecognized configuration parameter \"{name}\"").into(),
                )),
            }
        },
    )
    .map_err(unavailable)?;

    // `pg_has_role(member)` — the truth of the membership the role catalog resolved for the
    // call, which the statement carries in place of its arguments
    // (`super::pg_server::catalog_function_lookup`). PostgreSQL returns a boolean, and this
    // is what makes the column one: it is named after the function and reported under the
    // function's own return type.
    conn.create_scalar_function(
        "pg_has_role",
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |context| match context.get_raw(0) {
            ValueRef::Null => Ok(None),
            ValueRef::Integer(truth) => Ok(Some(i64::from(truth != 0))),
            ValueRef::Real(truth) => Ok(Some(i64::from(truth != 0.0))),
            ValueRef::Text(bytes) | ValueRef::Blob(bytes) => {
                Ok(Some(i64::from(!bytes.is_empty() && bytes != b"0")))
            }
        },
    )
    .map_err(unavailable)?;

    // The RELATION-SIZE functions, as the ONE argument their lookup leaves each of them: the
    // bytes the relation occupies, which the engine resolved out of the relation catalog
    // (`super::pg_server::relation_size_lookup`). The call is kept so the column is named
    // after the function and reported under its own type, exactly as `pg_has_role` is; a
    // relation the catalog holds no row for answers NULL, as PostgreSQL's own does.
    for named in super::pg_server::RELATION_SIZE_FUNCTIONS {
        conn.create_scalar_function(
            named,
            1,
            FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
            |context| match context.get_raw(0) {
                ValueRef::Null => Ok(None),
                ValueRef::Integer(bytes) => Ok(Some(bytes)),
                ValueRef::Real(bytes) => Ok(Some(bytes as i64)),
                ValueRef::Text(bytes) | ValueRef::Blob(bytes) => Ok(String::from_utf8_lossy(bytes)
                    .trim()
                    .parse::<i64>()
                    .ok()),
            },
        )
        .map_err(unavailable)?;
    }

    // `to_regclass`, as the ONE argument its own lookup leaves it: the relation the engine
    // resolved out of the relation catalog (`super::pg_server::to_regclass_lookup`). The call
    // is kept so the column is named after the function and reported under its own type, as
    // the relation-size functions are; a name this database holds no relation of answers
    // NULL, which is what that function answers rather than raising an error.
    conn.create_scalar_function(
        "to_regclass",
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |context| match context.get_raw(0) {
            ValueRef::Null => Ok(None),
            ValueRef::Integer(oid) => Ok(Some(oid.to_string())),
            ValueRef::Real(oid) => Ok(Some(oid.to_string())),
            ValueRef::Text(bytes) | ValueRef::Blob(bytes) => {
                Ok(Some(String::from_utf8_lossy(bytes).into_owned()))
            }
        },
    )
    .map_err(unavailable)?;

    // `md5(text)` — "Computes the MD5 hash of the argument, with the result written in
    // hexadecimal" on PostgreSQL, and the same digest under the same name on MySQL. The
    // digest is computed over the argument's own bytes, so what the client hashes is what
    // the client sent; a NULL argument hashes to NULL, as both engines answer.
    conn.create_scalar_function(
        "md5",
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |context| {
            let Some(bytes) = md5_argument(context.get_raw(0)) else {
                return Ok(None);
            };
            Ok(Some(::hex::encode(
                <::md5::Md5 as ::md5::Digest>::digest(bytes),
            )))
        },
    )
    .map_err(unavailable)?;

    // `char_length(text)` / `character_length(text)` — the number of CHARACTERS in the
    // argument, which both families define under both names. The count is over the
    // argument's own characters, so a multi-byte character counts once, which is what
    // separates it from the byte length.
    for name in ["char_length", "character_length"] {
        conn.create_scalar_function(
            name,
            1,
            FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
            |context| Ok(measured(context.get_raw(0), Measure::Characters)),
        )
        .map_err(unavailable)?;
    }

    // `length(x)` means a DIFFERENT measure in each family: MySQL's "returns the length of
    // the string measured in bytes", while PostgreSQL's is the number of characters — and
    // `octet_length(x)`, which both define, is the bytes. So the measure follows the wire the
    // session speaks, and a client asking its own engine's question gets its own engine's
    // answer.
    let measures = Arc::clone(view);
    conn.create_scalar_function(
        "length",
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        move |context| {
            let measure = match read(&measures).wire {
                Some(super::EngineWire::MySql) => Measure::Bytes,
                _ => Measure::Characters,
            };
            Ok(measured(context.get_raw(0), measure))
        },
    )
    .map_err(unavailable)?;
    conn.create_scalar_function(
        "octet_length",
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |context| Ok(measured(context.get_raw(0), Measure::Bytes)),
    )
    .map_err(unavailable)?;
    Ok(())
}

/// WHAT a length function counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Measure {
    /// The characters in the value, so a multi-byte character counts once.
    Characters,
    /// The bytes the value occupies.
    Bytes,
}

/// The length of `value` under `measure`, or `None` for SQL NULL — whose length is NULL on
/// both engines.
///
/// A number is measured as the digits the engine renders it with, which is the string both
/// engines measure for a numeric argument; a byte string is its own bytes either way, having
/// no character boundaries to count.
fn measured(value: ValueRef<'_>, measure: Measure) -> Option<i64> {
    let count = |bytes: &[u8]| match measure {
        Measure::Bytes => bytes.len(),
        Measure::Characters => String::from_utf8_lossy(bytes).chars().count(),
    };
    let measured = match value {
        ValueRef::Null => return None,
        ValueRef::Text(bytes) => count(bytes),
        ValueRef::Blob(bytes) => bytes.len(),
        ValueRef::Integer(number) => count(number.to_string().as_bytes()),
        ValueRef::Real(number) => count(number.to_string().as_bytes()),
    };
    Some(i64::try_from(measured).unwrap_or_default())
}

/// The bytes `md5()` hashes: the argument's own, as the engine holds them — text as its
/// UTF-8 bytes, a byte string as itself, and a number as the digits the engine renders it
/// with, which is what both engines hash for a numeric argument. `None` for SQL NULL, whose
/// digest is NULL.
fn md5_argument(value: ValueRef<'_>) -> Option<Vec<u8>> {
    match value {
        ValueRef::Null => None,
        ValueRef::Integer(number) => Some(number.to_string().into_bytes()),
        ValueRef::Real(number) => Some(number.to_string().into_bytes()),
        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => Some(bytes.to_vec()),
    }
}

/// Prepare, bind and run one statement, reading its rows out under the result caps.
fn run_statement(
    conn: &Connection,
    sql: &str,
    params: &[Option<String>],
    row_ceiling: usize,
) -> Result<ResultSet, EngineError> {
    let mut statement = conn.prepare(sql).map_err(from_engine)?;
    if statement.parameter_count() != params.len() {
        return Err(EngineError::new(
            EngineErrorKind::Syntax,
            format!(
                "bind message supplies {} parameters, but the statement requires {}",
                params.len(),
                statement.parameter_count()
            ),
        ));
    }
    for (position, value) in params.iter().enumerate() {
        statement
            .raw_bind_parameter(position + 1, value.as_deref())
            .map_err(from_engine)?;
    }
    let columns: Vec<ColumnInfo> = statement
        .columns()
        .map(|column| named_column(column.name(), column.decl_type()))
        .collect();
    if columns.is_empty() {
        let affected = statement.raw_execute().map_err(from_engine)?;
        return Ok(ResultSet::changed(
            u64::try_from(affected).unwrap_or(u64::MAX),
            conn.last_insert_rowid(),
        ));
    }

    let mut rows = Vec::new();
    let mut bytes = 0usize;
    let mut cursor = statement.raw_query();
    while let Some(row) = cursor.next().map_err(from_engine)? {
        if rows.len() >= row_ceiling {
            return Err(EngineError::new(
                EngineErrorKind::LimitExceeded,
                format!("the result set is larger than this emulator serves ({row_ceiling} rows)"),
            ));
        }
        let mut values = Vec::with_capacity(columns.len());
        for index in 0..columns.len() {
            let value = row.get_ref(index).map_err(from_engine)?;
            // A column DECLARED with an exact-decimal scale carries that scale: the engine
            // holds such a value in its own affinity, so the digits its declaration gives it
            // are put back here (`super::pg_numeric::as_the_declaration_holds_it`).
            let scaled = columns
                .get(index)
                .and_then(|column| column.declared_type.as_deref())
                .and_then(|declared| {
                    super::pg_numeric::as_the_declaration_holds_it(declared, value)
                });
            let cell = match (scaled, value) {
                (Some(scaled), _) => Cell::Text(scaled),
                (None, ValueRef::Null) => Cell::Null,
                (None, ValueRef::Integer(value)) => Cell::Integer(value),
                (None, ValueRef::Real(value)) => Cell::Real(value),
                (None, ValueRef::Text(bytes)) => {
                    Cell::Text(String::from_utf8_lossy(bytes).into_owned())
                }
                (None, ValueRef::Blob(bytes)) => Cell::Blob(bytes.to_vec()),
            };
            bytes = bytes.saturating_add(cell.byte_cost());
            if bytes > MAX_RESULT_BYTES {
                return Err(EngineError::new(
                    EngineErrorKind::LimitExceeded,
                    format!(
                        "the result set is larger than this emulator serves ({} MiB of values)",
                        MAX_RESULT_BYTES / (1024 * 1024)
                    ),
                ));
            }
            values.push(cell);
        }
        rows.push(values);
    }
    // `changes()` and the assigned key belong to a statement that wrote rows — an
    // `INSERT ... RETURNING` both returns rows and changes them.
    Ok(ResultSet {
        columns,
        rows,
        affected: conn.changes(),
        last_insert_id: conn.last_insert_rowid(),
    })
}

/// One column as the wire reports it.
///
/// A column the engine produced from a TABLE carries the type its declaration names. A
/// column that is a call of one of this server's own functions carries no declaration, so
/// it takes the two facts PostgreSQL reports for it: the column is named after the function
/// and carries that function's return type
/// ([`super::pg_server::function_result_type`]) — which is what lets a client decode
/// `pg_is_in_recovery()` as a boolean and `pg_postmaster_start_time()` as a timestamp.
fn named_column(name: &str, declared: Option<&str>) -> ColumnInfo {
    if declared.is_none() {
        if let Some(function) = name.strip_suffix(')').and_then(|call| call.split('(').next()) {
            if let Some(result) = super::pg_server::function_result_type(function) {
                return ColumnInfo {
                    name: function.to_string(),
                    declared_type: Some(result.to_string()),
                };
            }
        }
    }
    ColumnInfo {
        name: name.to_string(),
        declared_type: declared.map(str::to_string),
    }
}

/// The columns `sql` produces, each under the type a client decodes it with; see
/// [`InstanceDatabase::describe`], which is the only caller and which installs the bound
/// this runs under.
fn described_columns(
    conn: &Connection,
    sql: &str,
    params: &[Option<String>],
) -> Result<Vec<ColumnInfo>, EngineError> {
    let mut statement = conn.prepare(sql).map_err(from_engine)?;
    let mut columns: Vec<ColumnInfo> = statement
        .columns()
        .map(|column| named_column(column.name(), column.decl_type()))
        .collect();
    let computed = columns
        .iter()
        .any(|column| column.declared_type.is_none());
    // Only a statement the ENGINE itself reports as reading nothing but rows is run to
    // describe it, so describing a statement changes nothing a later caller sees.
    if !computed || !statement.readonly() {
        return Ok(columns);
    }
    // The values the client bound to this portal, so the row the engine answers with is the
    // row this portal would answer with; a statement described before anything is bound to
    // it is described with its parameters unset, which is what the protocol asks for.
    if statement.parameter_count() == params.len() {
        for (position, value) in params.iter().enumerate() {
            if statement
                .raw_bind_parameter(position + 1, value.as_deref())
                .is_err()
            {
                return Ok(columns);
            }
        }
    }
    let mut cursor = statement.raw_query();
    if let Ok(Some(row)) = cursor.next() {
        for (index, column) in columns.iter_mut().enumerate() {
            if column.declared_type.is_some() {
                continue;
            }
            if let Ok(value) = row.get_ref(index) {
                column.declared_type = value_type(value).map(str::to_string);
            }
        }
    }
    Ok(columns)
}

/// The type of a value the engine HOLDS, in the type names a wire maps to its own type codes
/// (`super::pg_wire`, `super::mysql_wire`): the type a computed column is described under.
///
/// `None` for text and for NULL — a text value is already what a column with no narrower
/// type is reported as, and a NULL says nothing about the column's type.
fn value_type(value: ValueRef<'_>) -> Option<&'static str> {
    match value {
        ValueRef::Integer(_) => Some("bigint"),
        ValueRef::Real(_) => Some("double precision"),
        ValueRef::Blob(_) => Some("bytea"),
        ValueRef::Text(_) | ValueRef::Null => None,
    }
}

/// Classify what the engine refused, keeping ITS message.
pub(super) fn from_engine(error: rusqlite::Error) -> EngineError {
    // The ONE failure whose message is the BACKING STORE's own vocabulary rather than a
    // condition either wire has: a value past the ceiling the store holds one under. It is
    // reported as this endpoint's limit, in the words of the conditions beside it, so no
    // store text ever reaches a client.
    if let Some(refusal) = past_the_value_ceiling(&error) {
        return refusal;
    }
    // A statement the engine could not READ at all arrives as its own variant, carrying
    // the engine's bare message and where in the statement it stopped — which is what a
    // wire needs to word its own parse error. Classified by the failure inside it, so it
    // is not reported as an engine that broke.
    if let rusqlite::Error::SqlInputError {
        error: failure,
        msg,
        offset,
        ..
    } = &error
    {
        // The SENTENCE still decides the class where it names one: an object the statement
        // reads that does not exist is an undefined object whether the engine reported it
        // while reading the statement or while running it, and a client branches on that
        // class rather than on a parse error.
        return EngineError::new(named_class(msg).unwrap_or_else(|| failure_kind(failure)), msg.clone())
            .at_offset(usize::try_from(*offset).ok());
    }
    let message = engine_message(&error);
    let kind = match &error {
        rusqlite::Error::SqliteFailure(failure, _) => failure_kind(failure),
        rusqlite::Error::InvalidParameterCount(..) | rusqlite::Error::InvalidQuery => {
            EngineErrorKind::Syntax
        }
        _ => EngineErrorKind::Internal,
    };
    EngineError::new(named_class(&message).unwrap_or(kind), message)
}

/// The refusal a failure that is the BACKING STORE's ceiling on ONE VALUE is reported as, or
/// `None` for every other failure.
///
/// The store answers such a statement with a sentence of its own, naming its own kinds of
/// value. No client of either wire has that vocabulary, and neither engine has that limit:
/// what IS true is that this endpoint serves values up to [`MAX_VALUE_BYTES`], so the
/// condition is the one each wire carries for a limit of the implementation
/// ([`EngineErrorKind::LimitExceeded`] — PostgreSQL's `program_limit_exceeded`), worded the
/// way this endpoint words the other bounds it holds a statement to.
fn past_the_value_ceiling(error: &rusqlite::Error) -> Option<EngineError> {
    let failure = match error {
        rusqlite::Error::SqliteFailure(failure, _) => failure,
        rusqlite::Error::SqlInputError { error: failure, .. } => failure,
        _ => return None,
    };
    (failure.code == rusqlite::ErrorCode::TooBig).then(|| {
        EngineError::new(
            EngineErrorKind::LimitExceeded,
            format!(
                "the value is larger than this emulator serves ({} MiB)",
                MAX_VALUE_BYTES / (1024 * 1024)
            ),
        )
    })
}

/// The class the engine's own SENTENCE names, for the conditions whose wording says which
/// they are, or `None` where only the failure code says.
///
/// SQLite words a missing table or column as "no such <thing>", which is the one failure
/// class every client branches on separately. A configuration parameter the server does not
/// have is the same class: PostgreSQL answers it as an undefined object, and the sentence
/// comes from `current_setting` above. The exact-decimal functions (`super::pg_numeric`) each
/// carry the engine's own sentence for their condition, and the class is read back off it
/// here.
fn named_class(message: &str) -> Option<EngineErrorKind> {
    if message.contains("no such table")
        || message.contains("no such column")
        || message.contains("no such function")
        || message.contains("no such index")
        || message.contains("no such view")
        || message.starts_with("unrecognized configuration parameter")
    {
        return Some(EngineErrorKind::UndefinedObject);
    }
    match message {
        STATEMENT_TIMEOUT_CANCELLATION => Some(EngineErrorKind::Cancelled),
        NUMERIC_FIELD_OVERFLOW => Some(EngineErrorKind::NumericValueOutOfRange),
        DIVISION_BY_ZERO => Some(EngineErrorKind::DivisionByZero),
        _ => None,
    }
}

/// What PostgreSQL tells a client whose value no longer fits the precision and scale its
/// column or its cast declares, and what MySQL reports the same condition as: the sentence
/// the exact-decimal functions refuse with, which is what the class above is read off.
pub const NUMERIC_FIELD_OVERFLOW: &str = "numeric field overflow";

/// The same, for a divisor of zero.
pub const DIVISION_BY_ZERO: &str = "division by zero";

/// What one engine failure code means, in terms both wires can render.
fn failure_kind(failure: &rusqlite::ffi::Error) -> EngineErrorKind {
    match failure.code {
        rusqlite::ErrorCode::ConstraintViolation => constraint_kind(failure.extended_code),
        rusqlite::ErrorCode::OperationInterrupted => EngineErrorKind::Cancelled,
        rusqlite::ErrorCode::AuthorizationForStatementDenied => {
            EngineErrorKind::InsufficientPrivilege
        }
        rusqlite::ErrorCode::TypeMismatch => EngineErrorKind::DataError,
        rusqlite::ErrorCode::Unknown => EngineErrorKind::Syntax,
        _ => EngineErrorKind::Internal,
    }
}

/// Which constraint the engine refused the row for, from its extended result code.
fn constraint_kind(extended_code: std::os::raw::c_int) -> EngineErrorKind {
    // The extended codes SQLite documents for SQLITE_CONSTRAINT: the primary result code
    // (19) in the low byte and the constraint in the high byte.
    const SQLITE_CONSTRAINT_CHECK: std::os::raw::c_int = 275;
    const SQLITE_CONSTRAINT_FOREIGNKEY: std::os::raw::c_int = 787;
    const SQLITE_CONSTRAINT_NOTNULL: std::os::raw::c_int = 1299;
    const SQLITE_CONSTRAINT_PRIMARYKEY: std::os::raw::c_int = 1555;
    const SQLITE_CONSTRAINT_UNIQUE: std::os::raw::c_int = 2067;
    match extended_code {
        SQLITE_CONSTRAINT_CHECK => EngineErrorKind::CheckViolation,
        SQLITE_CONSTRAINT_FOREIGNKEY => EngineErrorKind::ForeignKeyViolation,
        SQLITE_CONSTRAINT_NOTNULL => EngineErrorKind::NotNullViolation,
        SQLITE_CONSTRAINT_PRIMARYKEY | SQLITE_CONSTRAINT_UNIQUE => {
            EngineErrorKind::UniqueViolation
        }
        _ => EngineErrorKind::CheckViolation,
    }
}

/// The engine's own sentence for a failure, which is what the client is told.
fn engine_message(error: &rusqlite::Error) -> String {
    match error {
        rusqlite::Error::SqliteFailure(_, Some(message)) => message.clone(),
        other => other.to_string(),
    }
}

/// What a client's SQL may do inside one DB instance's database.
///
/// DEFAULT DENY: an action not named here is refused, so an engine capability this
/// emulator has not considered — including one a later SQLite build adds, which arrives as
/// a variant this match has never seen — cannot be reached from a session. What is allowed
/// is what the data plane exists for: reading and writing the instance's own tables, the
/// DDL that defines them, transactions, and pure computation over the values in them.
///
/// What that refuses, and why it matters: `ATTACH`/`DETACH` would make a host path visible
/// to SQL (and `VACUUM INTO` is implemented by attaching its output file), `PRAGMA` reaches
/// engine configuration including temp-file paths, and a virtual table is arbitrary
/// registered code. None of them is something a PostgreSQL or MySQL client can ask for over
/// its own wire anyway, so refusing them takes nothing away from the engine this plane
/// emulates. The functions are an allow-list of their own
/// ([`::service_common::sqlite_confinement`]), which is where the host-reaching ones are
/// named absent.
fn confine_to_the_instance(context: AuthContext<'_>) -> Authorization {
    match context.action {
        AuthAction::Select
        | AuthAction::Read { .. }
        | AuthAction::Insert { .. }
        | AuthAction::Update { .. }
        | AuthAction::Delete { .. }
        | AuthAction::Transaction { .. }
        | AuthAction::Savepoint { .. }
        | AuthAction::Recursive
        | AuthAction::CreateTable { .. }
        | AuthAction::CreateTempTable { .. }
        | AuthAction::CreateIndex { .. }
        | AuthAction::CreateTempIndex { .. }
        | AuthAction::CreateView { .. }
        | AuthAction::CreateTempView { .. }
        | AuthAction::CreateTrigger { .. }
        | AuthAction::CreateTempTrigger { .. }
        | AuthAction::DropTable { .. }
        | AuthAction::DropTempTable { .. }
        | AuthAction::DropIndex { .. }
        | AuthAction::DropTempIndex { .. }
        | AuthAction::DropView { .. }
        | AuthAction::DropTempView { .. }
        | AuthAction::DropTrigger { .. }
        | AuthAction::DropTempTrigger { .. }
        | AuthAction::AlterTable { .. }
        | AuthAction::Reindex { .. }
        | AuthAction::Analyze { .. } => Authorization::Allow,
        // Pure computation over the values in those tables, as an ALLOW-LIST: a function
        // this emulator has not reviewed — including one a future engine build defines —
        // falls through to the deny arm below.
        AuthAction::Function { function_name } => {
            if ::service_common::sqlite_confinement::sql_function_is_permitted(function_name)
                || super::pg_server::is_server_function(function_name)
                || super::pg_numeric::is_engine_function(function_name)
                || super::engine_time::is_time_function(function_name)
                || super::mysql_insert_id::is_mysql_only_function(function_name)
            {
                Authorization::Allow
            } else {
                Authorization::Deny
            }
        }
        // Everything that could reach outside this instance's own database, and everything
        // this emulator has not considered.
        _ => Authorization::Deny,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A session view asking for `statement_timeout_ms` and nothing else.
    fn asking_for(statement_timeout_ms: u64) -> ServerView {
        ServerView {
            statement_timeout_ms,
            ..ServerView::default()
        }
    }

    /// RULE: the statement's deadline is the session's `statement_timeout` where that is
    /// shorter than this endpoint's own budget, and the endpoint's budget otherwise — for
    /// EVERY number a session can put on the wire, the ones no clock can represent included.
    #[test]
    fn a_statement_deadline_is_never_past_the_endpoint_budget_whatever_the_session_asks() {
        // (case, the session's statement_timeout_ms, whether the session's own value decides)
        let rows: &[(&str, u64, bool)] = &[
            ("no timeout at all, so the endpoint's budget decides", 0, false),
            ("one millisecond, far inside the endpoint's budget", 1, true),
            (
                "a value comfortably inside the endpoint's budget",
                STATEMENT_DEADLINE.as_millis() as u64 / 2,
                true,
            ),
            (
                "a value past the endpoint's budget",
                STATEMENT_DEADLINE.as_millis() as u64 * 10,
                false,
            ),
            (
                // PostgreSQL's own ceiling for this parameter: `statement_timeout` is an
                // integer number of milliseconds, so this is the largest value a session
                // can set ("statement_timeout (integer)", client-connection defaults).
                "the largest value the parameter admits",
                2_147_483_647,
                false,
            ),
            (
                "a value past what the parameter admits",
                2_147_483_648,
                false,
            ),
            ("the largest number the wire can carry", u64::MAX, false),
        ];
        for (case, asked, session_decides) in rows {
            let (deadline, timeout_first) = statement_deadline(&asking_for(*asked));
            // Read AFTER the call, so the comparison needs no slack: the deadline was
            // computed from an earlier instant than this one.
            let ceiling = Instant::now() + STATEMENT_DEADLINE;
            assert_eq!(
                timeout_first, *session_decides,
                "case {case:?}: expected the session's timeout to decide == {session_decides}, \
                 got {timeout_first}"
            );
            assert!(
                deadline <= ceiling,
                "case {case:?}: expected a deadline inside the endpoint's budget of \
                 {STATEMENT_DEADLINE:?}, got one {:?} past it",
                deadline.saturating_duration_since(ceiling)
            );
        }
    }
}
