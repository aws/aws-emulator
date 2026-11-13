// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-rdsdata-rdsdataservice service.
//!
//! The RDS Data API is an HTTP front end over a SQL database, so this state is
//! exactly that: one real SQL database per (cluster, database name) pair, run
//! in-process by SQLite through the `rusqlite` crate, plus the transactions a
//! caller has opened against a cluster. Every statement is parsed, planned and
//! executed by that engine — no code here inspects SQL text, and no result is
//! synthesised.
//!
//! ## The PostgreSQL the API advertises
//!
//! The engine behind the Data API is Aurora PostgreSQL, so a statement is parsed
//! as PostgreSQL and its results are described as PostgreSQL's catalog describes
//! them. [`postgres`] is that front end: it parses the statement, renders it in
//! the form this engine's own grammar accepts, and names the PostgreSQL type of
//! every column — which is what `columnMetadata` reports and what decides the
//! `Field` member each value comes back in. A result whose type the `Field` union
//! has no member for is refused rather than reported as something it is not.
//!
//! ## Transactions
//!
//! `BeginTransaction` takes a private copy of the committed database
//! ([`Database::snapshot`]) for the transaction to write into; statements carrying that
//! `transactionId` run against that copy, so their writes are invisible to every other
//! reader until they are committed. A transaction READS AT READ COMMITTED: the copy is
//! rebuilt from the committed database, with the transaction's own writes replayed onto
//! it ([`Transaction::rebase_onto`]), whenever another writer has committed since the
//! transaction last ran — so a statement of it sees every row committed before it ran,
//! alongside its own uncommitted rows. `CommitTransaction` installs that rebuilt copy as
//! the committed database and `RollbackTransaction` drops it, so a commit adds the
//! transaction's writes to what is committed rather than replacing it. What the replay
//! does not reproduce is in [`Transaction::rebase_onto`]; everything inside one
//! transaction, and every non-transactional statement, is the engine's own behaviour.
//!
//! ## Confining the engine
//!
//! Caller SQL is executed by a real engine, so every host primitive SQLite ships
//! with has to be taken away deliberately. [`Database::open`] is the ONLY place a
//! connection is created, and it is confined before any caller statement can
//! reach it:
//!
//! * a DEFAULT-DENY authorizer ([`confine_to_data_plane`]) — only reading and
//!   writing this database's own tables, the DDL that defines them, transactions,
//!   and the SQL functions named in [`PERMITTED_FUNCTIONS`] are allowed. The
//!   functions and the virtual-table modules are ALLOW-LISTS, not lists of
//!   refusals: a function or module a future SQLite or `rusqlite` build makes
//!   available is denied until it is reviewed and named, so nothing that reaches
//!   a file or loads code can arrive by default. `ATTACH`/`DETACH` (and so
//!   `VACUUM INTO`, which the engine implements by attaching the output file),
//!   `PRAGMA` other than the two schema-introspection ones, every other virtual
//!   table and EVERY action not named are refused, so no host path can become
//!   visible to caller SQL;
//! * `SQLITE_LIMIT_ATTACHED` = 0, so no second database can be attached even if
//!   an authorizer arm is ever widened;
//! * extension loading off, defensive mode on, untrusted schema;
//! * `max_page_count` on BOTH schemas the connection has — `main` and the `temp`
//!   schema a caller's `CREATE TEMP TABLE` writes into — plus `temp_store =
//!   MEMORY`, so a TEMP object (which lives as long as the connection, so as long
//!   as the stored database) is bounded by the same page cap as a stored table and
//!   never reaches a host temp file;
//! * `SQLITE_LIMIT_LENGTH`, `SQLITE_LIMIT_SQL_LENGTH`,
//!   `SQLITE_LIMIT_EXPR_DEPTH`, `SQLITE_LIMIT_COMPOUND_SELECT` and
//!   `SQLITE_LIMIT_VARIABLE_NUMBER`, so the engine's memory, value size,
//!   statement size, expression depth and query complexity are all capped.
//!
//! ## Bounds
//!
//! A statement arrives from a caller, so its execution is bounded the way
//! `vendor/engine/src/lib.rs` requires, on every axis:
//!
//! * STEPS: SQLite's progress handler aborts a statement every
//!   `PROGRESS_HANDLER_OPS` virtual-machine instructions once its deadline has
//!   passed (a wall-clock timeout cannot preempt a CPU-bound query), reported as
//!   `StatementTimeoutException`.
//! * TIME: [`STATEMENT_TIME_BUDGET`] is ONE deadline per REQUEST, carried by the
//!   [`RequestBudget`] every statement of a `BatchExecuteStatement` shares, so N
//!   parameter sets cannot buy N times the budget; [`in_data_plane`] runs the
//!   whole synchronous unit on the blocking pool under the outer
//!   [`REQUEST_TIME_BUDGET`], so no engine work ever occupies an async worker or
//!   holds the state lock across an await.
//! * OUTPUT: [`MAX_RESPONSE_BYTES`] is the SAME per-REQUEST budget, charged on
//!   the [`RequestBudget`] before each value is copied out of the engine — so a
//!   batch of a thousand parameter sets, each returning a large value, is refused
//!   at the request's first byte past the cap instead of allocating a thousand
//!   times it.
//! * COUNT: [`MAX_PARAMETER_SETS`] statements per call, and [`MAX_CLUSTERS`],
//!   [`MAX_DATABASES_PER_CLUSTER`] and [`MAX_OPEN_TRANSACTIONS`] stored objects,
//!   so a stream of requests naming fresh ARNs, databases or transactions cannot
//!   grow the store without limit.
//! * MEMORY: `max_page_count` bounds each schema of each database, and
//!   [`MAX_ENGINE_HEAP_BYTES`] (`PRAGMA hard_heap_limit`, read back in
//!   [`Database::open`] so a build that will not take it is refused) bounds the
//!   ENGINE's allocator itself — the aggregate, process-wide ceiling that holds
//!   however many databases and transaction snapshots are open and whatever a
//!   statement allocates internally. Past it the engine refuses the statement,
//!   reported as this emulator's capacity (503), never as the caller's fault.
//!   `the_engine_heap_ceiling_refuses_an_allocation_heavy_statement` at the foot
//!   of this file pins that.

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime};

use rusqlite::config::DbConfig;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::limits::Limit;
use rusqlite::types::{Value as SqlValue, ValueRef};
use rusqlite::Connection;

use crate::ops::types::{ArrayValue, ColumnMetadata, Field, ResultSetOptions, SqlParameter};

/// The PostgreSQL front end every statement goes through: the dialect the Data
/// API's statements are written in, and the type catalog `columnMetadata` reports
/// from.
pub mod postgres;

/// Whether IAM lets a request perform the action it asks for — the identity leg of
/// the precondition chain.
pub mod authorization;

/// The ONE precondition chain every call is admitted by: the call's own shape, the
/// identity, the cluster, the secret, the login and the database.
pub mod preconditions;

use postgres::{PgType, Plan, TypeClass};

/// A handle on this service's store, as a handler gets it from [`service_state`].
pub type Store = ::engine::ServiceState<ComAmazonawsRdsdataRdsdataserviceState>;

/// Wall clock ONE REQUEST's statements may run for before the engine aborts
/// them. Chosen here (the Data API's own statement timeout is not documented as a
/// value), which is why it is enforced as a budget and reported as the modelled
/// `StatementTimeoutException`.
pub const STATEMENT_TIME_BUDGET: Duration = Duration::from_secs(2);

/// The outer wall clock on one request's whole unit of engine work, including the
/// time it spends waiting for the state lock. Chosen here; it is the bound that
/// holds even if the engine itself never returns.
///
/// DELIBERATELY LONGER than [`STATEMENT_TIME_BUDGET`], with slack for acquiring the
/// state lock: the inner budget is the one that normally fires, and the engine's
/// progress handler frees the lock when it does. [`in_data_plane`] explains what the
/// ordering buys.
pub const REQUEST_TIME_BUDGET: Duration = Duration::from_secs(5);

// The ordering above is what bounds the lock-holding window, so it is checked at
// compile time rather than trusted.
const _: () = assert!(REQUEST_TIME_BUDGET.as_millis() > STATEMENT_TIME_BUDGET.as_millis());

/// SQLite virtual-machine instructions between two progress callbacks. Small
/// enough that the budget above is honoured promptly, large enough that the
/// callback costs nothing measurable.
const PROGRESS_HANDLER_OPS: i32 = 1_000;

/// `ExecuteStatement`'s model doc: "If the binary response data from the database
/// is more than 1 MB, the call is terminated." Checked before each value is
/// copied out of the engine.
pub const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

/// `SQLITE_LIMIT_LENGTH`: the largest single value the engine will produce. Set to
/// the response cap above, so no one value can be bigger than the whole response
/// a call may return.
const MAX_VALUE_BYTES: i32 = 1024 * 1024;

/// `SQLITE_LIMIT_SQL_LENGTH`: the model's `SqlStatement` is `len ..65536`, so a
/// statement the model would refuse is also one the engine will not parse.
const MAX_SQL_BYTES: i32 = 65_536;

/// `SQLITE_LIMIT_EXPR_DEPTH` / `SQLITE_LIMIT_COMPOUND_SELECT` /
/// `SQLITE_LIMIT_VARIABLE_NUMBER`: complexity bounds on one statement, well above
/// anything a client writes by hand and far below what exhausts the parser.
const MAX_EXPRESSION_DEPTH: i32 = 200;
pub(crate) const MAX_COMPOUND_SELECT: i32 = 20;
const MAX_BOUND_PARAMETERS: i32 = 1_000;

/// How many values one call's `parameters` list (and each entry of its
/// `parameterSets`) may carry.
///
/// The model constrains neither list's length, and the length is the caller's to choose,
/// so it is capped here — at the engine's OWN variable limit
/// ([`MAX_BOUND_PARAMETERS`]), because a statement cannot bind more placeholders than
/// that whatever the request carries. The cap is applied BEFORE the list is scanned, so a
/// long list cannot buy work.
pub const MAX_PARAMETERS_PER_STATEMENT: usize = MAX_BOUND_PARAMETERS as usize;

/// `SQLITE_LIMIT_LIKE_PATTERN_LENGTH`: the longest `LIKE`/`GLOB` pattern one
/// comparison may carry.
///
/// The engine's documented bound on the one operation its instruction counter
/// cannot preempt: a pattern comparison is a single virtual-machine instruction,
/// and the matcher's cost grows with the pattern, so the engine's own limit on
/// pattern length is what bounds the longest stretch a statement can spend
/// between two points where [`Database::run_within`]'s deadline is checked. Set
/// far below the engine's own 50 000-byte default and well above any pattern a
/// client writes (a `LIKE` a person or an ORM writes is a few dozen bytes).
const MAX_LIKE_PATTERN_BYTES: i32 = 1_024;

/// `max_page_count`: 64 MiB per database at SQLite's 4 KiB default page size. The
/// engine answers `SQLITE_FULL` past it, so the rows a caller inserts cannot grow
/// the process without limit. Applied to EVERY schema the connection has — see
/// [`Database::open`].
const MAX_DATABASE_PAGES: i64 = 16_384;

/// `PRAGMA hard_heap_limit`: the ceiling on what the SQL ENGINE's own allocator may
/// hold. The page caps above bound each schema of each database, and the request
/// budget bounds steps, time and output; this bounds MEMORY itself, which is a
/// separate axis — it is the only bound that covers an engine-INTERNAL allocation
/// (pager and sorter scratch, a statement's working set) and the only one that
/// holds across every database and transaction snapshot open at once. SQLite's heap
/// limit is a PROCESS-WIDE setting, so this is the aggregate ceiling for every
/// database this emulator runs rather than a per-database one, and a statement that
/// would take the engine past it is failed by the engine itself with
/// `SQLITE_NOMEM`, reported as the modelled failure like any other refusal.
///
/// The same value as the sibling confined data plane in
/// `services/service-com-amazonaws-redshiftdata-redshiftdata`, so one process
/// running both services has ONE ceiling rather than two that disagree.
const MAX_ENGINE_HEAP_BYTES: i64 = 256 * 1024 * 1024;

/// The schemas one connection has, each of which is a SEPARATE database with its
/// own page limit: `main` holds the caller's tables, and `temp` is where SQLite
/// puts a `CREATE TEMP TABLE`/`INDEX`/`VIEW`/`TRIGGER` and its own transient
/// materialisations. Both are capped in [`Database::open`]; `temp` is not
/// governed by `main`'s limit.
const CAPPED_SCHEMAS: &[&str] = &["main", "temp"];

/// `temp_store = MEMORY` (SQLite's documented value 2 for that pragma): the `temp`
/// schema and every spill the engine makes stay in this process, whatever the
/// build's `SQLITE_TEMP_STORE` default is, so no statement a caller sends creates
/// a file in the host's temp directory. Set here because the authorizer refuses
/// `PRAGMA temp_store` to caller SQL, so this is the only place it can be chosen.
const TEMP_STORE_MEMORY: i64 = 2;

/// How many statements one `BatchExecuteStatement` may run. The model constrains
/// neither `parameterSets`'s length nor its total size, so the count is capped
/// here; the whole batch also shares one [`STATEMENT_TIME_BUDGET`].
pub const MAX_PARAMETER_SETS: usize = 1_000;

/// The savepoint one `BatchExecuteStatement` runs inside, so the engine itself is what
/// undoes a refused batch: opened before its first statement, released when every
/// statement has run, and rolled back to when one of them fails.
const BATCH_SAVEPOINT_BEGIN: &str = "SAVEPOINT rds_data_batch";
const BATCH_SAVEPOINT_RELEASE: &str = "RELEASE rds_data_batch";
const BATCH_SAVEPOINT_ROLLBACK: &str = "ROLLBACK TO rds_data_batch; RELEASE rds_data_batch";

/// How many clusters, databases per cluster and open transactions per cluster this
/// emulator stores at once. Each is a store a caller can grow by naming a fresh
/// ARN, database or transaction, so each is capped; a request past the cap is
/// refused rather than served by evicting another caller's data.
pub const MAX_CLUSTERS: usize = 16;
pub const MAX_DATABASES_PER_CLUSTER: usize = 16;
pub const MAX_OPEN_TRANSACTIONS: usize = 32;

/// How many statements ONE open transaction may have changed its database with.
///
/// A transaction keeps the statements that CHANGED its database until it ends, because
/// that is what is replayed when the committed database moves under it and what a
/// commit installs on top of that database ([`Transaction::rebase_onto`]). So the list
/// is a store one caller can grow by running statements, and it is capped like every
/// other store here — above the statements a whole [`MAX_PARAMETER_SETS`] batch leaves,
/// so a batch at ITS cap fits in a transaction, and a statement past this one is
/// refused rather than running and then being forgotten.
pub const MAX_TRANSACTION_WRITES: usize = 2 * MAX_PARAMETER_SETS;

/// The widest fixed-point scale a value is ever formatted at. A value the engine
/// holds is a 64-bit float, which carries ~17 significant decimal digits, so a
/// declared scale beyond this adds no information — and a declaration like
/// `DECIMAL(1,2000000000)` must not be able to ask for a two-gigabyte string.
const MAX_DECIMAL_SCALE: i32 = 38;

/// The largest value reported for a declared type's arguments (`VARCHAR(n)`'s
/// length, `DECIMAL(p,s)`'s precision and scale). Caps what a declaration can put
/// into `columnMetadata` and into any arithmetic over it.
pub(crate) const MAX_DECLARED_ARGUMENT: i32 = 65_535;

/// The only pragmas caller SQL — and this module's own schema introspection — may
/// run: `table_info`/`table_xinfo` report the columns a table declares, which is
/// where `columnMetadata`'s types and nullability come from. They read this
/// database's own schema and nothing else; every other pragma (temp-file paths,
/// journal and schema-writing switches) is refused. The same two names, prefixed
/// `pragma_`, are the ONLY virtual-table modules the authorizer admits, so one
/// rule governs both the statement form (`PRAGMA table_info(t)`) and the
/// table-valued form (`pragma_table_info(t)`) this module itself queries.
const SCHEMA_INTROSPECTION_PRAGMAS: &[&str] = &["table_info", "table_xinfo"];

/// Every SQL function caller SQL may call — an ALLOW-LIST, so a function a future
/// SQLite or `rusqlite` build defines is refused until it is reviewed and named
/// here, rather than allowed because no one added it to a list of refusals.
///
/// The names are SQLite's own documented library functions: the core scalar
/// functions (<https://sqlite.org/lang_corefunc.html>), the date and time
/// functions (<https://sqlite.org/lang_datefunc.html>) and the three date and
/// time KEYWORDS the engine resolves as functions of the same names
/// (<https://sqlite.org/lang_expr.html>), the aggregate functions
/// (<https://sqlite.org/lang_aggfunc.html>), the built-in window functions
/// (<https://sqlite.org/windowfunctions.html>), the math functions
/// (<https://sqlite.org/lang_mathfunc.html>) and the JSON functions
/// (<https://sqlite.org/json1.html>) — every one of which computes over the
/// values in this database, or over the clock, and nothing else.
///
/// The ONE function beside them is this data plane's own reader of a binary value's
/// input format (`postgres::binary_input_reader`), which [`Database::open`] defines
/// and the authorizer admits by that same name.
///
/// DELIBERATELY ABSENT, so they are denied by the default arm: `load_extension`
/// (the engine's own code loader), the `sqlite3` shell's file functions
/// (`readfile`, `writefile`, `edit`, `fsdir`), `sqlite_dbpage` and
/// `fts3_tokenizer`, and the build-reporting functions (`sqlite_version`,
/// `sqlite_source_id`, `sqlite_compileoption_get`, `sqlite_compileoption_used`,
/// `sqlite_offset`) — none of which a Data API client needs.
const PERMITTED_FUNCTIONS: &[&str] = &[
    // Core scalar functions.
    "abs",
    "changes",
    "char",
    "coalesce",
    "concat",
    "concat_ws",
    "format",
    "glob",
    "hex",
    "iif",
    "ifnull",
    "instr",
    "last_insert_rowid",
    "length",
    "like",
    "likelihood",
    "likely",
    "lower",
    "ltrim",
    "max",
    "min",
    "nullif",
    "octet_length",
    "printf",
    "quote",
    "random",
    "randomblob",
    "replace",
    "round",
    "rtrim",
    "sign",
    "soundex",
    "substr",
    "substring",
    "total_changes",
    "trim",
    "typeof",
    "unhex",
    "unicode",
    "unlikely",
    "upper",
    "zeroblob",
    // Date and time functions.
    "current_date",
    "current_time",
    "current_timestamp",
    "date",
    "datetime",
    "julianday",
    "strftime",
    "time",
    "timediff",
    "unixepoch",
    // Aggregate functions (`max`/`min`/`count` double as scalars above).
    "avg",
    "count",
    "group_concat",
    "string_agg",
    "sum",
    "total",
    // Built-in window functions.
    "cume_dist",
    "dense_rank",
    "first_value",
    "lag",
    "last_value",
    "lead",
    "nth_value",
    "ntile",
    "percent_rank",
    "rank",
    "row_number",
    // Math functions.
    "acos",
    "acosh",
    "asin",
    "asinh",
    "atan",
    "atan2",
    "atanh",
    "ceil",
    "ceiling",
    "cos",
    "cosh",
    "degrees",
    "exp",
    "floor",
    "ln",
    "log",
    "log10",
    "log2",
    "mod",
    "pi",
    "pow",
    "power",
    "radians",
    "sin",
    "sinh",
    "sqrt",
    "tan",
    "tanh",
    "trunc",
    // JSON functions, including the `->` and `->>` operators SQLite implements as
    // functions of those names.
    "->",
    "->>",
    "json",
    "json_array",
    "json_array_length",
    "json_error_position",
    "json_extract",
    "json_group_array",
    "json_group_object",
    "json_insert",
    "json_object",
    "json_patch",
    "json_pretty",
    "json_quote",
    "json_remove",
    "json_replace",
    "json_set",
    "json_type",
    "json_valid",
];

/// `BeginTransaction`'s model doc: "A transaction times out if no calls use its
/// transaction ID in three minutes. If a transaction times out before it's
/// committed, it's rolled back automatically."
pub const TRANSACTION_IDLE_TIMEOUT: Duration = Duration::from_secs(3 * 60);

/// `BeginTransaction`'s model doc: "A transaction can run for a maximum of 24
/// hours. A transaction is terminated and rolled back automatically after 24
/// hours."
pub const TRANSACTION_MAX_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);

/// How long a TERMINAL transaction's id is remembered.
///
/// A transaction that ended is kept so that a later call quoting its id is answered for a
/// transaction that ended; what the caller is told is the same either way, so forgetting
/// one changes nothing a caller can observe — which is why the window is the lifetime a
/// live transaction has at most ([`TRANSACTION_MAX_LIFETIME`]) rather than for ever, and
/// a cluster keeps at most [`MAX_TERMINAL_TRANSACTIONS`] of them.
pub const TERMINAL_TRANSACTION_MEMORY: Duration = TRANSACTION_MAX_LIFETIME;

/// How many ended transactions one cluster remembers at once. A terminal record holds no
/// database — only its id and when it ended — but a caller opening and committing
/// transactions in a loop would still grow the store without limit, so the OLDEST ended
/// record is dropped first and the ones a live workflow just ended stay.
pub const MAX_TERMINAL_TRANSACTIONS: usize = 64;

/// What the service tells a caller whose `transactionId` names a transaction a previous
/// CALL ended — a commit, a rollback, or a statement of its own that failed — one that
/// reached the maximum lifetime a transaction may run for, and an id this resource could
/// have issued and did not. A transaction the service abandoned for INACTIVITY carries
/// [`TRANSACTION_IDLE_MESSAGE`] instead. An id that could NOT have been issued for the
/// resource the call names is a fault of the call's own shape, refused before any
/// transaction is looked up.
///
/// Entirely fixed text: it carries no value from the request and none from state, so a
/// refusal says nothing about which ids this service has issued.
pub const TRANSACTION_NOT_RUNNING_MESSAGE: &str =
    "INVALID_ARGUMENT: Transaction error: one previous call resulted in a termination of \
     your transaction";

/// What the service tells a caller whose `transactionId` names a transaction the service
/// itself ABANDONED because no call used its id for [`TRANSACTION_IDLE_TIMEOUT`]: the
/// transaction aged out while idle and its writes were rolled back, which is a different
/// thing to tell a client than a transaction a previous call ended — and the only way to
/// tell them apart, since both carry `TransactionNotFoundException`.
///
/// Entirely fixed text, like [`TRANSACTION_NOT_RUNNING_MESSAGE`]: it carries no value
/// from the request and none from state.
pub const TRANSACTION_IDLE_MESSAGE: &str =
    "INVALID_ARGUMENT: Transaction error: transaction has been terminated due to an idleness";

/// Why a transaction is no longer RUNNING — the one thing that distinguishes the refusals
/// of its `transactionId`, which all carry `TransactionNotFoundException`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionEnd {
    /// It was terminated: a commit installed its writes, a rollback discarded them, a
    /// statement of its own failed and one of those two then ended it, or it reached the
    /// maximum lifetime a transaction may run for. An id no transaction of this resource
    /// ever had is answered the same way, because what the service tells a caller is only
    /// that no transaction of that id is running.
    Terminated,
    /// The service ABANDONED it for inactivity: no call used its id within the window
    /// [`TRANSACTION_IDLE_TIMEOUT`] measures, so the transaction was rolled back on its
    /// own.
    Idle,
}

impl TransactionEnd {
    /// The sentence a refusal of this transaction's id carries.
    #[must_use]
    pub fn sentence(self) -> &'static str {
        match self {
            Self::Terminated => TRANSACTION_NOT_RUNNING_MESSAGE,
            Self::Idle => TRANSACTION_IDLE_MESSAGE,
        }
    }
}

/// The `transactionStatus` a commit reports — the one it installed a transaction's
/// writes with, and the one a COMMIT of a transaction that a statement of its own had
/// already failed in reports too, because what the call reports is the operation the
/// CALLER asked for (see `CommitTransaction`, which discards those writes).
pub const TRANSACTION_COMMITTED_STATUS: &str = "Transaction Committed";

/// The `transactionStatus` a rollback reports.
pub const ROLLBACK_COMPLETE_STATUS: &str = "Rollback complete";

/// Run one request's unit of engine work.
///
/// The work is synchronous and CPU-bound (it drives the SQL engine and holds the
/// state lock), so it runs on the blocking pool instead of an async worker, under
/// an outer wall clock. Nothing holds the state lock across an await point, and a
/// request that somehow outlives [`REQUEST_TIME_BUDGET`] still answers.
///
/// When the OUTER clock is what fires, this answers `Timeout` while the blocking
/// closure is still running and still holding the state lock — `spawn_blocking` has
/// no cancellation. That window is bounded, not open: the statement inside carries
/// the strictly shorter [`STATEMENT_TIME_BUDGET`], which the engine's own progress
/// handler enforces against even a CPU-bound query, so the closure returns and
/// releases the lock. The outer clock exists only for the time that is NOT the
/// engine's (waiting for the lock, or an engine that never calls back at all).
pub async fn in_data_plane<T, F>(work: F) -> Result<T, SqlError>
where
    F: FnOnce() -> Result<T, SqlError> + Send + 'static,
    T: Send + 'static,
{
    match tokio::time::timeout(REQUEST_TIME_BUDGET, tokio::task::spawn_blocking(work)).await {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(joined)) => Err(SqlError::Unavailable(format!(
            "the data plane did not finish this request: {joined}"
        ))),
        Err(_elapsed) => Err(SqlError::Timeout),
    }
}

/// `java.sql.ResultSetMetaData.columnNoNulls` — the column rejects NULL.
const COLUMN_NO_NULLS: i32 = 0;
/// `java.sql.ResultSetMetaData.columnNullable` — the column accepts NULL.
///
/// What a result column reports unless a declaration FORBIDS a NULL in it: a
/// column no declaration constrains — a computed one, or one whose declaration
/// the schema cannot settle — can carry a NULL, which is what this says.
const COLUMN_NULLABLE: i32 = 1;

/// Per-service state for com-amazonaws-rdsdata-rdsdataservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsRdsdataRdsdataserviceState {
    /// Keyed by the `resourceArn` the caller addresses.
    pub clusters: HashMap<String, Cluster>,
}

impl ComAmazonawsRdsdataRdsdataserviceState {
    /// The cluster `resource_arn` names, created empty on first use: the emulator
    /// has no RDS control plane, so any cluster ARN a caller addresses is served —
    /// up to [`MAX_CLUSTERS`] of them, past which a request for a NEW cluster is
    /// refused rather than growing the store without limit.
    ///
    /// Admitting a NEW cluster first forgets the ones that naming alone brought
    /// into being ([`Self::forget_what_naming_created`]), so a caller cannot take a
    /// capped slot away from a later one just by addressing an ARN.
    pub fn cluster_mut(&mut self, resource_arn: &str) -> Result<&mut Cluster, SqlError> {
        if !self.clusters.contains_key(resource_arn) {
            self.forget_what_naming_created();
            if self.clusters.len() >= MAX_CLUSTERS {
                return Err(SqlError::Capacity(format!(
                    "this emulator serves at most {MAX_CLUSTERS} clusters at a time"
                )));
            }
        }
        Ok(self.clusters.entry(resource_arn.to_string()).or_default())
    }

    /// Forget every cluster and database that holds nothing: a database with no
    /// schema of its own and a cluster with no such database and no open
    /// transaction.
    ///
    /// Naming a cluster or a database is what creates it here, so a request that
    /// only named one — a read of an address never written, or a statement the
    /// engine refused — would otherwise hold one of the capped slots for the life
    /// of the process. An empty database is indistinguishable from one that was
    /// never created (every read of it answers the same either way), so dropping it
    /// changes nothing a caller can observe; what a caller did create keeps its
    /// slot, which is why the sweep runs only when a new name is asking for one.
    pub fn forget_what_naming_created(&mut self) {
        self.clusters.retain(|_, cluster| {
            cluster.forget_empty_databases();
            !cluster.databases.is_empty() || !cluster.transactions.is_empty()
        });
    }
}

/// One Aurora Serverless cluster: its databases and the transactions open on it.
#[derive(Debug, Default)]
pub struct Cluster {
    /// Keyed by database name; `""` is the connection's default database (the
    /// request omitted `database`).
    pub databases: HashMap<String, Database>,
    /// Keyed by the `transactionId` [`Transaction::open`] minted.
    pub transactions: HashMap<String, Transaction>,
}

impl Cluster {
    /// The committed database `name` holds, created empty on first use — up to
    /// [`MAX_DATABASES_PER_CLUSTER`], past which a request for a NEW database is
    /// refused. Admitting a new one first forgets the databases that naming alone
    /// brought into being, for the reason [`Database::is_empty`] gives.
    pub fn database_mut(&mut self, name: &str) -> Result<&mut Database, SqlError> {
        if !self.databases.contains_key(name) {
            self.forget_empty_databases();
            if self.databases.len() >= MAX_DATABASES_PER_CLUSTER {
                return Err(SqlError::Capacity(format!(
                    "this emulator serves at most {MAX_DATABASES_PER_CLUSTER} databases per cluster"
                )));
            }
            self.databases.insert(name.to_string(), Database::open()?);
        }
        Ok(self
            .databases
            .get_mut(name)
            .expect("the database was just inserted"))
    }

    /// Drop every committed database that holds no schema of its own — one a
    /// request created by naming it and left empty. A transaction's own copy is
    /// not touched: it lives on the [`Transaction`], and its commit re-installs
    /// the database under its name.
    pub fn forget_empty_databases(&mut self) {
        self.databases.retain(|_, database| !database.is_empty());
    }

    /// Roll back every transaction the clock has already ended: one idle for
    /// [`TRANSACTION_IDLE_TIMEOUT`], and one that has been open for
    /// [`TRANSACTION_MAX_LIFETIME`]. Both are owed at the instant they fall due
    /// whether or not anybody calls, so materialising them here is reporting what
    /// the clock did, and a later call quoting that `transactionId` is answered with
    /// `TransactionNotFoundException` — carrying the sentence for WHY the transaction
    /// stopped running ([`TransactionEnd`]), which is what tells a client a transaction
    /// that aged out while idle from one a previous call ended.
    ///
    /// The clock's rollback is a rollback: the copy the transaction was writing into is
    /// dropped, so none of its writes reach the committed database, and the marks its
    /// sequences handed out are carried into that database first — for the reason
    /// [`Database::advance_sequences`] gives, which does not depend on who ended the
    /// transaction.
    ///
    /// A transaction that ended is FORGOTTEN once [`TERMINAL_TRANSACTION_MEMORY`] has
    /// passed, and the oldest ended record goes first past
    /// [`MAX_TERMINAL_TRANSACTIONS`]: a forgotten terminal id is answered as one the
    /// service never issued, which is the same refusal.
    pub fn expire_timed_out_transactions(&mut self) {
        // What each transaction the clock just ended had taken out of a sequence, by the
        // database it was opened for: read while its copy is still here, carried over
        // once the loop has let go of it.
        let mut carried: Vec<(String, Vec<(String, i64)>)> = Vec::new();
        for transaction in self.transactions.values_mut() {
            if let Some(pending) = transaction.end_if_the_clock_has() {
                carried.push((
                    database_key(transaction.database.as_ref()),
                    pending.sequence_marks(),
                ));
            }
        }
        for (database, marks) in carried {
            // Only into a database this cluster still has: the clock's rollback creates
            // nothing.
            if let Some(committed) = self.databases.get(&database) {
                committed.advance_sequences(&marks);
            }
        }
        self.transactions
            .retain(|_, transaction| !transaction.is_forgotten());
        while self.terminal_transaction_count() > MAX_TERMINAL_TRANSACTIONS {
            let Some(oldest) = self
                .transactions
                .values()
                .filter_map(|transaction| {
                    transaction
                        .ended_at()
                        .map(|at| (at, transaction.id.clone()))
                })
                .min()
                .map(|(_, id)| id)
            else {
                break;
            };
            self.transactions.remove(&oldest);
        }
    }

    /// How many transactions of this cluster are still open — the ones that hold a copy
    /// of a database, which is what [`MAX_OPEN_TRANSACTIONS`] bounds.
    pub fn open_transaction_count(&self) -> usize {
        self.transactions
            .values()
            .filter(|transaction| !transaction.is_terminal())
            .count()
    }

    /// How many ended transactions this cluster still remembers.
    fn terminal_transaction_count(&self) -> usize {
        self.transactions
            .values()
            .filter(|transaction| transaction.is_terminal())
            .count()
    }

    /// The OPEN transaction `transaction_id` names.
    ///
    /// `Err(SqlError::TransactionNotFound)` for an id this cluster has no open
    /// transaction for — one it never issued, one the clock has rolled back, and one that
    /// has ENDED (committed, rolled back, or terminated by a statement of its own that
    /// failed) — carrying WHY that transaction is no longer running, which is the one
    /// thing the refusal of its id says about it.
    pub fn open_transaction_mut(
        &mut self,
        transaction_id: &str,
    ) -> Result<&mut Transaction, SqlError> {
        let transaction = self
            .transactions
            .get_mut(transaction_id)
            .ok_or(SqlError::TransactionNotFound(TransactionEnd::Terminated))?;
        if let Some(end) = transaction.end_cause() {
            return Err(SqlError::TransactionNotFound(end));
        }
        Ok(transaction)
    }

    /// End the transaction `transaction_id` names and hand back the database it was
    /// writing into. The id stays with the cluster, now terminal.
    pub fn end_transaction(&mut self, transaction_id: &str) -> Result<Database, SqlError> {
        self.transactions
            .get_mut(transaction_id)
            .ok_or(SqlError::TransactionNotFound(TransactionEnd::Terminated))?
            .end()
    }

    /// Record that a statement of the transaction `transaction_id` names FAILED, so the
    /// database ignores every further command of it until a commit or a rollback ends it.
    /// A no-op for an id this cluster holds no transaction for.
    pub fn abort_transaction(&mut self, transaction_id: &str) {
        if let Some(transaction) = self.transactions.get_mut(transaction_id) {
            transaction.abort();
        }
    }

    /// Run one statement inside the transaction `transaction_id` names, against the
    /// database that transaction READS: everything committed on its database now, plus
    /// its own writes ([`Transaction::rebase_onto`]).
    pub fn run_in_transaction(
        &mut self,
        transaction_id: &str,
        budget: &mut RequestBudget,
        sql: &str,
        parameters: &[SqlParameter],
    ) -> Result<StatementOutcome, SqlError> {
        self.read_what_is_committed(transaction_id, budget)?;
        let transaction = self.open_transaction_mut(transaction_id)?;
        transaction.touch();
        transaction.require_room_for(1)?;
        let outcome = transaction
            .open_database()?
            .run_within(budget, sql, parameters)?;
        transaction.record(sql, parameters, &outcome);
        Ok(outcome)
    }

    /// Run one statement once per parameter set inside the transaction
    /// `transaction_id` names, as ONE unit of work, against the same database its
    /// single statements run against.
    pub fn run_batch_in_transaction(
        &mut self,
        transaction_id: &str,
        budget: &mut RequestBudget,
        sql: &str,
        parameter_sets: &[Vec<SqlParameter>],
    ) -> Result<Vec<StatementOutcome>, SqlError> {
        self.read_what_is_committed(transaction_id, budget)?;
        let transaction = self.open_transaction_mut(transaction_id)?;
        transaction.touch();
        transaction.require_room_for(parameter_sets.len())?;
        let outcomes = transaction
            .open_database()?
            .run_batch_within(budget, sql, parameter_sets)?;
        for (parameters, outcome) in parameter_sets.iter().zip(&outcomes) {
            transaction.record(sql, parameters, outcome);
        }
        Ok(outcomes)
    }

    /// Put the transaction `transaction_id` names back on top of what is COMMITTED on
    /// its database now, where the committed database has moved since it last ran.
    ///
    /// This is what gives a transaction READ COMMITTED visibility: its next statement —
    /// and the commit that installs its writes — read every row another transaction
    /// committed in the meantime. Nothing to do for a transaction that has ended, one a
    /// statement of its own already failed in (its writes can no longer reach the
    /// committed database), one whose database the store no longer holds, and one whose
    /// copy is already built from what is committed.
    pub fn read_what_is_committed(
        &mut self,
        transaction_id: &str,
        budget: &mut RequestBudget,
    ) -> Result<(), SqlError> {
        let Some(open) = self
            .transactions
            .get(transaction_id)
            .filter(|transaction| !transaction.is_terminal() && !transaction.is_aborted())
        else {
            return Ok(());
        };
        let name = database_key(open.database.as_ref());
        // Disjoint fields of this cluster: the committed database is read while the
        // transaction that rebases onto it is written.
        let Some(committed) = self.databases.get(&name) else {
            return Ok(());
        };
        let Some(transaction) = self.transactions.get_mut(transaction_id) else {
            return Ok(());
        };
        if transaction.reads_version(committed.version()) {
            return Ok(());
        }
        transaction.rebase_onto(committed, budget)
    }

    /// Apply a statement about THIS CLUSTER'S DATABASES and report what the engine
    /// reports for it: a statement that defines or drops a database changes no rows and
    /// returns no result set.
    ///
    /// Each database of a cluster is its own engine here, so no engine behind one of
    /// them can create or drop another; and such a statement is DDL, which the engines
    /// behind this API apply outside any transaction block, so it is applied to the
    /// cluster at once whether or not the call quoted a `transactionId`.
    pub fn apply_namespace(
        &mut self,
        statement: &postgres::NamespaceStatement,
    ) -> Result<StatementOutcome, SqlError> {
        match statement {
            postgres::NamespaceStatement::Create {
                name,
                if_not_exists,
            } => {
                if self.databases.contains_key(name) {
                    if *if_not_exists {
                        return Ok(StatementOutcome::nothing());
                    }
                    return Err(SqlError::Engine(postgres::database_already_exists(name)));
                }
                self.database_mut(name)?.declare();
            }
            postgres::NamespaceStatement::Drop { name, if_exists } => {
                if self.databases.remove(name).is_none() && !*if_exists {
                    return Err(SqlError::Engine(postgres::database_does_not_exist(name)));
                }
            }
        }
        Ok(StatementOutcome::nothing())
    }
}

/// The name a request's optional `database` member selects. `""` stands for the
/// secret's default database, so a caller that never names one still gets a
/// single stable database rather than a new one per call.
pub fn database_key(database: Option<&String>) -> String {
    database.cloned().unwrap_or_default()
}

/// The columns of the table `table` an UPSERT conflicts on: the key a duplicate-key
/// condition is raised against, and empty for a table whose declaration holds no key at
/// all — which can raise no such condition.
///
/// MySQL's `ON DUPLICATE KEY UPDATE` conflicts on ANY unique key of the table, while the
/// engine behind this data plane spells an upsert with the conflicting key NAMED, so the
/// key has to be read off the table itself. Both halves of it are:
///
/// * the PRIMARY KEY, from the engine's own schema (`pragma_table_info`, whose `pk` column
///   is a column's one-based position in the primary key and 0 for a column outside it) —
///   the same introspection [`Database::declared_column`] reads a declaration through, and
///   the only one the engine's authorizer admits;
/// * failing that, the first UNIQUE key the table's own DECLARATION carries, read out of
///   the engine's schema and parsed ([`postgres::unique_key_columns`]) rather than
///   inspected as text.
///
/// ONE key is reported, because the engine spells an upsert with ONE conflict target:
/// where a table carries a primary key AND a further unique key, an insert that duplicates
/// the second is answered by the engine's own constraint rather than updating the stored
/// row. Both lookups match the table's name without regard to case, as every other
/// introspection here does ([`Database::declared_column`]).
fn conflict_key_columns(conn: &Connection, table: &str) -> Vec<String> {
    let primary_key = query_column(
        conn,
        "SELECT name FROM pragma_table_info(?1) WHERE pk > 0 ORDER BY pk",
        table,
    );
    if !primary_key.is_empty() {
        return primary_key;
    }
    query_column(
        conn,
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1 COLLATE NOCASE",
        table,
    )
    .first()
    .map(|declaration| postgres::unique_key_columns(declaration))
    .unwrap_or_default()
}

/// The first column of every row `query` answers for `argument`, and empty where the
/// engine would not answer it at all — a schema this cannot read decides nothing, exactly
/// as [`Database::declared_column`] treats one.
fn query_column(conn: &Connection, query: &str, argument: &str) -> Vec<String> {
    let Ok(mut statement) = conn.prepare(query) else {
        return Vec::new();
    };
    let Ok(rows) = statement.query_map([argument], |row| row.get::<_, String>(0)) else {
        return Vec::new();
    };
    rows.filter_map(Result::ok).collect()
}

/// The type the schema declares for a column named `column`, when it settles ONE: the
/// type an aggregate over that column is exact or inexact by.
///
/// Read from the engine's own schema, and answered only where every table that
/// declares such a column declares it the same way — a name two tables type
/// differently settles nothing, exactly as [`Database::declared_column`] treats one.
fn declared_column_type(conn: &Connection, column: &str) -> Option<String> {
    let declared = query_column(
        conn,
        "SELECT DISTINCT info.type FROM sqlite_master AS m JOIN pragma_table_info(m.name) AS info \
         WHERE m.type = 'table' AND info.name = ?1 COLLATE NOCASE AND info.type <> ''",
        column,
    );
    match declared.as_slice() {
        [only] => Some(only.clone()),
        _ => None,
    }
}

/// The type ONE table's declaration gives one of its columns, read from the engine's
/// own schema. `None` where the engine holds no such column, whose type nothing here
/// decides.
fn declared_type_in(conn: &Connection, table: &str, column: &str) -> Option<String> {
    let Ok(mut statement) = conn.prepare(
        "SELECT info.type FROM sqlite_master AS m JOIN pragma_table_info(m.name) AS info \
         WHERE m.type = 'table' AND m.name = ?1 COLLATE NOCASE \
           AND info.name = ?2 COLLATE NOCASE AND info.type <> ''",
    ) else {
        return None;
    };
    statement
        .query_row(rusqlite::params![table, column], |row| {
            row.get::<_, String>(0)
        })
        .ok()
}

/// The bytes a binary value's TEXT spells, in the input format PostgreSQL documents
/// for `bytea`: its "hex format" — a leading `\x`, then two hexadecimal digits per
/// byte.
///
/// Text in no such format is the bytes of the text itself, which is what the engine's
/// own cast to its binary type gives, and a number is the bytes of its digits, for the
/// same reason. Decoded by the `hex` crate, so the codec is one implementation and not
/// a loop written here.
fn binary_input(value: ValueRef<'_>) -> SqlValue {
    let text = match value {
        ValueRef::Null => return SqlValue::Null,
        ValueRef::Blob(bytes) => return SqlValue::Blob(bytes.to_vec()),
        ValueRef::Text(bytes) => bytes.to_vec(),
        ValueRef::Integer(number) => number.to_string().into_bytes(),
        ValueRef::Real(number) => number.to_string().into_bytes(),
    };
    let decoded = text
        .strip_prefix(BYTEA_HEX_PREFIX)
        .and_then(|digits| hex::decode(digits).ok());
    SqlValue::Blob(decoded.unwrap_or(text))
}

/// What PostgreSQL's hex input format for a binary value opens with.
const BYTEA_HEX_PREFIX: &[u8] = br"\x";

/// One SQL database, run by SQLite in this process.
#[derive(Debug)]
pub struct Database {
    conn: Connection,
    /// What the declaration gave each sequence-carrying column of this database,
    /// keyed by (table, column), both lower-cased.
    ///
    /// A `serial` or `AUTO_INCREMENT` column is declared to the engine as the integer
    /// key it is, so the engine assigns its values; what the engine then reports for
    /// it is `INTEGER`, and this is what remembers what the DECLARATION said, which
    /// decides the type `columnMetadata` reports and whether the assigned value is one
    /// `generatedFields` carries. Replaced wholesale when a table is created and
    /// dropped with the table.
    sequence_columns: HashMap<(String, String), DeclaredSequence>,
    /// The version this database's CONTENT is at: a ticket from [`next_version`],
    /// taken afresh every time a statement changes it.
    ///
    /// What a transaction compares to tell whether the committed database it reads has
    /// MOVED since it last ran — which is what READ COMMITTED needs and a counter of
    /// this database's own would not give, because a commit replaces one `Database`
    /// with another.
    version: u64,
    /// Whether a `CREATE DATABASE` brought this database into being, as opposed to a
    /// statement that merely named it: one a caller created EXISTS even while it holds
    /// nothing, so it is not swept away as a database naming alone created
    /// ([`Self::is_empty`]).
    declared: bool,
}

/// What a table's declaration said about a column whose values the ENGINE assigns.
#[derive(Debug, Clone)]
struct DeclaredSequence {
    /// The catalog name of the declared type (`serial`, `int4`).
    type_name: String,
    /// Whether the value the engine assigns is one `generatedFields` reports.
    reports_generated_key: bool,
}

/// A ticket from the ONE counter every change to a database here takes its version
/// from.
///
/// Process-wide rather than per database, because a commit INSTALLS a transaction's own
/// copy as the committed database: a version read from a counter the database object
/// keeps would start again with it, and another transaction comparing versions would
/// not see that the database it reads had moved.
fn next_version() -> u64 {
    static CHANGES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    CHANGES.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl Database {
    /// Whether this database holds nothing of its own: no table, view, index or
    /// trigger in EITHER of its schemas — the stored one and the connection-local
    /// `temp` one, where a caller's `CREATE TEMP TABLE` lives for as long as this
    /// database does. A database that holds neither answers every statement a
    /// caller could run exactly as a database that was never created would, which
    /// is what lets the store drop it instead of charging it to a cap. Read from
    /// the engine's own schemas, never from a count this module keeps.
    ///
    /// A database the engine cannot be asked (a connection that has failed) is
    /// reported as NOT empty, so a sweep never discards state it could not read.
    pub fn is_empty(&self) -> bool {
        !self.declared
            && self
                .conn
                .query_row(
                    "SELECT (SELECT count(*) FROM sqlite_master) \
                      + (SELECT count(*) FROM sqlite_temp_master)",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .is_ok_and(|objects| objects == 0)
    }

    /// Record that a `CREATE DATABASE` created this database, so it is a database the
    /// cluster HAS until a statement drops it.
    pub fn declare(&mut self) {
        self.declared = true;
    }

    /// An empty database, CONFINED before it can run anything: see the module
    /// docs for the whole list. This is the only place a connection is created.
    pub fn open() -> Result<Self, SqlError> {
        let mut conn = Connection::open_in_memory().map_err(SqlError::unavailable)?;

        // Memory, value size, statement size and complexity. Set first, so they
        // hold for every statement the connection ever prepares.
        //
        // Every schema is capped, not just `main`: SQLite keeps `temp` as its own
        // database with its own `max_page_count`, and caller SQL can write into it
        // (`CREATE TEMP TABLE`, a materialised recursive CTE). A TEMP object lives
        // for the connection's lifetime — so for the lifetime of this stored
        // `Database`, across requests — which a per-statement time budget does not
        // bound. `temp_store = MEMORY` first, so the temp database is created in
        // memory rather than as a host file, and then the same page cap on it.
        // Naming `temp` in a pragma is itself what opens that database, so the cap
        // is in force before any caller statement can reach it.
        conn.pragma_update(None, "temp_store", TEMP_STORE_MEMORY)
            .map_err(SqlError::unavailable)?;
        for schema in CAPPED_SCHEMAS {
            conn.pragma_update(Some(schema), "max_page_count", MAX_DATABASE_PAGES)
                .map_err(SqlError::unavailable)?;
        }
        // The ENGINE's allocator, which the page caps above do not bound: an
        // engine-internal allocation (pager and sorter scratch, a statement's
        // working set) is charged to the heap, not to a database's pages, and the
        // heap limit is process-wide, so it is the one bound that holds across every
        // database and transaction snapshot open at once. The pragma REPORTS the
        // ceiling the engine ended up with, so it is read back and a build that
        // would not take one is refused here rather than serving statements the
        // module docs claim are memory-bounded and are not.
        let heap_limit = conn
            .pragma_update_and_check(None, "hard_heap_limit", MAX_ENGINE_HEAP_BYTES, |row| {
                row.get::<_, i64>(0)
            })
            .map_err(SqlError::unavailable)?;
        if heap_limit != MAX_ENGINE_HEAP_BYTES {
            return Err(SqlError::Unavailable(format!(
                "the engine would not take a {MAX_ENGINE_HEAP_BYTES}-byte heap ceiling; \
                 it reports {heap_limit}"
            )));
        }
        for (limit, value) in [
            // No second database, ever: this also takes `VACUUM INTO` away, which
            // the engine implements by attaching its output file.
            (Limit::SQLITE_LIMIT_ATTACHED, 0),
            (Limit::SQLITE_LIMIT_LENGTH, MAX_VALUE_BYTES),
            (Limit::SQLITE_LIMIT_SQL_LENGTH, MAX_SQL_BYTES),
            (Limit::SQLITE_LIMIT_EXPR_DEPTH, MAX_EXPRESSION_DEPTH),
            (Limit::SQLITE_LIMIT_COMPOUND_SELECT, MAX_COMPOUND_SELECT),
            (Limit::SQLITE_LIMIT_VARIABLE_NUMBER, MAX_BOUND_PARAMETERS),
            (Limit::SQLITE_LIMIT_LIKE_PATTERN_LENGTH, MAX_LIKE_PATTERN_BYTES),
        ] {
            conn.set_limit(limit, value).map_err(SqlError::unavailable)?;
        }

        // No schema the engine trusts, and the engine's own defensive mode on.
        // Extension loading needs no switch here: `rusqlite`'s `load_extension`
        // feature is deliberately NOT enabled, so the C call that would turn it on
        // is not compiled in, and the `load_extension` SQL function is refused by
        // the authorizer below as well.
        for (config, enabled) in [
            (DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false),
            (DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true),
            // A double-quoted word is an IDENTIFIER and nothing else, in a
            // statement and in a declaration alike. The engine's default is to
            // fall back to reading one it cannot resolve as a string literal,
            // which PostgreSQL never does — and which would quietly turn a
            // misspelled column into a string value, now that every name this
            // service renders is delimited (see `postgres::names`). With the
            // fallback off, a name that resolves to nothing is the error it is in
            // PostgreSQL.
            (DbConfig::SQLITE_DBCONFIG_DQS_DML, false),
            (DbConfig::SQLITE_DBCONFIG_DQS_DDL, false),
        ] {
            conn.set_db_config(config, enabled)
                .map_err(SqlError::unavailable)?;
        }

        // The reader of the INPUT FORMAT a binary value is written in, defined on this
        // connection because the format is the DATABASE's and not the engine's: a
        // `bytea` value written as text (PostgreSQL's documented hex format, `\x` and
        // two hexadecimal digits per byte) holds the BYTES that text spells, where the
        // engine's own cast to its binary type would store the text's own bytes. Every
        // cast to a binary type is rendered as a call of this (see
        // `postgres::binary_input_reader`), so the format is decoded whatever the value
        // turns out to be: a literal, or a value a parameter binds.
        conn.create_scalar_function(
            postgres::binary_input_reader(),
            1,
            rusqlite::functions::FunctionFlags::SQLITE_UTF8
                | rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC,
            |context| Ok(binary_input(context.get_raw(0))),
        )
        .map_err(SqlError::unavailable)?;

        // Default deny on every SQL action, installed LAST so the settings above
        // (which are themselves pragmas) are not refused by it.
        conn.authorizer(Some(confine_to_data_plane))
            .map_err(SqlError::unavailable)?;
        Ok(Self {
            conn,
            sequence_columns: HashMap::new(),
            version: next_version(),
            declared: false,
        })
    }

    /// A private copy of this database — the same schema and the same rows — for a
    /// transaction to write into. Copied by SQLite's own backup API, so the copy is
    /// byte-identical to the source database, and bounded by the same
    /// `max_page_count` as every other database here.
    pub fn snapshot(&self) -> Result<Self, SqlError> {
        let mut copy = Self::open()?;
        {
            let backup = rusqlite::backup::Backup::new(&self.conn, &mut copy.conn)
                .map_err(SqlError::unavailable)?;
            backup
                .run_to_completion(1024, Duration::ZERO, None)
                .map_err(SqlError::from_engine)?;
        }
        // The copy carries the same schema, so it carries the same declarations —
        // and the same CONTENT, so it is at the same version as the database it was
        // taken from.
        copy.sequence_columns = self.sequence_columns.clone();
        copy.version = self.version;
        Ok(copy)
    }

    /// The version this database's content is at: what a transaction compares to tell
    /// whether the committed database it reads has moved since it last ran.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// The high-water mark the engine keeps for each table whose key it assigns:
    /// the largest key the sequence has handed out, by table name.
    ///
    /// A database that has handed out none (no table declares a sequence, or
    /// none has been inserted into) reports nothing.
    pub fn sequence_marks(&self) -> Vec<(String, i64)> {
        let Ok(mut statement) = self.conn.prepare("SELECT name, seq FROM sqlite_sequence") else {
            return Vec::new();
        };
        let Ok(marks) = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        }) else {
            return Vec::new();
        };
        marks.filter_map(Result::ok).collect()
    }

    /// Carry `marks` into this database's own sequences, each to the larger of
    /// the two.
    ///
    /// A sequence is NOT part of the transaction that read from it: PostgreSQL
    /// documents that "changes made by nextval are not undone if the transaction
    /// rolls back", so the values a rolled-back statement took out of a sequence
    /// are not handed out again. The transaction's own copy of the database is
    /// what held those values, which is why they are carried back out of it
    /// before it is dropped.
    pub fn advance_sequences(&self, marks: &[(String, i64)]) {
        for (table, mark) in marks {
            // Only for a table this database has, and only forwards.
            let advanced = self
                .conn
                .execute(
                    "UPDATE sqlite_sequence SET seq = ?2 WHERE name = ?1 AND seq < ?2",
                    rusqlite::params![table, mark],
                )
                .and_then(|_| {
                    self.conn.execute(
                        "INSERT INTO sqlite_sequence (name, seq) SELECT ?1, ?2 \
                         WHERE NOT EXISTS (SELECT 1 FROM sqlite_sequence WHERE name = ?1) \
                           AND EXISTS \
                             (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
                        rusqlite::params![table, mark],
                    )
                });
            if let Err(error) = advanced {
                // The sequence would hand a value out twice, which is the one
                // thing this is for, so it is reported rather than passed over.
                tracing::warn!(
                    %error,
                    table = %table,
                    "the rds-data data plane could not carry a sequence's mark out of a transaction"
                );
            }
        }
    }

    /// Run `sql` with `parameters` bound by name, under a budget of its own.
    pub fn run(
        &mut self,
        sql: &str,
        parameters: &[SqlParameter],
    ) -> Result<StatementOutcome, SqlError> {
        self.run_within(&mut RequestBudget::new(), sql, parameters)
    }

    /// Run `sql` with `parameters` bound by name, and report what the engine did.
    ///
    /// `budget` is what ONE REQUEST may spend: its deadline and its remaining
    /// response bytes. A caller that runs several statements for one request
    /// (`BatchExecuteStatement`) passes the SAME budget to each, so neither the
    /// request's engine time nor the memory its results occupy grows with the
    /// number of statements it asks for.
    ///
    /// The engine decides everything: whether the statement produces a result set
    /// (`column_count`), how many rows it changed (`changes`), and the type of
    /// every value it returns.
    ///
    /// The deadline is enforced from BOTH sides of the engine, because the
    /// progress handler alone cannot reach inside ONE virtual-machine instruction
    /// (a single function call, one pattern comparison) — and this request holds
    /// the state lock for as long as the engine stays inside it: the handler
    /// aborts the statement at the first instruction boundary past the deadline,
    /// and a [`Watchdog`] asks the engine to unwind whatever it is in when that
    /// same deadline passes. Both arrive as the engine's interrupted code, which
    /// is the modelled `StatementTimeoutException`.
    pub fn run_within(
        &mut self,
        budget: &mut RequestBudget,
        sql: &str,
        parameters: &[SqlParameter],
    ) -> Result<StatementOutcome, SqlError> {
        let deadline = budget.deadline;
        self.conn
            .progress_handler(
                PROGRESS_HANDLER_OPS,
                Some(move || Instant::now() >= deadline),
            )
            .map_err(SqlError::unavailable)?;
        let watchdog = Watchdog::arm(self.conn.get_interrupt_handle(), deadline);
        let outcome = self.run_bounded(budget, sql, parameters);
        // Dismissed first, so the engine is never asked to unwind a statement
        // that is no longer running.
        drop(watchdog);
        self.conn
            .progress_handler(PROGRESS_HANDLER_OPS, None::<fn() -> bool>)
            .map_err(SqlError::unavailable)?;
        // A statement that CHANGED this database takes it to a new version, which is
        // what tells a transaction reading this database that it has moved.
        if outcome.as_ref().is_ok_and(StatementOutcome::changed_it) {
            self.version = next_version();
        }
        // An error the DATABASE raised reaches the caller in the words of the database
        // this API answers for, with its SQLSTATE — never in the words of whichever
        // engine ran the statement.
        outcome.map_err(|error| self.in_postgresql_words(error))
    }

    /// The same failure, worded as Aurora PostgreSQL words that condition
    /// ([`postgres::postgresql_diagnostic`]). A condition that front end does not word,
    /// and every failure that is not the engine refusing a statement, is unchanged.
    fn in_postgresql_words(&self, error: SqlError) -> SqlError {
        let SqlError::Engine(message) = &error else {
            return error;
        };
        match postgres::postgresql_diagnostic(message, &|table| self.primary_key_of(table)) {
            Some(worded) => SqlError::Engine(worded),
            None => error,
        }
    }

    /// The columns the PRIMARY KEY of `table` covers, in key order, read from the engine's
    /// own schema. Empty for a table it holds no such declaration for.
    fn primary_key_of(&self, table: &str) -> Vec<String> {
        let Ok(mut statement) = self.conn.prepare(
            "SELECT info.name FROM sqlite_master AS m JOIN pragma_table_info(m.name) AS info \
             WHERE m.type = 'table' AND m.name = ?1 COLLATE NOCASE AND info.pk > 0 \
             ORDER BY info.pk",
        ) else {
            return Vec::new();
        };
        let Ok(columns) = statement.query_map([table], |row| row.get::<_, String>(0)) else {
            return Vec::new();
        };
        columns.filter_map(Result::ok).collect()
    }

    /// Run `sql` once per parameter set, as ONE unit of work: either every statement
    /// of the batch is applied, or none of them is.
    ///
    /// `BatchExecuteStatement` is "a batch SQL statement over an array of data", and a
    /// batch one of whose statements fails leaves nothing behind: the whole batch is
    /// refused, so a caller retrying it cannot land a row twice. The statements run
    /// inside a savepoint the engine itself undoes, which is what makes that atomic
    /// rather than a sequence of writes this module tries to take back by hand.
    ///
    /// `budget` is the REQUEST's, shared by every statement of the batch, so N parameter
    /// sets buy neither N deadlines nor N times the output.
    pub fn run_batch_within(
        &mut self,
        budget: &mut RequestBudget,
        sql: &str,
        parameter_sets: &[Vec<SqlParameter>],
    ) -> Result<Vec<StatementOutcome>, SqlError> {
        self.conn
            .execute_batch(BATCH_SAVEPOINT_BEGIN)
            .map_err(SqlError::from_engine)?;
        let mut outcomes = Vec::with_capacity(parameter_sets.len());
        let mut refused = None;
        for parameters in parameter_sets {
            match self.run_within(budget, sql, parameters) {
                Ok(outcome) => outcomes.push(outcome),
                Err(error) => {
                    refused = Some(error);
                    break;
                }
            }
        }
        let Some(error) = refused else {
            self.conn
                .execute_batch(BATCH_SAVEPOINT_RELEASE)
                .map_err(SqlError::from_engine)?;
            return Ok(outcomes);
        };
        // Undoing the batch is what makes the refusal atomic, so a failure to undo it is
        // reported rather than passed over: the caller is told the batch was refused, and
        // this says so if the engine could not take the earlier statements back.
        if let Err(undone) = self.conn.execute_batch(BATCH_SAVEPOINT_ROLLBACK) {
            tracing::error!(
                error = %undone,
                "the rds-data data plane could not undo a refused batch"
            );
        }
        Err(error)
    }

    fn run_bounded(
        &mut self,
        budget: &mut RequestBudget,
        sql: &str,
        parameters: &[SqlParameter],
    ) -> Result<StatementOutcome, SqlError> {
        // The statement is parsed as the dialect the API's own engines speak and the
        // engine is handed this plan's rendering of that parse. The plan is given the
        // KEY of a table an upsert names, because the engine spells an upsert with the
        // conflicting key named and only the table's own declaration holds it (see
        // `postgres::plan_upsert`).
        let plan = {
            let catalog = postgres::Catalog {
                key_columns: &|table: &str| conflict_key_columns(&self.conn, table),
                column_type: &|column: &str| declared_column_type(&self.conn, column),
            };
            postgres::plan(sql, &catalog)?
        };
        let mut statement = self.conn.prepare(&plan.sql).map_err(SqlError::from_engine)?;
        let mut columns = self.describe(&statement, &plan);
        // The values this statement assigns have to be values the columns it assigns
        // them to TAKE, and the values it CASTS have to be values the types it casts
        // them to read: the database decides both before it stores anything.
        self.require_assignable_values(&plan, parameters)?;
        self.require_readable_values(&plan, parameters)?;
        bind_named_parameters(&mut statement, parameters)?;

        let mut rows = Vec::new();
        if statement.column_count() > 0 {
            let mut cursor = statement.raw_query();
            while let Some(row) = cursor.next().map_err(SqlError::from_engine)? {
                // The ROW's own container is charged BEFORE it is filled, as each of
                // its values is charged before it is copied: a result set of cheap
                // values is then bounded by what it really occupies, containers
                // included, so a statement cannot grow this process by returning
                // millions of empty rows.
                budget.charge(row_bytes(columns.len()))?;
                let mut record = Vec::with_capacity(columns.len());
                for index in 0..columns.len() {
                    // The REQUEST's remaining bytes are charged for the value the
                    // engine is holding, BEFORE a copy of it is made, so neither
                    // one statement nor a whole batch of them can allocate past
                    // the cap on the way to being refused.
                    let value = row.get_ref(index).map_err(SqlError::from_engine)?;
                    budget.charge(value_bytes(&value))?;
                    record.push(owned_value(value));
                }
                rows.push(record);
            }
            name_undeclared_types(&mut columns, &rows);
        } else {
            statement.raw_execute().map_err(SqlError::from_engine)?;
        }

        // Only a data-manipulation statement has a row count, and it is the rows
        // the statement itself changed: the engine does not count what a
        // sequence's trigger then wrote, any more than PostgreSQL's command tag
        // counts what a trigger did.
        let changed = if plan.changes_rows {
            i64::try_from(self.conn.changes()).unwrap_or(i64::MAX)
        } else {
            0
        };
        drop(statement);
        // The statements that complete this one: the sequence a declaration
        // needs, once the table it belongs to exists, and the further tables a
        // statement empties.
        for follow_on in &plan.follow_on {
            self.conn
                .execute(follow_on, [])
                .map_err(SqlError::from_engine)?;
        }
        // What the statement declared, and what it dropped: a table's
        // declarations are replaced wholesale when it is created, and go with it
        // when it is dropped, so a table's remembered declarations are the ones
        // its current definition made.
        for table in plan
            .dropped_tables
            .iter()
            .chain(plan.sequence_columns.iter().map(|column| &column.table))
        {
            self.sequence_columns
                .retain(|(declared_in, _), _| declared_in != table);
        }
        for column in &plan.sequence_columns {
            self.sequence_columns.insert(
                (column.table.clone(), column.column.clone()),
                DeclaredSequence {
                    type_name: column.type_name.clone(),
                    reports_generated_key: column.reports_generated_key,
                },
            );
        }
        // The RESULT is shaped once the statement has been APPLIED, so a statement whose
        // result cannot be returned still did what it said: AWS runs and commits an
        // `INSERT … RETURNING` whose returned value the API cannot represent, and only
        // then refuses the result. Refusing it before the statement ran would make
        // "the result is unsupported" indistinguishable from "the statement was rejected"
        // — the row would be missing against this emulator and present against AWS.
        require_returnable(&columns)?;
        Ok(StatementOutcome {
            generated_fields: self.generated_fields(&plan, changed),
            columns,
            rows,
            records_updated: changed,
        })
    }

    /// `generatedFields` for the row an insert stored: the key the ENGINE assigned to
    /// it, read from the engine itself.
    ///
    /// Reported for a table whose declaration carries an identity key the engine
    /// assigns and the API reports — MySQL's `AUTO_INCREMENT` key — and EMPTY for every
    /// other statement and every other table, because
    /// `ExecuteStatementResponse.generatedFields`'s own documentation says "The
    /// generatedFields data isn't supported by Aurora PostgreSQL. To get the values of
    /// generated fields, use the RETURNING clause": a `serial` column's assigned value
    /// is read through RETURNING and not from this list.
    ///
    /// The value is the key of the LAST row the statement stored, which for an insert
    /// of one row is the key it generated for that row.
    fn generated_fields(&self, plan: &Plan, changed: i64) -> Vec<Field> {
        if changed == 0 {
            return Vec::new();
        }
        let Some(table) = plan.inserted_table.as_deref() else {
            return Vec::new();
        };
        if !self.reports_generated_key(table) {
            return Vec::new();
        }
        vec![Field::longValue(self.conn.last_insert_rowid())]
    }

    /// Whether a table declares an identity key whose assigned value `generatedFields`
    /// reports.
    fn reports_generated_key(&self, table: &str) -> bool {
        let table = table.to_ascii_lowercase();
        self.sequence_columns
            .iter()
            .any(|((declared_in, _), sequence)| {
                *declared_in == table && sequence.reports_generated_key
            })
    }

    /// Refuse a value the column the statement ASSIGNS it to does not take.
    ///
    /// A parameter reaches the database as a value of the type its `Field` member
    /// names, and a `stringValue` carrying no `typeHint` is a value written as TEXT:
    /// PostgreSQL has no implicit cast from a character type to another type, so such a
    /// value can be assigned to a character column and to no other — and the database
    /// says so rather than storing it. Which is what `SqlParameter$typeHint` is for:
    /// the model documents each hint as what "the corresponding String parameter value
    /// is sent as", so a hinted value is sent as a value of THAT type and is not text
    /// at all. A cast the statement writes is the other way of saying it, and is not an
    /// assignment of the parameter (see [`postgres::Plan::assigned_values`]).
    ///
    /// Only where the statement and the schema BOTH settle the question: a placeholder
    /// the statement assigns directly, to a column of a table this database holds, of a
    /// type the catalog names and that does not take a value written as text.
    fn require_assignable_values(
        &self,
        plan: &Plan,
        parameters: &[SqlParameter],
    ) -> Result<(), SqlError> {
        let Some(table) = plan.assigned_table.as_deref() else {
            return Ok(());
        };
        for assigned in &plan.assigned_values {
            let sent_as_text = parameters.iter().any(|parameter| {
                parameter.name.as_deref() == Some(assigned.placeholder.as_str())
                    && parameter.type_hint.is_none()
                    && matches!(parameter.value.as_ref(), Some(Field::stringValue(_)))
            });
            if !sent_as_text {
                continue;
            }
            let Some(declared) = declared_type_in(&self.conn, table, &assigned.column) else {
                continue;
            };
            let pg = postgres::pg_type(&declared);
            if !pg.takes_text {
                return Err(SqlError::Engine(postgres::wrong_type_for_column(
                    &assigned.column,
                    &pg,
                    postgres::CHARACTER_VALUE_TYPE,
                    assigned.position,
                )));
            }
        }
        Ok(())
    }

    /// Refuse a value the type the statement CASTS it to does not read.
    ///
    /// A cast is how a statement says which type a value written as text is, and the
    /// type then reads that text as one of its own values or refuses it — "invalid
    /// input syntax for type <type>" — rather than reading as much of it as it can and
    /// storing a value the caller never wrote. The same rule the statement's own
    /// literals are held to (`postgres::reads_text`), applied to the values the
    /// request carries, which the statement's text does not hold.
    fn require_readable_values(
        &self,
        plan: &Plan,
        parameters: &[SqlParameter],
    ) -> Result<(), SqlError> {
        for (placeholder, pg) in &plan.cast_values {
            let sent = parameters.iter().find_map(|parameter| {
                match (parameter.name.as_deref(), parameter.value.as_ref()) {
                    (Some(name), Some(Field::stringValue(text))) if name == placeholder => {
                        Some(text)
                    }
                    _ => None,
                }
            });
            let Some(text) = sent else {
                continue;
            };
            if !postgres::reads_text(pg, text) {
                return Err(SqlError::Engine(postgres::invalid_input_syntax(
                    pg.worded_name(),
                    text,
                )));
            }
        }
        Ok(())
    }

    /// The declared shape of each column of a prepared statement's result set:
    /// the column's label, its PostgreSQL type, and — when the result column is a
    /// stored column — the `NOT NULL` and primary-key declarations of the table
    /// it is stored in.
    ///
    /// A column's type comes from the table that declares it when there is one,
    /// and otherwise from `plan`, which carries the type the statement's own text
    /// gives a computed column (a cast, a literal). A column neither names is
    /// left for [`name_undeclared_types`] to name from the storage class its
    /// values arrive in.
    ///
    /// WHICH column of which table a result column comes from is the ENGINE's own
    /// answer (`table_name`/`origin_name`), not a search of the schema for the label:
    /// an ALIAS renames a result column without detaching it from the column it reads,
    /// so the table, the declaration and the sequence are all looked up under the name
    /// the table gave it.
    fn describe(&self, statement: &rusqlite::Statement<'_>, plan: &Plan) -> Vec<ColumnInfo> {
        let count = statement.column_count();
        statement
            .columns()
            .enumerate()
            .map(|(index, column)| {
                let label = column.name().to_string();
                // The engine reports a declared type only for a column that is
                // stored in a table, so its absence means this result column is
                // computed (an expression, an aggregate) and no table declares it.
                let declared_type = column.decl_type().map(str::to_string);
                // WHICH column of which table this result column reads: an ALIAS
                // renames a result column without detaching it from the column it
                // reads, so the table, the declaration and the sequence are looked up
                // under the name the TABLE gave it. A wildcard expands to the columns
                // of the one table the statement reads, under their own names.
                let (reads_from, stored_as) = match plan.result_origin(index, count) {
                    Some(origin) => (origin.table.clone(), origin.column.clone()),
                    None => (plan.sole_table.clone(), label.clone()),
                };
                // Only a column a TABLE declares has a declaration to report: the
                // engine names a declared type for exactly those, and the schema is
                // asked for that column of that table.
                let declared = declared_type.as_ref().and(reads_from.as_deref()).and_then(
                    |table| self.declared_column(table, &stored_as),
                );
                let table_name = declared.as_ref().and(reads_from);
                let (pg, named_type) = match declared_type.as_deref() {
                    Some(declared_type) => {
                        // A column the engine reports as an integer may be one
                        // this database declared with a sequence, which is the
                        // type the catalog holds for it.
                        let mut pg =
                            match self.declared_sequence(table_name.as_deref(), &stored_as) {
                                Some(sequence) => postgres::pg_type(&sequence),
                                None => postgres::pg_type(declared_type),
                            };
                        // A refusal the STATEMENT's own text determines stands even where
                        // the table names the column's type: it is about the VALUE this
                        // statement puts in that column, not about the declaration.
                        if pg.refusal.is_none() {
                            pg.refusal = plan
                                .result_type(index, count)
                                .and_then(|planned| planned.refusal.clone());
                        }
                        (pg, true)
                    }
                    None => match plan.result_type(index, count) {
                        Some(pg) => (pg.clone(), true),
                        None => (postgres::type_of_storage_class(None), false),
                    },
                };
                ColumnInfo {
                    label,
                    table_name,
                    declared: declared_type.is_some(),
                    named_type,
                    // A primary key is NOT NULL, whatever the engine's own
                    // nullability flag says about the column it stores it in.
                    nullable: match &declared {
                        Some(column) if column.not_null || column.primary_key => COLUMN_NO_NULLS,
                        _ => COLUMN_NULLABLE,
                    },
                    // A `serial` declaration carries a sequence, and the engine
                    // also assigns the value of an integer primary key an INSERT
                    // leaves out.
                    auto_increment: pg.auto_increment
                        || declared.as_ref().is_some_and(|declared| {
                            declared.primary_key && declared.type_name.eq_ignore_ascii_case("INTEGER")
                        }),
                    pg,
                }
            })
            .collect()
    }

    /// The type a sequence-carrying column was declared with, when the column a
    /// result set reports is one.
    ///
    /// Keyed by the table the column is stored in and the name that table gave it, so
    /// it answers for the column that was DECLARED however the result set labels it.
    fn declared_sequence(&self, table: Option<&str>, stored_as: &str) -> Option<String> {
        let table = table?.to_ascii_lowercase();
        self.sequence_columns
            .get(&(table, stored_as.to_ascii_lowercase()))
            .map(|sequence| sequence.type_name.clone())
    }

    /// How one table's schema declares the column it stores as `stored_as`, read from
    /// the engine's own schema (`pragma_table_info`). `None` for a table or a column
    /// the engine holds no declaration for, whose nullability nothing here decides.
    fn declared_column(&self, table: &str, stored_as: &str) -> Option<DeclaredColumn> {
        let mut statement = self
            .conn
            .prepare(
                "SELECT info.type, info.\"notnull\", info.pk \
                 FROM sqlite_master AS m JOIN pragma_table_info(m.name) AS info \
                 WHERE m.type = 'table' \
                   AND m.name = ?1 COLLATE NOCASE \
                   AND info.name = ?2 COLLATE NOCASE",
            )
            .ok()?;
        statement
            .query_row(rusqlite::params![table, stored_as], |row| {
                Ok(DeclaredColumn {
                    type_name: row.get::<_, String>(0)?,
                    not_null: row.get::<_, i64>(1)? != 0,
                    primary_key: row.get::<_, i64>(2)? != 0,
                })
            })
            .ok()
    }
}

/// The statement deadline enforced from OUTSIDE the engine.
///
/// [`Database::run_within`]'s progress handler stops a statement between two
/// virtual-machine instructions, which is every boundary the engine offers the
/// caller — but ONE instruction runs to completion whatever the clock says, and
/// it runs while this request holds the state lock, so the handler alone leaves
/// the lock-holding window as long as the engine's longest single operation. This
/// is the other side of the same deadline: a thread that waits for it and then
/// asks the engine to unwind whatever it is in (the interrupt the engine reports
/// as `StatementTimeoutException`), dismissed the moment the statement returns.
///
/// Dismissal is the SENDER being dropped: the thread waits on the channel until
/// the deadline, so it costs nothing until it fires and never outlives the
/// statement it was armed for.
struct Watchdog {
    dismiss: Option<std::sync::mpsc::Sender<()>>,
    waiting: Option<std::thread::JoinHandle<()>>,
}

impl Watchdog {
    /// Watch one statement until `deadline`.
    ///
    /// A thread this cannot start leaves the statement bounded by the progress
    /// handler alone, which is the bound that was there before: it is reported
    /// and the statement runs, rather than failing a request because the host
    /// would not give out a thread.
    fn arm(engine: rusqlite::InterruptHandle, deadline: Instant) -> Self {
        let (dismiss, dismissed) = std::sync::mpsc::channel::<()>();
        let waiting = std::thread::Builder::new()
            .name("rdsdata-statement-deadline".to_string())
            .spawn(move || {
                let wait = deadline.saturating_duration_since(Instant::now());
                // A disconnected channel is the statement having returned; only
                // the deadline passing interrupts the engine.
                if dismissed.recv_timeout(wait) == Err(std::sync::mpsc::RecvTimeoutError::Timeout) {
                    engine.interrupt();
                }
            });
        match waiting {
            Ok(waiting) => Self {
                dismiss: Some(dismiss),
                waiting: Some(waiting),
            },
            Err(error) => {
                tracing::warn!(
                    %error,
                    "the rds-data data plane could not watch a statement's deadline from outside \
                     the engine"
                );
                Self {
                    dismiss: None,
                    waiting: None,
                }
            }
        }
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        // Dismiss first, then wait for the thread to see it, so no interrupt is
        // in flight once this returns.
        drop(self.dismiss.take());
        if let Some(waiting) = self.waiting.take() {
            let _ = waiting.join();
        }
    }
}

/// A transaction a caller opened with `BeginTransaction`.
///
/// A transaction has three states, which are three different answers to a later call:
///
/// * OPEN — its statements run against its own copy of the database;
/// * ABORTED — a statement of its own FAILED, so "a transaction is terminated when a
///   statement in it fails" (`BeginTransaction`'s own documentation) and the database
///   ignores every further command "until end of transaction block". The transaction is
///   still THERE: only a commit or a rollback ends it, and either one discards what it
///   wrote;
/// * ENDED — a commit installed its writes, a rollback discarded them, or the CLOCK
///   rolled it back: the window it may sit idle in, or the lifetime it may run for,
///   passed. Its id stays with the cluster rather than vanishing from it, so a later call
///   quoting that id is answered for a transaction that ended — and told WHY it ended
///   ([`TransactionEnd`]) — rather than served against a copy of the database nobody can
///   commit any more.
///
/// Its copy of the database goes when it ends, so what is remembered is the id and when it
/// ended.
///
/// ## What it reads
///
/// A transaction reads at READ COMMITTED: everything committed on its database AT THE
/// MOMENT ITS STATEMENT RUNS, alongside its own writes. The copy it runs against is
/// therefore rebuilt whenever the committed database has MOVED under it — a fresh copy
/// of what is committed now, with the transaction's own writes replayed onto it
/// ([`Self::rebase_onto`]) — which is why those writes are kept as the statements they
/// are and not only as the rows they left behind.
#[derive(Debug)]
pub struct Transaction {
    /// The opaque id returned to the caller and used to find this transaction.
    pub id: String,
    pub resource_arn: String,
    pub secret_arn: String,
    pub database: Option<String>,
    pub schema: Option<String>,
    pub created_at: SystemTime,
    /// When a call last used this transaction's id, which is what the idle timeout
    /// is measured from.
    pub last_used_at: SystemTime,
    /// The transaction's own copy of the database: everything committed as of
    /// [`Self::based_on`] plus [`Self::writes`], written by its statements, installed on
    /// commit, dropped on rollback — and `None` once the
    /// transaction has ENDED, because an ended transaction has no database to write into.
    /// An ABORTED transaction still holds it: the copy is what a rollback discards.
    pending: Option<Database>,
    /// Every statement of this transaction that CHANGED its database, in the order it
    /// ran, with the values it bound: what is replayed onto the committed database when
    /// that database moves, and what a commit therefore installs on top of it.
    writes: Vec<RecordedWrite>,
    /// The version of the committed database the copy above was built from.
    based_on: u64,
    /// Set when a statement of this transaction failed, which is what the database
    /// ignores every further command of it for. A commit or a rollback still ends it.
    aborted: bool,
    /// When the transaction ended and WHY, and `None` while it is open or aborted.
    ended: Option<Ended>,
}

/// When a transaction stopped running, and why — what a refusal of its id is answered
/// from once it is no longer running.
#[derive(Debug, Clone, Copy)]
struct Ended {
    at: SystemTime,
    cause: TransactionEnd,
}

/// One statement a transaction ran that changed its database, with the values it bound.
#[derive(Debug, Clone)]
struct RecordedWrite {
    sql: String,
    parameters: Vec<SqlParameter>,
}

impl Transaction {
    /// Open a transaction on `cluster`'s `database`, minting its id and taking its
    /// snapshot. The id is opaque, unique and BOUND to the resource it was opened on
    /// ([`mint_transaction_id`]), well inside the modelled 192-character limit. A cluster
    /// holds at most [`MAX_OPEN_TRANSACTIONS`] open transactions — each is a whole copy of
    /// a database — past which opening another is refused. A transaction that ENDED holds
    /// no database, so it is not one of them.
    pub fn open(
        cluster: &mut Cluster,
        resource_arn: &str,
        secret_arn: &str,
        database: Option<&String>,
        schema: Option<&String>,
    ) -> Result<String, SqlError> {
        if cluster.open_transaction_count() >= MAX_OPEN_TRANSACTIONS {
            return Err(SqlError::Capacity(format!(
                "this emulator holds at most {MAX_OPEN_TRANSACTIONS} open transactions per cluster"
            )));
        }
        let committed = cluster.database_mut(&database_key(database))?;
        let based_on = committed.version();
        let pending = committed.snapshot()?;
        let id = mint_transaction_id(resource_arn);
        let now = SystemTime::now();
        cluster.transactions.insert(
            id.clone(),
            Self {
                id: id.clone(),
                resource_arn: resource_arn.to_string(),
                secret_arn: secret_arn.to_string(),
                database: database.cloned(),
                schema: schema.cloned(),
                created_at: now,
                last_used_at: now,
                pending: Some(pending),
                writes: Vec::new(),
                based_on,
                aborted: false,
                ended: None,
            },
        );
        Ok(id)
    }

    /// Record that a call used this transaction, restarting its idle timeout.
    pub fn touch(&mut self) {
        self.last_used_at = SystemTime::now();
    }

    /// Whether this transaction has ENDED — a commit installed its writes, a rollback
    /// discarded them, or the clock rolled it back.
    pub fn is_terminal(&self) -> bool {
        self.ended.is_some()
    }

    /// Why this transaction is no longer running, and `None` while it still is.
    pub fn end_cause(&self) -> Option<TransactionEnd> {
        self.ended.map(|ended| ended.cause)
    }

    /// When this transaction stopped running, and `None` while it still is.
    fn ended_at(&self) -> Option<SystemTime> {
        self.ended.map(|ended| ended.at)
    }

    /// Whether a statement of this transaction FAILED, so the database ignores every
    /// further command of it until the transaction block ends.
    pub fn is_aborted(&self) -> bool {
        self.aborted
    }

    /// The database this transaction's statements write into.
    ///
    /// `Err` once a statement of this transaction has failed: the database "ignores
    /// commands until end of transaction block", which is an error the DATABASE raises
    /// for every further statement rather than the transaction being gone.
    pub fn open_database(&mut self) -> Result<&mut Database, SqlError> {
        if self.aborted {
            return Err(SqlError::Engine(postgres::transaction_aborted()));
        }
        self.pending
            .as_mut()
            .ok_or(SqlError::TransactionNotFound(TransactionEnd::Terminated))
    }

    /// Whether this transaction's copy was built from the committed database as it is
    /// at `version`, so its next statement already reads everything committed.
    fn reads_version(&self, version: u64) -> bool {
        self.based_on == version
    }

    /// Rebuild the copy this transaction runs against from the COMMITTED database as it
    /// is now, with this transaction's own writes replayed onto it.
    ///
    /// What READ COMMITTED means here: a statement of this transaction reads every row
    /// another transaction committed before it ran, alongside the rows this transaction
    /// has written itself and nobody else can see yet. The rows another writer committed
    /// come from the fresh copy and this transaction's own from the replay, so both are
    /// there.
    ///
    /// What the replay does NOT reproduce: a statement that read the database as it was
    /// (`UPDATE t SET n = n + 1`, `INSERT … SELECT`) is applied again to the newer rows,
    /// and one that read a clock or a random value gets another reading. A replayed
    /// statement the newer rows REFUSE (a key another transaction has since committed)
    /// fails as that statement's own failure, which terminates this transaction the way
    /// any failed statement in it does.
    fn rebase_onto(
        &mut self,
        committed: &Database,
        budget: &mut RequestBudget,
    ) -> Result<(), SqlError> {
        let mut rebuilt = committed.snapshot()?;
        for write in &self.writes {
            rebuilt.run_within(budget, &write.sql, &write.parameters)?;
        }
        self.pending = Some(rebuilt);
        self.based_on = committed.version();
        Ok(())
    }

    /// Remember a statement of this transaction that CHANGED its database, so a later
    /// rebase puts it back.
    fn record(&mut self, sql: &str, parameters: &[SqlParameter], outcome: &StatementOutcome) {
        if !outcome.changed_it() {
            return;
        }
        self.writes.push(RecordedWrite {
            sql: sql.to_string(),
            parameters: parameters.to_vec(),
        });
    }

    /// Refuse a transaction that is already holding [`MAX_TRANSACTION_WRITES`]
    /// statements one more, BEFORE it runs: the statements a transaction has written are
    /// kept until it ends (that is what a rebase replays and a commit installs), so their
    /// number is a store one caller can grow and is capped like every other store here.
    fn require_room_for(&self, statements: usize) -> Result<(), SqlError> {
        if self.writes.len() + statements > MAX_TRANSACTION_WRITES {
            return Err(SqlError::Capacity(format!(
                "this emulator holds at most {MAX_TRANSACTION_WRITES} statements per open \
                 transaction"
            )));
        }
        Ok(())
    }

    /// End this transaction and hand back the database it was writing into, so the caller
    /// can install it (a commit) or read the sequence marks out of it and drop it (a
    /// rollback). The id stays in the cluster, now ended.
    ///
    /// `Err` for one that has already ended, which is what refuses a second commit or
    /// rollback of the same id.
    pub fn end(&mut self) -> Result<Database, SqlError> {
        let pending = self.pending.take().ok_or_else(|| {
            SqlError::TransactionNotFound(self.end_cause().unwrap_or(TransactionEnd::Terminated))
        })?;
        self.ended = Some(Ended {
            at: SystemTime::now(),
            cause: TransactionEnd::Terminated,
        });
        Ok(pending)
    }

    /// Record that a statement of this transaction FAILED, which is what terminates it:
    /// "a transaction is terminated when a statement in it fails"
    /// (`BeginTransaction`'s own documentation). Nothing it wrote can reach the committed
    /// database any more — a commit of it discards its writes exactly as a rollback does —
    /// and the copy it was writing into is kept until one of them ends it, because that is
    /// what they discard.
    pub fn abort(&mut self) {
        self.aborted = true;
    }

    /// End this transaction if the CLOCK has already ended it, and hand back the copy of
    /// the database it was writing into so its sequence marks can be carried over before
    /// it is dropped. `None` for one the clock does not owe a rollback, and for one that
    /// has already stopped running.
    ///
    /// Two windows end a transaction nobody ended:
    ///
    /// * it sat IDLE for [`TRANSACTION_IDLE_TIMEOUT`] — "a transaction times out if no
    ///   calls use its transaction ID in three minutes", and a transaction that timed out
    ///   "is rolled back automatically" — so its id is answered from then on as one the
    ///   service abandoned for inactivity ([`TransactionEnd::Idle`]);
    /// * it has been open for [`TRANSACTION_MAX_LIFETIME`] — "a transaction is terminated
    ///   and rolled back automatically after 24 hours" — which the service words as a
    ///   termination like any other ([`TransactionEnd::Terminated`]).
    ///
    /// Each is dated at the instant it FELL DUE and not at the call that noticed, so how
    /// long a terminal record is then kept does not depend on when anybody looked.
    fn end_if_the_clock_has(&mut self) -> Option<Database> {
        if self.is_terminal() {
            return None;
        }
        let idle_since = self.last_used_at + TRANSACTION_IDLE_TIMEOUT;
        let lifetime_since = self.created_at + TRANSACTION_MAX_LIFETIME;
        let now = SystemTime::now();
        let (at, cause) = if idle_since <= now {
            (idle_since, TransactionEnd::Idle)
        } else if lifetime_since <= now {
            (lifetime_since, TransactionEnd::Terminated)
        } else {
            return None;
        };
        let pending = self.pending.take();
        // The statements it wrote go with the copy they wrote into: nothing can replay
        // them any more.
        self.writes.clear();
        self.ended = Some(Ended { at, cause });
        pending
    }

    /// Whether this transaction can be FORGOTTEN: an ended one past the window a terminal
    /// id is kept for. Its id is then answered as one of a transaction that ended all the
    /// same, which is what a forgotten record and a remembered one have in common.
    fn is_forgotten(&self) -> bool {
        self.ended_at().is_some_and(|at| {
            at.elapsed()
                .is_ok_and(|since| since >= TERMINAL_TRANSACTION_MEMORY)
        })
    }
}

/// The id a `BeginTransaction` on `resource_arn` mints for the transaction it opens: an
/// opaque token that CARRIES the resource the transaction belongs to.
///
/// A transaction belongs to the DB cluster it was opened on, and every later call that
/// quotes its id names a `resourceArn` of its own: the real service can tell an id that
/// could not have been issued for the resource it is presented with from one that resolves
/// to nothing on a resource it could have come from, and answers the two differently
/// ([`transaction_id_names_resource`]). So the binding travels in the id, which is what
/// lets this emulator tell them apart too — including after the transaction has ended, when
/// no stored record of it is left to resolve.
///
/// The id is a version-4 UUID's 32 hex digits — where its uniqueness and its
/// unguessability come from — followed by a stable 16-digit mixing of the resource's ARN
/// ([`::service_common::stable_hex_id`], which nothing authenticates on). 48 characters,
/// well inside the modelled `Id` length of 192, and opaque: it carries no readable part of
/// the ARN.
#[must_use]
pub fn mint_transaction_id(resource_arn: &str) -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        ::service_common::stable_hex_id(&[resource_arn])
    )
}

/// Whether `transaction_id` is an id [`mint_transaction_id`] could have minted AT ALL: the
/// form a transaction id of this service has, whichever resource issued it.
///
/// A value of ANOTHER form could not have been issued by this service for any resource, so
/// it is refused on the call's own shape, before any transaction is looked up
/// (`preconditions`), together with an id of this form that belongs to another resource
/// ([`transaction_id_names_resource`]).
#[must_use]
pub fn is_minted_transaction_id(transaction_id: &str) -> bool {
    transaction_id.len() == UUID_HEX_DIGITS + RESOURCE_TAG_DIGITS
        && transaction_id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Whether `transaction_id` is an id [`mint_transaction_id`] could have minted for
/// `resource_arn` — the resource binding the id carries.
#[must_use]
pub fn transaction_id_names_resource(transaction_id: &str, resource_arn: &str) -> bool {
    is_minted_transaction_id(transaction_id)
        && transaction_id.ends_with(&::service_common::stable_hex_id(&[resource_arn]))
}

/// How many hexadecimal digits the mixing of the resource ARN carries, which is the second
/// part of a minted transaction id ([`::service_common::stable_hex_id`]'s own width).
const RESOURCE_TAG_DIGITS: usize = 16;

/// How many hexadecimal digits a version-4 UUID's simple form carries, which is the first
/// part of a minted transaction id.
const UUID_HEX_DIGITS: usize = 32;

/// What the engine did with one statement.
#[derive(Debug)]
pub struct StatementOutcome {
    /// The result set's columns; empty when the statement returns no result set.
    pub columns: Vec<ColumnInfo>,
    /// The rows the statement returned, in the order the engine returned them.
    pub rows: Vec<Vec<SqlValue>>,
    /// Rows the statement inserted, updated or deleted — 0 for DDL and for a
    /// statement that only reads.
    pub records_updated: i64,
    /// The values the engine GENERATED for the row this statement stored, as
    /// `generatedFields` reports them (see [`Database::generated_fields`]).
    pub generated_fields: Vec<Field>,
}

impl StatementOutcome {
    /// What a statement that produced nothing at all did: no result set, no row
    /// changed and no value generated — what a DDL statement answers with.
    pub fn nothing() -> Self {
        Self {
            columns: Vec::new(),
            rows: Vec::new(),
            records_updated: 0,
            generated_fields: Vec::new(),
        }
    }

    /// Whether the statement produced a result set (as opposed to changing rows).
    pub fn has_result_set(&self) -> bool {
        !self.columns.is_empty()
    }

    /// Whether the statement CHANGED the database it ran against: it changed rows, or
    /// it produced no result set at all, which is what a statement that defines or
    /// drops something does.
    pub fn changed_it(&self) -> bool {
        self.records_updated > 0 || !self.has_result_set()
    }

    /// The result set as `records`: one list of [`Field`]s per row.
    ///
    /// `Err` when a value has no `Field` member to come back in — answered with
    /// the modelled `UnsupportedResultException`.
    pub fn records(&self, returns: ReturnTypes) -> Result<Vec<Vec<Field>>, SqlError> {
        self.rows
            .iter()
            .map(|row| {
                row.iter()
                    .zip(&self.columns)
                    .map(|(value, column)| field_for(value, column, returns))
                    .collect()
            })
            .collect()
    }

    /// The result set's `columnMetadata`.
    pub fn column_metadata(&self) -> Vec<ColumnMetadata> {
        self.columns.iter().map(ColumnInfo::metadata).collect()
    }

    /// The result set as one JSON string: an array with one object per row, keyed
    /// by column label — what `formatRecordsAs=JSON` returns in `formattedRecords`.
    ///
    /// BOUNDED BY THE EMITTED SIZE, not by the engine's values: a column LABEL is
    /// chosen by the request (an alias in its `sql`) and is repeated once per row,
    /// so the bytes this body costs are not the bytes the row path already charged
    /// to the request's [`RequestBudget`] while reading the values out of the
    /// engine. The string is therefore built incrementally and charged — label,
    /// separator and rendered value for every cell — against THAT SAME budget, so
    /// one request has one output bound covering both what it read and what it
    /// emits, and a body that would pass it is refused BEFORE the bytes are
    /// allocated rather than materialising a map per row and then a second full
    /// copy of the whole body.
    ///
    /// `Err` when the body would pass the budget, or when a value has no JSON
    /// form at all (see [`json_for`]) — both answered with the modelled
    /// `UnsupportedResultException`.
    pub fn formatted_records(&self, budget: &mut RequestBudget) -> Result<String, SqlError> {
        let mut formatted = String::from("[");
        for (index, row) in self.rows.iter().enumerate() {
            if index > 0 {
                formatted.push(',');
            }
            formatted.push('{');
            for (position, (value, column)) in row.iter().zip(&self.columns).enumerate() {
                // The label is rendered as a JSON string, so a label carrying a
                // quote or a control character cannot break out of the key.
                let key = serde_json::Value::String(column.label.clone()).to_string();
                let rendered = json_for(value, column)?.to_string();
                // `+ 4` covers the punctuation this cell and its row still need:
                // the ':' before the value, the ',' that may separate it from the
                // next cell or row, and the '}' and ']' that must close them.
                budget.charge(key.len() + rendered.len() + 4)?;
                if position > 0 {
                    formatted.push(',');
                }
                formatted.push_str(&key);
                formatted.push(':');
                formatted.push_str(&rendered);
            }
            formatted.push('}');
        }
        formatted.push(']');
        Ok(formatted)
    }
}

/// One column of a result set, as the engine declares it.
#[derive(Debug, Clone)]
pub struct ColumnInfo {
    /// The column's label in the result set — its name, or the alias the statement
    /// gave it.
    pub label: String,
    /// The table the column is stored in, when it is a stored column.
    pub table_name: Option<String>,
    /// Whether the engine reported a declared type for this column.
    pub declared: bool,
    /// Whether this column's PostgreSQL type is known: declared by a table, or
    /// given by the statement's own text. When it is not, the type is named from
    /// the storage class the values arrive in.
    pub named_type: bool,
    /// This column's PostgreSQL type.
    pub pg: PgType,
    /// `java.sql.ResultSetMetaData`'s nullability: 0, 1 or 2.
    pub nullable: i32,
    /// Whether the engine assigns this column's value when an INSERT omits it.
    pub auto_increment: bool,
}

impl ColumnInfo {
    /// A boolean column.
    pub fn is_boolean(&self) -> bool {
        self.pg.class == TypeClass::Boolean
    }

    /// A fixed-point column, whose representation `decimalReturnType` controls.
    pub fn is_decimal(&self) -> bool {
        self.pg.class == TypeClass::Decimal
    }

    /// This column as modelled `ColumnMetadata`.
    ///
    /// Every member is this column's PostgreSQL type as the catalog holds it:
    /// `type` is the type's `java.sql.Types` code, `typeName` its catalog name,
    /// `precision` and `scale` the digits it carries, `isSigned` whether it is a
    /// signed number and `isCaseSensitive` whether its values compare as text.
    fn metadata(&self) -> ColumnMetadata {
        ColumnMetadata {
            name: Some(self.label.clone()),
            label: Some(self.label.clone()),
            type_name: Some(self.pg.name.clone()),
            r#type: self.pg.jdbc_type,
            // Both names are always reported, EMPTY where there is nothing to
            // report: a computed column is stored in no table, and a column is in
            // no schema beyond the database that holds it (the model's own
            // `schema` member "isn't supported").
            table_name: Some(self.table_name.clone().unwrap_or_default()),
            schema_name: Some(String::new()),
            nullable: self.nullable,
            precision: self.pg.precision,
            scale: self.pg.scale,
            is_auto_increment: self.auto_increment,
            is_signed: self.pg.signed,
            is_currency: false,
            is_case_sensitive: self.pg.case_sensitive,
            // The recorded response carries no base type for an array column
            // either: `arrayBaseColumnType` stays at its modelled default.
            array_base_column_type: 0,
        }
    }
}

/// Refuse a result set carrying a column of a type the `Field` union has no
/// member for, or a value the result has no form for.
///
/// The statement itself is valid and the engine ran it: it is the RESULT that
/// cannot be returned, which is what the modelled `UnsupportedResultException`
/// ("It contained an unsupported data type") says — which is why this runs AFTER the
/// statement has been applied.
fn require_returnable(columns: &[ColumnInfo]) -> Result<(), SqlError> {
    match columns.iter().find_map(|column| column.pg.refusal.as_ref()) {
        Some(refusal) => Err(SqlError::UnsupportedResult(refusal.clone())),
        None => Ok(()),
    }
}

/// What ONE REQUEST may spend on the engine.
///
/// Both axes are per REQUEST and not per statement, so a
/// `BatchExecuteStatement` with a thousand parameter sets buys no more engine
/// time and no more result memory than a single statement does:
///
/// * [`STATEMENT_TIME_BUDGET`] from the moment the budget is made, which every
///   statement of the request shares through the progress handler;
/// * [`MAX_RESPONSE_BYTES`] of result values, charged as each value is read out
///   of the engine and BEFORE it is copied, so the request is refused with
///   `UnsupportedResultException` instead of allocating past the cap.
#[derive(Debug)]
pub struct RequestBudget {
    deadline: Instant,
    response_bytes_left: usize,
}

impl RequestBudget {
    /// A fresh budget: one request's worth of wall clock and of output.
    pub fn new() -> Self {
        Self {
            deadline: Instant::now() + STATEMENT_TIME_BUDGET,
            response_bytes_left: MAX_RESPONSE_BYTES,
        }
    }

    /// Charge `bytes` of result to this request, or refuse it: the first value
    /// that does not fit stops the whole request rather than the statement.
    fn charge(&mut self, bytes: usize) -> Result<(), SqlError> {
        self.response_bytes_left = self
            .response_bytes_left
            .checked_sub(bytes)
            .ok_or(SqlError::ResultTooLarge)?;
        Ok(())
    }
}

impl Default for RequestBudget {
    fn default() -> Self {
        Self::new()
    }
}

/// A column as the table declares it.
#[derive(Debug, Clone)]
struct DeclaredColumn {
    type_name: String,
    not_null: bool,
    primary_key: bool,
}

/// How `ResultSetOptions` asks for numbers to be represented. Both members come
/// from the model's own documentation of `ResultSetOptions`.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReturnTypes {
    /// `longReturnType=STRING`: return a `LONG` as `stringValue`.
    pub long_as_string: bool,
    /// `decimalReturnType=DOUBLE_OR_LONG`: return a `DECIMAL` as `longValue` when
    /// its scale is 0 and as `doubleValue` otherwise. The model documents `STRING`
    /// as the default for a decimal, so this is false unless the caller asks.
    pub decimal_as_number: bool,
}

impl ReturnTypes {
    /// Read the two options off a request's `resultSetOptions`.
    ///
    /// Each member's values are an enum the model declares (`LongReturnType` is `LONG |
    /// STRING`, `DecimalReturnType` is `STRING | DOUBLE_OR_LONG`), and a value outside
    /// it never reaches here: the request walk refuses it against the model before this
    /// operation runs (`tests/all/typed_values.rs` pins that refusal). So what is left
    /// to read is which of the declared values the caller asked for.
    pub fn from_options(options: Option<&ResultSetOptions>) -> Self {
        let long_as_string = options
            .and_then(|options| options.long_return_type.as_deref())
            .is_some_and(|value| value.eq_ignore_ascii_case("STRING"));
        let decimal_as_number = options
            .and_then(|options| options.decimal_return_type.as_deref())
            .is_some_and(|value| value.eq_ignore_ascii_case("DOUBLE_OR_LONG"));
        Self {
            long_as_string,
            decimal_as_number,
        }
    }
}

/// Why a statement did not run, or did not produce a usable result.
#[derive(Debug)]
pub enum SqlError {
    /// The engine refused the statement: a syntax error, an unknown table, a
    /// constraint violation, an action the authorizer does not allow, or a limit
    /// the connection carries.
    Engine(String),
    /// The request's `parameters` cannot be bound to the statement's placeholders: a
    /// placeholder with no parameter, a parameter named twice, a name that is not a
    /// parameter name, or a value the service refuses as a parameter. A fault of the
    /// CALL rather than of the statement, so it is reported as a validation fault and
    /// carries the sentence AWS words that fault with ([`parameter_faults`]).
    Parameter(String),
    /// The result the statement produced cannot be returned in the form the
    /// request asked for.
    UnsupportedResult(String),
    /// The request quoted a `transactionId` no open transaction answers to, and WHY that
    /// transaction is no longer running — the only thing that tells a caller a
    /// transaction the service abandoned for inactivity from one a previous call ended,
    /// since both are the modelled `TransactionNotFoundException`.
    TransactionNotFound(TransactionEnd),
    /// The request itself is wrong in a way the model's own constraints do not
    /// express.
    InvalidRequest(String),
    /// A bound on how much of this emulator's own room is left: the stored
    /// clusters, databases or open transactions are at their cap. Nothing is
    /// wrong with the request, so this is reported as the modelled
    /// `ServiceUnavailableError` (HTTP 503) and not blamed on the caller.
    Capacity(String),
    /// The statement, or the request, outran its deadline.
    Timeout,
    /// The result set passed [`MAX_RESPONSE_BYTES`].
    ResultTooLarge,
    /// The data plane itself could not be served — not the caller's fault.
    Unavailable(String),
}

impl SqlError {
    /// Classify a `rusqlite` failure: an aborted statement is the budget being
    /// enforced, a refused allocation is this emulator's own memory ceiling, and
    /// anything else is the engine refusing the statement.
    fn from_engine(error: rusqlite::Error) -> Self {
        if let rusqlite::Error::SqliteFailure(failure, _) = &error {
            match failure.code {
                rusqlite::ErrorCode::OperationInterrupted => return Self::Timeout,
                // The engine hit [`MAX_ENGINE_HEAP_BYTES`], which is this
                // emulator's room and not the caller's request being wrong, so it
                // is reported the way every other cap here is: 503, never 400.
                rusqlite::ErrorCode::OutOfMemory => {
                    return Self::Capacity(format!(
                        "the SQL engine is at this emulator's {MAX_ENGINE_HEAP_BYTES}-byte memory \
                         ceiling"
                    ))
                }
                _ => {}
            }
        }
        Self::Engine(error.to_string())
    }

    /// A failure to stand the data plane up, which says nothing about the request.
    fn unavailable(error: rusqlite::Error) -> Self {
        Self::Unavailable(error.to_string())
    }

    /// Whether a transaction a statement failed in is ENDED by that failure, or stays
    /// there to be ended by a commit or a rollback.
    ///
    /// "A transaction is terminated when a statement in it fails"
    /// (`BeginTransaction`'s own documentation): a statement the engine refused, or one
    /// its deadline stopped, reached the database and failed there, so the transaction it
    /// was part of can no longer be committed. A fault of the CALL — a parameter that
    /// cannot be bound, a member the model does not constrain — never reached the
    /// database, and a RESULT the API cannot represent is not the statement failing:
    /// neither touches the transaction.
    pub fn terminates_a_transaction(&self) -> bool {
        matches!(self, Self::Engine(_) | Self::Timeout)
    }

    /// What to tell the caller.
    pub fn message(&self) -> String {
        match self {
            Self::Engine(message)
            | Self::Parameter(message)
            | Self::UnsupportedResult(message)
            | Self::InvalidRequest(message)
            | Self::Capacity(message) => message.clone(),
            Self::TransactionNotFound(end) => end.sentence().to_string(),
            Self::Timeout => format!(
                "the statement did not finish within {} seconds",
                STATEMENT_TIME_BUDGET.as_secs()
            ),
            Self::ResultTooLarge => format!(
                "the result set is larger than the {} MB this call can return",
                MAX_RESPONSE_BYTES / (1024 * 1024)
            ),
            Self::Unavailable(_) => "An internal error occurred".to_string(),
        }
    }
}

/// What caller SQL may do inside this emulator's data plane — the authorizer every
/// connection carries.
///
/// DEFAULT DENY: an action not named here is refused, so an engine capability this
/// emulator has not considered cannot be reached from a request. What is allowed is
/// reading and writing this database's own tables, the DDL that defines them,
/// transactions, and pure functions over the values in them. What is refused, and
/// why it matters: `ATTACH`/`DETACH` would make a host path visible to SQL (and
/// `VACUUM INTO` is implemented by attaching its output file), `PRAGMA` reaches
/// engine configuration including temp-file paths, and a virtual table other than
/// the schema introspection this module runs is arbitrary registered code. The
/// functions and the virtual-table modules are allow-listed by name
/// ([`PERMITTED_FUNCTIONS`], [`SCHEMA_INTROSPECTION_PRAGMAS`]), so a capability
/// this emulator has not reviewed is refused rather than allowed for want of an
/// entry in a list of refusals.
fn confine_to_data_plane(context: AuthContext<'_>) -> Authorization {
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
        // Pure computation over the values in those tables, as an ALLOW-LIST: a
        // function this emulator has not reviewed — including one a future engine
        // build defines — falls through to the deny arm below.
        AuthAction::Function { function_name } => {
            if PERMITTED_FUNCTIONS
                .iter()
                .any(|allowed| function_name.eq_ignore_ascii_case(allowed))
                || function_name.eq_ignore_ascii_case(postgres::binary_input_reader())
            {
                Authorization::Allow
            } else {
                Authorization::Deny
            }
        }
        // The schema introspection THIS module runs for `columnMetadata`
        // (`pragma_table_info`), by EXACT name, and nothing else: every other
        // module — a registered extension, `sqlite_dbpage`, an FTS or RTREE table
        // — is refused.
        AuthAction::CreateVtable { module_name, .. } => {
            let module = module_name.to_ascii_lowercase();
            match module.strip_prefix("pragma_") {
                Some(pragma) if SCHEMA_INTROSPECTION_PRAGMAS.contains(&pragma) => {
                    Authorization::Allow
                }
                _ => Authorization::Deny,
            }
        }
        AuthAction::Pragma { pragma_name, .. } => {
            if SCHEMA_INTROSPECTION_PRAGMAS
                .iter()
                .any(|allowed| pragma_name.eq_ignore_ascii_case(allowed))
            {
                Authorization::Allow
            } else {
                Authorization::Deny
            }
        }
        // Everything that could reach outside this emulator's own state, and
        // everything this emulator has not considered.
        _ => Authorization::Deny,
    }
}

/// The sentences AWS refuses each distinct parameter-binding fault with. Every one is
/// fixed text but for the NAME, which comes from the request's own `parameters` or from
/// the placeholder in its statement.
///
/// A binding fault is a fault of the CALL, not of the statement, which is why these are
/// answered as validation faults and never as a SQL error: the caller has to change its
/// request, not its data.
pub mod parameter_faults {
    /// A placeholder in the statement that no parameter supplies a value for. The name is
    /// the placeholder's, WITHOUT the prefix that introduces it in the statement.
    pub fn cannot_find(name: &str) -> String {
        format!("Cannot find parameter: {name}")
    }

    /// Two parameters of the same name: which value the placeholder takes is not the
    /// service's to choose.
    pub fn duplicate(name: &str) -> String {
        format!("Duplicate parameter {name}")
    }

    /// A parameter whose NAME carries the ':' that introduces a placeholder in the
    /// statement. The name belongs to the parameter; the prefix belongs to the statement.
    pub fn invalid_name(name: &str) -> String {
        format!("Named parameter syntax is invalid, input: {name}")
    }

    /// An `arrayValue` parameter. The `Field` union declares an `arrayValue` arm — it is
    /// how an array comes BACK — but `parameters`'s own documentation says "Array
    /// parameters are not supported".
    pub const ARRAYS_UNSUPPORTED: &str = "Array parameters are not supported.";
}

/// Refuse a request whose `parameters` cannot be bound, before the statement runs.
///
/// Three faults are decided from the request alone — a name repeated, a name carrying the
/// placeholder prefix, and an `arrayValue` — so they are decided here, ahead of the
/// engine: the statement must not run and then be refused. The fourth, a placeholder with
/// no parameter, needs the statement the engine prepared and is reported from
/// [`bind_named_parameters`].
///
/// BOUNDED, and in ONE pass: the list's length is the caller's to choose, so it is capped
/// at [`MAX_PARAMETERS_PER_STATEMENT`] before anything is scanned, and the repeated name is
/// found through a set rather than by comparing each name with the ones before it — so the
/// work is linear in the list and the list is bounded.
///
/// Shared by both operations that carry parameters, so one set of parameters is judged one
/// way whichever of them is called.
pub fn require_bindable_parameters(parameters: &[SqlParameter]) -> Result<(), SqlError> {
    if parameters.len() > MAX_PARAMETERS_PER_STATEMENT {
        // A property of the REQUEST: it carries more values than one statement can bind.
        return Err(SqlError::InvalidRequest(format!(
            "parameters carries {} values; a statement binds at most \
             {MAX_PARAMETERS_PER_STATEMENT}",
            parameters.len()
        )));
    }
    let mut named: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for parameter in parameters {
        let name = parameter.name.as_deref().unwrap_or_default();
        if name.starts_with(':') {
            return Err(SqlError::Parameter(parameter_faults::invalid_name(name)));
        }
        if !named.insert(name) {
            return Err(SqlError::Parameter(parameter_faults::duplicate(name)));
        }
        if matches!(parameter.value.as_ref(), Some(Field::arrayValue(_))) {
            return Err(SqlError::Parameter(
                parameter_faults::ARRAYS_UNSUPPORTED.to_string(),
            ));
        }
    }
    Ok(())
}

/// Bind every placeholder of a prepared statement from `parameters`.
///
/// A named placeholder (`:name`, `@name`, `$name` — the forms SQLite accepts) takes
/// the parameter of that name; an anonymous one (`?`) takes the parameter in the
/// same position. A placeholder with no parameter is a bad request, not an
/// implicit NULL: the caller would silently get the wrong rows.
fn bind_named_parameters(
    statement: &mut rusqlite::Statement<'_>,
    parameters: &[SqlParameter],
) -> Result<(), SqlError> {
    let placeholders: Vec<Option<String>> = (1..=statement.parameter_count())
        .map(|index| statement.parameter_name(index).map(str::to_string))
        .collect();
    for (position, placeholder) in placeholders.iter().enumerate() {
        let parameter = match placeholder {
            Some(placeholder) => {
                let name = placeholder.trim_start_matches([':', '@', '$']);
                parameters
                    .iter()
                    .find(|parameter| {
                        parameter
                            .name
                            .as_deref()
                            .is_some_and(|candidate| candidate == name)
                    })
                    .ok_or_else(|| SqlError::Parameter(parameter_faults::cannot_find(name)))?
            }
            None => parameters.get(position).ok_or_else(|| {
                SqlError::Parameter(parameter_faults::cannot_find(&(position + 1).to_string()))
            })?,
        };
        statement
            .raw_bind_parameter(position + 1, sql_value_for(parameter)?)
            .map_err(SqlError::from_engine)?;
    }
    Ok(())
}

/// The value a `SqlParameter` carries, as the engine's own value type.
///
/// The `Field` member the caller used picks the type that is stored: `isNull(true)`
/// a NULL, `booleanValue` a boolean, `longValue` an integer, `doubleValue` a real,
/// `stringValue` text, `blobValue` binary; a parameter with no `value` at all is a
/// NULL. `arrayValue` is refused — the model's own `parameters` doc says "Array
/// parameters are not supported" — and so is `isNull(false)`: the member says "a
/// NULL value", so `false` names no value at all, and binding NULL for it would be
/// a guess at what the caller meant.
///
/// A `typeHint` qualifies a `stringValue` — the model documents each hint as what
/// "the corresponding `String` parameter value is sent as" — so it is applied by
/// [`hinted_text`] on that member and ignored on every other, where the member
/// itself already names the type.
fn sql_value_for(parameter: &SqlParameter) -> Result<SqlValue, SqlError> {
    Ok(match parameter.value.as_ref() {
        None | Some(Field::isNull(true)) => SqlValue::Null,
        Some(Field::isNull(false)) => {
            return Err(SqlError::Parameter(format!(
                "parameter {} sets isNull to false, which names no value; send the value \
                 in its own member, or isNull true for a NULL",
                parameter.name.as_deref().unwrap_or("")
            )))
        }
        Some(Field::booleanValue(value)) => SqlValue::Integer(i64::from(*value)),
        Some(Field::longValue(value)) => SqlValue::Integer(*value),
        Some(Field::doubleValue(value)) => SqlValue::Real(*value),
        Some(Field::stringValue(value)) => match parameter.type_hint.as_deref() {
            Some(hint) => hinted_text(parameter.name.as_deref().unwrap_or(""), hint, value)?,
            None => SqlValue::Text(value.clone()),
        },
        Some(Field::blobValue(value)) => SqlValue::Blob(value.clone()),
        // Refused by [`require_bindable_parameters`] before the statement is prepared;
        // this arm is what makes the union exhaustive.
        Some(Field::arrayValue(_)) => {
            return Err(SqlError::Parameter(
                parameter_faults::ARRAYS_UNSUPPORTED.to_string(),
            ))
        }
    })
}

/// A `stringValue` qualified by a `typeHint`, as the engine's own value.
///
/// The model documents the hint as the object type the string is sent to the
/// database AS, together with the format each one accepts: `DATE` is
/// `YYYY-MM-DD`, `TIME` is `HH:MM:SS[.FFF]`, `TIMESTAMP` is
/// `YYYY-MM-DD HH:MM:SS[.FFF]`, and `DECIMAL`, `JSON` and `UUID` are a decimal
/// number, a JSON document and a UUID
/// (`smithy_query(query="shape", shape="SqlParameter")`, member `typeHint`;
/// the value list is `TypeHint`). So the hint is what the request is CHECKED
/// against: a string that is not the form its hint names is refused, naming the
/// parameter and the accepted form, rather than silently stored under a type it
/// is not.
///
/// And the hint is what the value is SENT AS, which is not always the string the
/// request spelled it with: a value of the hint's own type has one form, and the
/// request's spelling of it is only one of the spellings that name it. So what reaches
/// the column is that type's own value —
///
/// * a JSON document, whose members a document type holds in ITS order and not in the
///   order a request happened to write them ([`json_document`]);
/// * a time of day at the precision the hint's documented format carries, which is
///   thousandths of a second ([`time_of_day`]);
///
/// — and the other four, whose documented format admits exactly one spelling of each
/// value, as they were written.
///
/// Every one of the six is then bound as TEXT, because that is how this engine
/// represents each of these types — it has no DATE, TIME, TIMESTAMP, DECIMAL,
/// JSON or UUID storage class of its own — and what a later read returns is
/// decided by the COLUMN's declared type, not by the hint (see [`field_for`]).
fn hinted_text(name: &str, hint: &str, value: &str) -> Result<SqlValue, SqlError> {
    let cannot_parse = |hint: &str| Err(cannot_parse_parameter(hint, value));
    let sent: Result<String, String> = match hint.to_ascii_uppercase().as_str() {
        "DATE" => is_date(value)
            .then(|| value.to_string())
            .map_or_else(|| cannot_parse("DATE"), Ok),
        "TIME" => time_of_day(value).map_or_else(|| cannot_parse("TIME"), Ok),
        "TIMESTAMP" => is_timestamp(value)
            .then(|| value.to_string())
            .map_or_else(|| cannot_parse("TIMESTAMP"), Ok),
        "DECIMAL" => is_decimal(value)
            .then(|| value.to_string())
            .map_or_else(|| cannot_parse("DECIMAL"), Ok),
        // A document is read by a PARSER, which says what is wrong with a string that
        // is not one — the detail the type's own input error carries.
        "JSON" => serde_json::from_str::<serde_json::Value>(value)
            .map(|parsed| as_stored_document(&parsed).to_string())
            .map_err(|error| invalid_json_input(&error)),
        "UUID" => uuid::Uuid::parse_str(value)
            .is_ok()
            .then(|| value.to_string())
            .map_or_else(|| cannot_parse("UUID"), Ok),
        // The model declares `TypeHint` as an enum of exactly those six, so a
        // seventh value is a request this service cannot act on rather than a
        // hint it may drop.
        _ => {
            return Err(SqlError::Parameter(format!(
                "parameter {name} carries typeHint {hint}, which is not one of DATE, \
                 DECIMAL, JSON, TIME, TIMESTAMP or UUID"
            )))
        }
    };
    sent.map(SqlValue::Text).map_err(SqlError::Engine)
}

/// What the service tells a caller whose hinted string it cannot read as a value of the
/// type the hint named: the hint as the model declares it, and the string the request
/// sent.
///
/// A failure to READ the value the request sent, so it is reported as the data plane's
/// own error and not as a fault of the call's shape: the request named a type the model
/// has and sent a string the model carries.
fn cannot_parse_parameter(hint: &str, value: &str) -> String {
    format!("Cannot parse {hint} parameter: \"{value}\"")
}

/// And what it tells a caller whose JSON parameter is not a document: the type the
/// string was sent as, and what the PARSER said was wrong with it, which is the detail
/// a document type's own input error carries.
fn invalid_json_input(error: &serde_json::Error) -> String {
    format!("invalid input syntax for type json: {error}")
}

/// The time of day a `TIME` parameter names, in the form the hint's documented format
/// gives it: `HH:MM:SS`, and with the fraction at the THOUSANDTHS the format's `.FFF`
/// carries for a value that has one, so `.5` and `.500` are the one value they both
/// name. `None` for a string that is no time of day at all.
///
/// Parsed and rendered by the `time` crate, so no clock arithmetic is written here.
fn time_of_day(value: &str) -> Option<String> {
    if time::Time::parse(value, TIME_FORMAT).is_ok() {
        return Some(value.to_string());
    }
    time::Time::parse(value, TIME_WITH_SUBSECOND_FORMAT)
        .ok()?
        .format(TIME_WITH_MILLISECONDS_FORMAT)
        .ok()
}

/// The document a `JSON` parameter carries, in the form a document type holds it:
/// every object's members IN NAME ORDER, and none of the whitespace a request wrote
/// between them. `None` for a string that is not a JSON document.
///
/// A document type stores a DOCUMENT and not the text of one, so one document has one
/// form however a request wrote it, and what a later read returns is that form — which
/// is why a round trip through such a column answers the request's own members in the
/// type's order rather than the request's.
fn json_document(value: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(value).ok()?;
    Some(as_stored_document(&parsed).to_string())
}

/// The same document with every object's members in the order of their names.
///
/// Bounded by the parse that produced the document, whose own nesting limit is what
/// bounds this walk.
fn as_stored_document(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(members) => {
            let mut ordered: Vec<(&String, &serde_json::Value)> = members.iter().collect();
            ordered.sort_by_key(|(name, _)| *name);
            serde_json::Value::Object(
                ordered
                    .into_iter()
                    .map(|(name, member)| (name.clone(), as_stored_document(member)))
                    .collect(),
            )
        }
        serde_json::Value::Array(elements) => {
            serde_json::Value::Array(elements.iter().map(as_stored_document).collect())
        }
        settled => settled.clone(),
    }
}

/// `YYYY-MM-DD`, parsed as a CALENDAR date by the `time` crate, so only a date
/// the calendar really has is accepted (29 February in a leap year, not in a
/// common one). No date arithmetic is written here.
fn is_date(value: &str) -> bool {
    time::Date::parse(value, DATE_FORMAT).is_ok()
}

/// `YYYY-MM-DD HH:MM:SS[.FFF]`: the date and the time in one value, checked as a
/// whole by the same crate.
fn is_timestamp(value: &str) -> bool {
    time::PrimitiveDateTime::parse(value, TIMESTAMP_FORMAT).is_ok()
        || time::PrimitiveDateTime::parse(value, TIMESTAMP_WITH_SUBSECOND_FORMAT).is_ok()
}

/// The accepted format of each date-and-time `typeHint`, written exactly as the
/// model documents it.
const DATE_FORMAT: &[time::format_description::FormatItem<'static>] =
    time::macros::format_description!("[year]-[month]-[day]");
const TIME_FORMAT: &[time::format_description::FormatItem<'static>] =
    time::macros::format_description!("[hour]:[minute]:[second]");
const TIME_WITH_SUBSECOND_FORMAT: &[time::format_description::FormatItem<'static>] =
    time::macros::format_description!("[hour]:[minute]:[second].[subsecond digits:1+]");
/// And the form a time of day is SENT in: the same format with the three fractional
/// digits its `.FFF` names.
const TIME_WITH_MILLISECONDS_FORMAT: &[time::format_description::FormatItem<'static>] =
    time::macros::format_description!("[hour]:[minute]:[second].[subsecond digits:3]");
const TIMESTAMP_FORMAT: &[time::format_description::FormatItem<'static>] =
    time::macros::format_description!("[year]-[month]-[day] [hour]:[minute]:[second]");
const TIMESTAMP_WITH_SUBSECOND_FORMAT: &[time::format_description::FormatItem<'static>] =
    time::macros::format_description!(
        "[year]-[month]-[day] [hour]:[minute]:[second].[subsecond digits:1+]"
    );

/// A decimal number: an optional sign, decimal digits, and at most one
/// fractional part. No exponent — the model's `DECIMAL` names an exact decimal,
/// which `1e3` is a float's spelling of.
fn is_decimal(value: &str) -> bool {
    let unsigned = value.strip_prefix(['+', '-']).unwrap_or(value);
    let (whole, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    if unsigned.contains('.') && fraction.is_empty() {
        return false;
    }
    !(whole.is_empty() && fraction.is_empty())
        && whole
            .bytes()
            .chain(fraction.bytes())
            .all(|byte| byte.is_ascii_digit())
}

/// The `Field` member a stored value is returned in.
///
/// The rule: NULL is `isNull`, and otherwise the COLUMN's PostgreSQL type
/// decides — a boolean is `booleanValue`, an exact number follows
/// `decimalReturnType` (the model documents `STRING` as its default, and
/// `DOUBLE_OR_LONG` as "a Long value if its scale is 0, or a Double value
/// otherwise"), an integer follows `longReturnType` (`LONG` by default, `STRING`
/// on request) and a float is `doubleValue`. For a type whose values the engine
/// stores as they come (text, binary, a date or a time, a type the catalog does
/// not name) the storage class the engine holds the value in decides: real to
/// `doubleValue`, text to `stringValue`, binary to `blobValue`.
pub fn field_for(
    value: &SqlValue,
    column: &ColumnInfo,
    returns: ReturnTypes,
) -> Result<Field, SqlError> {
    Ok(match value {
        SqlValue::Null => Field::isNull(true),
        // A non-finite number is no value of any of the union's members: `isNull` would
        // tell the caller the value is missing and a number member cannot carry it, so
        // the result is the one this call cannot return, in the modelled refusal's own
        // words.
        SqlValue::Real(number) if !number.is_finite() => {
            return Err(SqlError::UnsupportedResult(
                postgres::NON_FINITE_FLOAT.to_string(),
            ))
        }
        SqlValue::Integer(number) => {
            if column.is_boolean() {
                Field::booleanValue(*number != 0)
            } else if column.is_decimal() {
                if returns.decimal_as_number {
                    // The same rule the Real arm below follows, because it is the
                    // COLUMN's declared scale that decides, not how the engine
                    // happens to hold the value: "a Long value if its scale is 0,
                    // or a Double value otherwise".
                    if column.pg.scale == 0 {
                        Field::longValue(*number)
                    } else {
                        Field::doubleValue(*number as f64)
                    }
                } else {
                    Field::stringValue(exact_integer_text(*number, column))
                }
            } else if column.pg.class == TypeClass::Float {
                // A float column holding a whole number is still a float.
                Field::doubleValue(*number as f64)
            } else if returns.long_as_string {
                Field::stringValue(number.to_string())
            } else {
                Field::longValue(*number)
            }
        }
        SqlValue::Real(number) => {
            if column.is_decimal() {
                if returns.decimal_as_number {
                    // "a Long value if its scale is 0, or a Double value otherwise",
                    // and a value no integer can hold stays a double.
                    match (column.pg.scale == 0, in_integer_range(*number)) {
                        (true, true) => Field::longValue(*number as i64),
                        _ => Field::doubleValue(*number),
                    }
                } else {
                    Field::stringValue(exact_text(*number, column))
                }
            } else {
                Field::doubleValue(*number)
            }
        }
        SqlValue::Text(text) if column.pg.class == TypeClass::Array => {
            Field::arrayValue(array_value(text, &column.pg)?)
        }
        // A document column holds the DOCUMENT a value names, so what comes back is
        // that document written out and not the text that named it.
        SqlValue::Text(text) if column.pg.document => Field::stringValue(document_text(text)),
        // And a date-and-time column holds an INSTANT, so what comes back is that
        // instant in the form the database displays it.
        SqlValue::Text(text) if column.pg.class == TypeClass::Temporal => {
            Field::stringValue(temporal_text(text, &column.pg))
        }
        SqlValue::Text(text) => Field::stringValue(text.clone()),
        SqlValue::Blob(bytes) => Field::blobValue(bytes.clone()),
    })
}

/// One array value, as the `ArrayValue` union carries it: the elements of the
/// array the engine holds as text, each in the member the array's ELEMENT type
/// names.
///
/// The text is read in either of the two forms an array value reaches the engine
/// in: the JSON document this data plane builds for an array constructor, and
/// PostgreSQL's own braced literal, which a caller can store as text.
///
/// `Err` for a result the union cannot carry: an array of arrays (the
/// multidimensional array the model names), a value that is not an array at all,
/// and an element the modelled list has no room for — `booleanValues`,
/// `longValues`, `doubleValues` and `stringValues` are generated here as
/// `Vec<bool>`, `Vec<i64>`, `Vec<f64>` and `Vec<String>` (see
/// `ops/types/shared_types.rs`), which hold a value per element and nothing
/// else, so an array carrying a NULL among its elements is refused rather than
/// returned with that element silently dropped.
fn array_value(text: &str, pg: &PgType) -> Result<ArrayValue, SqlError> {
    let unsupported = || unsupported_type(pg);
    let elements = read_array(text).ok_or_else(unsupported)?;
    if holds_an_array(&elements) {
        return Err(multidimensional_array());
    }
    let element = pg.element().ok_or_else(unsupported)?;
    Ok(match element.class {
        TypeClass::Boolean => ArrayValue::booleanValues(
            elements
                .iter()
                .map(boolean_element)
                .collect::<Option<Vec<bool>>>()
                .ok_or_else(unsupported)?,
        ),
        TypeClass::Integer => ArrayValue::longValues(
            elements
                .iter()
                .map(integer_element)
                .collect::<Option<Vec<i64>>>()
                .ok_or_else(unsupported)?,
        ),
        TypeClass::Float => ArrayValue::doubleValues(
            elements
                .iter()
                .map(float_element)
                .collect::<Option<Vec<f64>>>()
                .ok_or_else(unsupported)?,
        ),
        TypeClass::Decimal | TypeClass::Text | TypeClass::Temporal => ArrayValue::stringValues(
            elements
                .iter()
                .map(text_element)
                .collect::<Option<Vec<String>>>()
                .ok_or_else(unsupported)?,
        ),
        // Binary has no array member of its own, and an array of arrays was
        // refused above.
        TypeClass::Binary | TypeClass::Array | TypeClass::Other => return Err(unsupported()),
    })
}

/// The elements of an array value the engine holds as text: the JSON document
/// this data plane builds for an array constructor, or PostgreSQL's braced array
/// literal. `None` when the text is neither.
fn read_array(text: &str) -> Option<Vec<serde_json::Value>> {
    let text = text.trim();
    if text.starts_with('[') {
        return match serde_json::from_str(text) {
            Ok(serde_json::Value::Array(elements)) => Some(elements),
            _ => None,
        };
    }
    read_braced_array(&mut text.chars().peekable(), 0)
}

/// PostgreSQL's braced array literal (`{1,2}`, `{"a","b"}`, `{NULL}`,
/// `{{1,2},{3,4}}`), as the elements it holds: a quoted or bare element is its
/// text, `NULL` is a null, and a brace opens a nested array, which the caller
/// refuses as multidimensional.
///
/// Bounded by [`MAX_ARRAY_DEPTH`], so a literal cannot nest without limit.
fn read_braced_array(
    characters: &mut std::iter::Peekable<std::str::Chars<'_>>,
    depth: usize,
) -> Option<Vec<serde_json::Value>> {
    if depth > MAX_ARRAY_DEPTH || characters.next() != Some('{') {
        return None;
    }
    let mut elements = Vec::new();
    loop {
        // Whatever comes next: the end of the array, a separator, a nested
        // array, a quoted element or a bare one.
        while characters.peek().is_some_and(|c| c.is_whitespace()) {
            characters.next();
        }
        match characters.peek() {
            None => return None,
            Some('}') => {
                characters.next();
                return Some(elements);
            }
            Some(',') => {
                characters.next();
            }
            Some('{') => elements.push(serde_json::Value::Array(read_braced_array(
                characters,
                depth + 1,
            )?)),
            Some('"') => {
                characters.next();
                let mut quoted = String::new();
                loop {
                    match characters.next()? {
                        '\\' => quoted.push(characters.next()?),
                        '"' => break,
                        character => quoted.push(character),
                    }
                }
                elements.push(serde_json::Value::String(quoted));
            }
            Some(_) => {
                let mut bare = String::new();
                while characters
                    .peek()
                    .is_some_and(|c| *c != ',' && *c != '}' && *c != '{')
                {
                    bare.push(characters.next()?);
                }
                let bare = bare.trim();
                elements.push(if bare.eq_ignore_ascii_case("NULL") {
                    serde_json::Value::Null
                } else {
                    serde_json::Value::String(bare.to_string())
                });
            }
        }
    }
}

/// How deeply an array literal may nest. An array of arrays is already a result
/// this service cannot return, so this only has to be deep enough to SEE that
/// nesting.
const MAX_ARRAY_DEPTH: usize = 2;

/// One element of an array as a boolean: the engine's own `true`/`false`, or the
/// text PostgreSQL writes a boolean as.
fn boolean_element(element: &serde_json::Value) -> Option<bool> {
    match element {
        serde_json::Value::Bool(value) => Some(*value),
        serde_json::Value::Number(number) => Some(number.as_f64().is_some_and(|n| n != 0.0)),
        serde_json::Value::String(text) => match text.to_ascii_lowercase().as_str() {
            "t" | "true" | "yes" | "on" | "1" => Some(true),
            "f" | "false" | "no" | "off" | "0" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// One element of an array as an integer.
fn integer_element(element: &serde_json::Value) -> Option<i64> {
    match element {
        serde_json::Value::Number(number) => number.as_i64(),
        serde_json::Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

/// One element of an array as a floating-point number.
fn float_element(element: &serde_json::Value) -> Option<f64> {
    match element {
        serde_json::Value::Number(number) => number.as_f64(),
        serde_json::Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

/// One element of an array as text.
fn text_element(element: &serde_json::Value) -> Option<String> {
    match element {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Number(number) => Some(number.to_string()),
        serde_json::Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

/// A fixed-point value as text, at the column's declared scale — clamped to
/// [`MAX_DECIMAL_SCALE`], so a declared scale cannot size an allocation.
fn decimal_text(number: f64, scale: i32) -> String {
    let places = decimal_places(scale);
    format!("{number:.places$}")
}

/// An exact number the engine holds as an INTEGER, as the text an exact number is
/// reported as.
///
/// Built from the integer itself, never through a float, so a value beyond a float's
/// 53 significant bits keeps every digit. A type that declared no scale carries each
/// value's own, so the value keeps the digits it has instead of being padded to a
/// scale nobody declared.
fn exact_integer_text(number: i64, column: &ColumnInfo) -> String {
    match column.pg.declared_scale {
        Some(scale) => decimal_text_of_integer(number, scale),
        None => number.to_string(),
    }
}

/// An exact number the engine holds as a REAL, as that same text: at the scale the
/// column's declaration gave, and otherwise with the digits the value has.
fn exact_text(number: f64, column: &ColumnInfo) -> String {
    match column.pg.declared_scale {
        Some(scale) => decimal_text(number, scale),
        None => number.to_string(),
    }
}

/// An integer held in a fixed-point column as text, at the column's declared
/// scale. The integer is exact, so its digits are kept and the scale only decides
/// how many zeros follow the point.
fn decimal_text_of_integer(number: i64, scale: i32) -> String {
    let places = decimal_places(scale);
    if places == 0 {
        return number.to_string();
    }
    format!("{number}.{}", "0".repeat(places))
}

/// How many fractional digits a declared scale asks for, bounded.
fn decimal_places(scale: i32) -> usize {
    usize::try_from(scale.clamp(0, MAX_DECIMAL_SCALE)).unwrap_or(0)
}

/// Whether a float is a value an `i64` can hold exactly enough to report as one.
fn in_integer_range(number: f64) -> bool {
    number.is_finite() && number >= -(2_f64.powi(63)) && number < 2_f64.powi(63)
}

/// What the modelled `UnsupportedResultException` says about a result carrying a
/// value of a type it has no form for — the model's own wording for the
/// condition: "It contained an unsupported data type".
fn unsupported_type(pg: &PgType) -> SqlError {
    SqlError::UnsupportedResult(format!(
        "The result contains the unsupported data type {}.",
        pg.name.to_ascii_uppercase()
    ))
}

/// The same exception for the OTHER result the model names: "It contained a
/// multidimensional array". Worded once, in the type catalog, so a nested array
/// is refused in the same words whichever form the result set was asked for.
fn multidimensional_array() -> SqlError {
    SqlError::UnsupportedResult(postgres::MULTIDIMENSIONAL_ARRAY.to_string())
}

/// Whether an array value holds an array among its elements, which is the
/// multidimensional array the model names.
fn holds_an_array(elements: &[serde_json::Value]) -> bool {
    elements
        .iter()
        .any(|element| matches!(element, serde_json::Value::Array(_)))
}

/// One value inside a `formatRecordsAs=JSON` result set.
///
/// The rule: a value is rendered in the JSON form of the `Field` member its
/// COLUMN's type names, so a client reading the JSON string sees the same value,
/// of the same JSON type, as it would in `records`. The `Field` union models
/// `booleanValue` as a boolean and `blobValue` as a blob — which this protocol
/// carries base64-encoded — so a boolean column is one of the JSON literals
/// `true`/`false` and a binary column is its base64 text; an EXACT number is a JSON
/// string, as `records` reports it, because neither form turns an exact number into a
/// float; an inexact one is a JSON number, text and a date or time are JSON strings, a
/// NULL is `null`, and an array column is a JSON array whose elements take the form
/// their ELEMENT type names (see [`json_element`]).
fn json_for(value: &SqlValue, column: &ColumnInfo) -> Result<serde_json::Value, SqlError> {
    Ok(match value {
        SqlValue::Null => serde_json::Value::Null,
        // JSON has no form for a non-finite number, and neither has the `Field` union:
        // such a value is the result this call cannot return, stated the way the
        // modelled refusal states it, rather than reported as NULL, which would tell
        // the caller the value is missing.
        SqlValue::Real(number) if !number.is_finite() => {
            return Err(SqlError::UnsupportedResult(
                postgres::NON_FINITE_FLOAT.to_string(),
            ))
        }
        SqlValue::Integer(number) => {
            if column.is_boolean() {
                serde_json::Value::Bool(*number != 0)
            } else if column.is_decimal() {
                serde_json::Value::String(exact_integer_text(*number, column))
            } else {
                serde_json::Value::from(*number)
            }
        }
        // An exact number keeps its digits and its scale, which is what the text form
        // is for.
        SqlValue::Real(number) if column.is_decimal() => {
            serde_json::Value::String(exact_text(*number, column))
        }
        SqlValue::Real(number) => serde_json::Number::from_f64(*number)
            .map(serde_json::Value::Number)
            .ok_or_else(|| {
                SqlError::UnsupportedResult(postgres::NON_FINITE_FLOAT.to_string())
            })?,
        // An array column's value is the list of its elements, not the text the
        // engine holds that list as.
        SqlValue::Text(text) if column.pg.class == TypeClass::Array => {
            let elements = read_array(text).ok_or_else(|| unsupported_type(&column.pg))?;
            if holds_an_array(&elements) {
                return Err(multidimensional_array());
            }
            let element_type = column.pg.element().ok_or_else(|| unsupported_type(&column.pg))?;
            serde_json::Value::Array(
                elements
                    .iter()
                    .map(|element| json_element(element, element_type))
                    .collect::<Option<Vec<serde_json::Value>>>()
                    .ok_or_else(|| unsupported_type(&column.pg))?,
            )
        }
        SqlValue::Text(text) if column.pg.document => {
            serde_json::Value::String(document_text(text))
        }
        SqlValue::Text(text) if column.pg.class == TypeClass::Temporal => {
            serde_json::Value::String(temporal_text(text, &column.pg))
        }
        SqlValue::Text(text) => serde_json::Value::String(text.clone()),
        // A blob is what the `Field` union's `blobValue` carries, and this
        // protocol carries a blob as its base64 text — so that is the text a
        // binary column is rendered as here too, and a client that base64-decodes
        // it gets the bytes that are stored. Coded by the same standard-alphabet
        // codec the protocol itself uses, so the two cannot disagree.
        SqlValue::Blob(bytes) => {
            serde_json::Value::String(::aws_smithy_types::base64::encode(bytes))
        }
    })
}

/// One element of an array column inside a `formatRecordsAs=JSON` result set: the
/// JSON form the array's ELEMENT type names, read off the element the engine
/// holds — the same element-type mapping [`array_value`] uses to choose the
/// `ArrayValue` list an element comes back in, with the one difference JSON
/// itself makes: a NULL element has a JSON form where the union's lists have no room
/// for one.
///
/// `None` for an element whose type has no form of its own: an array of arrays,
/// or an array of a type the `Field` union cannot carry.
fn json_element(element: &serde_json::Value, element_type: &PgType) -> Option<serde_json::Value> {
    // JSON has a form for a missing value, so an array's NULL element is reported
    // as that form whatever the element type is. It is the `ArrayValue` union's
    // lists that have no room for one.
    if element.is_null() {
        return Some(serde_json::Value::Null);
    }
    match element_type.class {
        TypeClass::Boolean => boolean_element(element).map(serde_json::Value::Bool),
        TypeClass::Integer => integer_element(element).map(serde_json::Value::from),
        // An inexact number is a JSON number, as a scalar one is above; a value JSON
        // cannot represent is refused rather than reported as something else.
        TypeClass::Float => float_element(element)
            .and_then(serde_json::Number::from_f64)
            .map(serde_json::Value::Number),
        // And an exact one is the text of its own digits, which is what both forms
        // report an exact number as.
        TypeClass::Decimal | TypeClass::Text | TypeClass::Temporal => {
            text_element(element).map(serde_json::Value::String)
        }
        TypeClass::Binary | TypeClass::Array | TypeClass::Other => None,
    }
}

/// The document a document column holds, written out: the form [`json_document`]
/// gives it, and the text as it is for a value that is no document at all — which the
/// engine has no form of its own for either.
fn document_text(text: &str) -> String {
    json_document(text).unwrap_or_else(|| text.to_string())
}

/// The instant a date-and-time column holds, in the form the DATABASE displays that
/// type in: `YYYY-MM-DD` for a date, `HH:MM:SS` for a time of day, the two separated
/// by a SPACE for a timestamp — and with no zone designator, which is the zone a Data
/// API caller does not get back.
///
/// A value of such a type can be WRITTEN in more forms than one (ISO 8601's `T`
/// separator and its `Z`, an offset, a space), and what the column holds is the instant
/// they all name rather than the spelling one caller chose; a zone the value carries is
/// read as that zone and displayed in UTC, which is the session's. The FRACTION of a
/// second is kept as the value carries it, because that is the precision the value was
/// given at.
///
/// Parsed by the `time` crate, so no calendar or clock arithmetic is written here; text
/// that is no date or time at all is returned as it is, which is what the engine holds.
fn temporal_text(text: &str, pg: &PgType) -> String {
    match pg.name.as_str() {
        "date" => time::Date::parse(text, DATE_FORMAT)
            .ok()
            .and_then(|date| date.format(DATE_FORMAT).ok()),
        "time" | "timetz" => time_text(text),
        _ => timestamp_text(text),
    }
    .unwrap_or_else(|| text.to_string())
}

/// A time of day in the form the database displays it, read in either of the forms it
/// is written in — with a fraction of a second and without.
fn time_text(text: &str) -> Option<String> {
    let clock = time::Time::parse(text, TIME_FORMAT)
        .or_else(|_| time::Time::parse(text, TIME_WITH_SUBSECOND_FORMAT))
        .ok()?;
    let displayed = clock.format(TIME_FORMAT).ok()?;
    Some(format!("{displayed}{}", written_fraction(text)))
}

/// A timestamp in the form the database displays it, read in any of the forms it is
/// WRITTEN in: with the date and the time separated by a space or by ISO 8601's `T`,
/// with or without a zone (`Z` or an offset), and with or without a fraction of a
/// second.
fn timestamp_text(text: &str) -> Option<String> {
    let displayed = timestamp_instant(text)?.format(TIMESTAMP_FORMAT).ok()?;
    Some(format!("{displayed}{}", written_fraction(text)))
}

/// The instant a timestamp's text names, with a zone it carries read as that zone and
/// answered in the session's, which is UTC.
fn timestamp_instant(text: &str) -> Option<time::PrimitiveDateTime> {
    for description in TIMESTAMP_INPUT_FORMATS {
        if let Ok(parsed) = time::PrimitiveDateTime::parse(text, description) {
            return Some(parsed);
        }
    }
    let zoned = ZONED_TIMESTAMP_INPUT_FORMATS
        .iter()
        .find_map(|description| time::OffsetDateTime::parse(text, description).ok())
        .or_else(|| {
            time::OffsetDateTime::parse(text, &time::format_description::well_known::Rfc3339).ok()
        })?;
    let utc = zoned.to_offset(time::UtcOffset::UTC);
    Some(time::PrimitiveDateTime::new(utc.date(), utc.time()))
}

/// The forms a timestamp carrying NO zone is written in: the separator is a space or
/// ISO 8601's `T`, and the fraction of a second is optional.
const TIMESTAMP_INPUT_FORMATS: &[&[time::format_description::FormatItem<'static>]] = &[
    TIMESTAMP_FORMAT,
    TIMESTAMP_WITH_SUBSECOND_FORMAT,
    time::macros::format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]"),
    time::macros::format_description!(
        "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:1+]"
    ),
];

/// And the forms one carrying a zone as an OFFSET is written in, which is how
/// PostgreSQL itself displays such a value (`+00`, `+02:00`). ISO 8601's `T` form,
/// including its `Z`, is read by the `time` crate's own well-known description.
const ZONED_TIMESTAMP_INPUT_FORMATS: &[&[time::format_description::FormatItem<'static>]] = &[
    time::macros::format_description!(
        "[year]-[month]-[day] [hour]:[minute]:[second][offset_hour sign:mandatory]"
    ),
    time::macros::format_description!(
        "[year]-[month]-[day] [hour]:[minute]:[second][offset_hour sign:mandatory]:[offset_minute]"
    ),
    time::macros::format_description!(
        "[year]-[month]-[day] [hour]:[minute]:[second].[subsecond digits:1+]\
         [offset_hour sign:mandatory]"
    ),
    time::macros::format_description!(
        "[year]-[month]-[day] [hour]:[minute]:[second].[subsecond digits:1+]\
         [offset_hour sign:mandatory]:[offset_minute]"
    ),
];

/// The fraction of a second a date-and-time value's text carries, with its point, and
/// empty for a value written without one.
fn written_fraction(text: &str) -> String {
    let Some((_, rest)) = text.split_once('.') else {
        return String::new();
    };
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        String::new()
    } else {
        format!(".{digits}")
    }
}

/// An owned copy of a value the engine handed back.
fn owned_value(value: ValueRef<'_>) -> SqlValue {
    match value {
        ValueRef::Null => SqlValue::Null,
        ValueRef::Integer(number) => SqlValue::Integer(number),
        ValueRef::Real(number) => SqlValue::Real(number),
        ValueRef::Text(bytes) => SqlValue::Text(String::from_utf8_lossy(bytes).into_owned()),
        ValueRef::Blob(bytes) => SqlValue::Blob(bytes.to_vec()),
    }
}

/// Roughly what one value costs in the response, for the 1 MB cap. Read off the
/// value the ENGINE holds, so the cap is applied before anything is copied, and
/// charged the per-value storage the response keeps as well as the payload, so a
/// result set of cheap values is bounded by what it really occupies.
fn value_bytes(value: &ValueRef<'_>) -> usize {
    let payload = match value {
        ValueRef::Null | ValueRef::Integer(_) | ValueRef::Real(_) => 0,
        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => bytes.len(),
    };
    payload + std::mem::size_of::<SqlValue>()
}

/// And what one ROW costs beside its values: the container the response keeps it in,
/// and this row's own place in the list of them. Charged per row, so the ROW COUNT a
/// statement can return is bounded by the same cap as the bytes in them — a result set
/// of empty values cannot grow this process by its length.
fn row_bytes(columns: usize) -> usize {
    std::mem::size_of::<Vec<SqlValue>>() * 2 + columns * std::mem::size_of::<SqlValue>()
}

/// Give a result column whose type nothing has named — a computed column the
/// statement's own text does not type — the PostgreSQL type of the storage class
/// its values actually arrived in, so `typeName` still describes the column.
fn name_undeclared_types(columns: &mut [ColumnInfo], rows: &[Vec<SqlValue>]) {
    for (index, column) in columns.iter_mut().enumerate() {
        if column.named_type {
            continue;
        }
        let storage_class = rows
            .iter()
            .filter_map(|row| row.get(index))
            .find(|value| !matches!(value, SqlValue::Null))
            .map(|value| match value {
                SqlValue::Integer(_) => "INTEGER",
                SqlValue::Real(_) => "REAL",
                SqlValue::Text(_) => "TEXT",
                SqlValue::Blob(_) => "BLOB",
                SqlValue::Null => "NULL",
            });
        column.pg = postgres::type_of_storage_class(storage_class);
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsRdsdataRdsdataserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsRdsdataRdsdataserviceState>)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A heap ceiling low enough that a statement writing a few megabytes of pages
    /// runs into it, and high enough that the engine can still prepare statements
    /// (so the ceiling can be put back afterwards).
    const PROBE_HEAP_BYTES: i64 = 16 * 1024 * 1024;

    /// Half a megabyte per row, under [`MAX_VALUE_BYTES`], so what the statements
    /// below run into is the HEAP ceiling and not the per-value limit.
    const PROBE_VALUE_BYTES: i64 = 512 * 1024;

    /// Enough rows to need four times [`PROBE_HEAP_BYTES`] of pages.
    const PROBE_ROWS: usize = 128;

    /// Set SQLite's process-wide heap ceiling, through a connection of this test's
    /// own: a confined [`Database`] refuses `PRAGMA` to anything, including this
    /// crate, once its authorizer is installed.
    fn set_process_heap_ceiling(bytes: i64) {
        let probe = Connection::open_in_memory().expect("a connection for the pragma");
        probe
            .pragma_update(None, "hard_heap_limit", bytes)
            .expect("the engine takes a heap ceiling");
    }

    /// `PRAGMA hard_heap_limit` bounds the ENGINE's allocator, so a statement whose
    /// pages would take the engine past the ceiling is REFUSED by the engine
    /// instead of growing this process — the memory axis the page, step, time and
    /// output bounds do not cover.
    ///
    /// SQLite's heap limit is process-wide, which is why this lives beside the code
    /// rather than in `tests/all/`: lowering it is visible to every database in the
    /// process, so the window is held open for one statement and closed again,
    /// where the wire suite's databases (which deliberately fill tens of megabytes)
    /// cannot see it.
    #[test]
    fn the_engine_heap_ceiling_refuses_an_allocation_heavy_statement() {
        let mut database = Database::open().expect("a confined database opens");
        database
            .run_within(
                &mut RequestBudget::new(),
                "CREATE TABLE blobs (payload BLOB)",
                &[],
            )
            .expect("the table is created under the configured ceiling");

        // The ceiling is the only thing that changes for the length of this loop.
        set_process_heap_ceiling(PROBE_HEAP_BYTES);
        let mut refused = None;
        let mut rows_written = 0_usize;
        for _ in 0..PROBE_ROWS {
            match database.run_within(
                &mut RequestBudget::new(),
                &format!("INSERT INTO blobs (payload) VALUES (zeroblob({PROBE_VALUE_BYTES}))"),
                &[],
            ) {
                Ok(_) => rows_written += 1,
                Err(error) => {
                    refused = Some(error);
                    break;
                }
            }
        }
        set_process_heap_ceiling(MAX_ENGINE_HEAP_BYTES);

        let refused = refused.unwrap_or_else(|| {
            panic!(
                "case \"rows past the heap ceiling\": the engine accepted {rows_written} rows of \
                 {PROBE_VALUE_BYTES} bytes under a {PROBE_HEAP_BYTES}-byte ceiling, so memory is \
                 not bounded"
            )
        });
        let message = refused.message();
        assert!(
            matches!(refused, SqlError::Capacity(_)) && message.contains("memory ceiling"),
            "case \"rows past the heap ceiling\": expected the engine's out-of-memory refusal, \
             reported as this emulator's capacity, after {rows_written} rows, got \
             {refused:?} ({message})"
        );

        // The ceiling is back, so the engine serves statements again: the bound
        // refuses work, it does not break the database it was applied to.
        database
            .run_within(&mut RequestBudget::new(), "SELECT count(*) FROM blobs", &[])
            .expect("the database still answers once the ceiling is restored");
    }
}
