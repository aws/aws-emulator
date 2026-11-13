// SPDX-License-Identifier: CC0-1.0

//! The CONNECTION a submission's statements run on: what belongs to that
//! connection alone, and what it holds against the others.
//!
//! The Data API runs every statement over a database connection. A submission
//! that names a `SessionId` runs on the connection that session IS — "the
//! session is open and ready to run a SQL statement" — and one that names none
//! runs on a connection of its own, opened for it and closed when it ends. Two
//! things follow from that, and both are this module's:
//!
//! * a TEMPORARY relation lives in the temporary schema of the connection that
//!   created it, so only that connection can resolve it: a statement of the
//!   session that built one reaches it by name, and a statement on any other
//!   connection does not reach it at all. Each connection's temporary relations
//!   are therefore held under a name of its own ([`TempRelations`]), and the
//!   ones of a connection that has closed are dropped.
//! * a LOCK a statement takes inside an open transaction is held by that
//!   connection until the transaction ends, and a statement on another
//!   connection that needs a conflicting lock on the same table cannot run
//!   while it is held: it WAITS ([`await_lock_release`]), which is what makes a
//!   blocked statement observable as running and cancellable rather than
//!   finished.

use std::collections::{BTreeSet, HashMap};
use std::time::{Duration, Instant, SystemTime};

use super::{
    service_state, ComAmazonawsRedshiftdataRedshiftdataState, ExecutionBound, QueueSlot,
    MAX_RETAINED_SESSIONS,
};

/// The prefix the temporary relations of ONE connection are held under in the
/// engine, ahead of that connection's own key.
///
/// Amazon Redshift holds a session's temporary relations in a schema of that
/// session's own, which `pg_temp` names from inside it; this is the same idea,
/// spelled after that schema and the connection's key — a caller would have to
/// create a schema named for this prefix AND for a live connection's key, which
/// is a session id, to reach the same engine identifier.
const TEMP_PREFIX: &str = "pg_temp_";

/// The qualifier a statement names its OWN connection's temporary schema with
/// (PostgreSQL's `pg_temp`, which Amazon Redshift's dialect carries), so a
/// statement that qualifies a temporary relation reaches the same relation the
/// unqualified name does.
pub const TEMP_QUALIFIER: &str = "pg_temp";

/// How many temporary relations ONE connection may hold. A caller creates them
/// one statement at a time, so the set is bounded like every other map this
/// service grows on request.
const MAX_TEMP_RELATIONS: usize = 64;

/// How many explicit table locks ONE connection may hold at once — one
/// statement can ask for several, and a session holds them until its
/// transaction ends, so what one caller can accumulate is bounded. A `LOCK`
/// that would take its connection past this is REFUSED, as every other bound of
/// this service refuses the statement that reaches it, rather than reported as
/// taken and then not held.
pub const MAX_LOCKS_PER_CONNECTION: usize = 64;

/// How many locks the sessions of this emulator hold between them: the
/// per-connection bound across every session this service retains, so the
/// registry is bounded whatever the callers do. The per-connection bound is
/// what a statement is refused by, so this is the registry's own last word.
const MAX_HELD_LOCKS: usize = MAX_LOCKS_PER_CONNECTION * MAX_RETAINED_SESSIONS;

/// How long a statement waits for a lock another connection holds before it is
/// given up on.
///
/// Amazon Redshift leaves a blocked statement waiting until the holder's
/// transaction ends or the statement is cancelled, so this is purely this
/// emulator's bound: generous, so a caller polling the statement really sees it
/// running and can cancel it, and finite, so a statement whose holder never
/// commits cannot stay non-terminal for ever.
pub const MAX_LOCK_WAIT: Duration = Duration::from_secs(30);

/// How often a waiting statement looks again at the lock it is waiting for. It
/// sleeps on its own execution's task, so waiting here holds up nothing else.
const LOCK_POLL: Duration = Duration::from_millis(20);

/// The temporary relations one connection holds, and the name the engine holds
/// them under.
///
/// The names are matched and held LOWER-CASED: an unquoted identifier is
/// case-insensitive in this dialect, so `Sess_Tmp` and `sess_tmp` name one
/// relation and must resolve to one engine name.
#[derive(Debug, Clone, Copy)]
pub struct TempRelations<'a> {
    prefix: &'a str,
    relations: &'a BTreeSet<String>,
}

impl<'a> TempRelations<'a> {
    /// The relations held under `prefix`.
    pub(crate) fn new(prefix: &'a str, relations: &'a BTreeSet<String>) -> Self {
        Self { prefix, relations }
    }

    /// Whether this connection holds a temporary relation of this name.
    pub fn holds(&self, relation: &str) -> bool {
        self.relations.contains(&relation.to_ascii_lowercase())
    }

    /// The name the ENGINE holds this connection's `relation` under.
    pub fn engine_name(&self, relation: &str) -> String {
        format!("{}.{}", self.prefix, relation.to_ascii_lowercase())
    }
}

/// The connection ONE submission's statements run on.
///
/// Built by the state store for a submission it has just recorded
/// ([`ComAmazonawsRedshiftdataRedshiftdataState::connection_scope`]) and handed
/// to that submission's execution, which is the only thing that touches the
/// engine: the execution reads the connection's temporary relations from it,
/// drops the ones of connections that have closed, and reports back what it
/// created.
#[derive(Debug, Clone)]
pub struct ConnectionScope {
    /// Identifies the connection: the `SessionId` it reuses, or the
    /// submission's own `Id` when it runs on a connection of its own.
    key: String,
    /// The engine prefix this connection's temporary relations are held under.
    prefix: String,
    /// Whether the connection OUTLIVES this submission, which a session's
    /// connection does and a submission's own does not.
    session: bool,
    /// The temporary relations the connection already holds.
    temp: BTreeSet<String>,
    /// The ENGINE names of the temporary relations of connections this service
    /// no longer holds, for this execution to drop.
    retired: Vec<String>,
    /// How many locks this connection already holds, so a `LOCK` that would
    /// take it past [`MAX_LOCKS_PER_CONNECTION`] is refused while it runs
    /// rather than reported as taken and then dropped by the registry.
    locks_held: usize,
}

impl ConnectionScope {
    /// The connection with this key, holding these temporary relations.
    pub(crate) fn new(
        key: String,
        session: bool,
        temp: BTreeSet<String>,
        retired: Vec<String>,
        locks_held: usize,
    ) -> Self {
        Self {
            prefix: format!("{TEMP_PREFIX}{key}"),
            key,
            session,
            temp,
            retired,
            locks_held,
        }
    }

    /// A connection of its own, holding nothing: what work that is not a
    /// submission's (a catalog read, a submission this service no longer holds
    /// a record of) runs on.
    pub fn detached() -> Self {
        Self::new(super::new_uuid(), false, BTreeSet::new(), Vec::new(), 0)
    }

    /// How many locks this connection already holds.
    pub fn locks_held(&self) -> usize {
        self.locks_held
    }

    /// The connection's key.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Whether this connection outlives the submission running on it.
    pub fn is_session(&self) -> bool {
        self.session
    }

    /// The temporary relations it holds.
    pub fn relations(&self) -> TempRelations<'_> {
        TempRelations::new(&self.prefix, &self.temp)
    }

    /// Those relations, as an execution's own working set.
    pub(crate) fn temp_names(&self) -> BTreeSet<String> {
        self.temp.clone()
    }

    /// The engine prefix this connection's temporary relations are held under.
    pub(crate) fn prefix(&self) -> &str {
        &self.prefix
    }

    /// The ENGINE names of the relations of closed connections this execution
    /// has to drop.
    pub fn retired(&self) -> &[String] {
        &self.retired
    }
}

/// The lock modes a statement takes on a table.
///
/// The names and the spelling are the ones the `LOCK` grammar declares; Amazon
/// Redshift documents the statement as taking an `ACCESS EXCLUSIVE` lock, and
/// the PostgreSQL form it is derived from names the mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockMode {
    /// Taken by a read.
    AccessShare,
    /// Taken by `SELECT … FOR UPDATE`/`FOR SHARE`.
    RowShare,
    /// Taken by `INSERT`, `UPDATE`, `DELETE`.
    RowExclusive,
    /// Taken by `ANALYZE` and by index maintenance.
    ShareUpdateExclusive,
    /// Taken by `CREATE INDEX`.
    Share,
    /// Taken by nothing implicitly; asked for by `LOCK`.
    ShareRowExclusive,
    /// Taken by nothing implicitly; asked for by `LOCK`.
    Exclusive,
    /// Taken by `DROP`, `TRUNCATE` and `ALTER TABLE`, and by `LOCK` when the
    /// statement names no mode; it conflicts with every mode, including itself.
    AccessExclusive,
}

/// Which modes each mode CONFLICTS with, one bit per mode in the order
/// [`LockMode::index`] numbers them.
///
/// Source: the conflicting-lock-modes table of PostgreSQL's *Explicit Locking*
/// documentation, which Amazon Redshift's `LOCK` is derived from — so the
/// matrix is that table and not a rule of this emulator's own. It is
/// symmetric, as that table is.
const CONFLICTS: [u8; 8] = [0x80, 0xC0, 0xF0, 0xF8, 0xEC, 0xFC, 0xFE, 0xFF];

impl LockMode {
    /// The mode as the `LOCK` grammar spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AccessShare => "ACCESS SHARE",
            Self::RowShare => "ROW SHARE",
            Self::RowExclusive => "ROW EXCLUSIVE",
            Self::ShareUpdateExclusive => "SHARE UPDATE EXCLUSIVE",
            Self::Share => "SHARE",
            Self::ShareRowExclusive => "SHARE ROW EXCLUSIVE",
            Self::Exclusive => "EXCLUSIVE",
            Self::AccessExclusive => "ACCESS EXCLUSIVE",
        }
    }

    /// This mode's position in the conflict matrix, weakest first.
    fn index(self) -> usize {
        match self {
            Self::AccessShare => 0,
            Self::RowShare => 1,
            Self::RowExclusive => 2,
            Self::ShareUpdateExclusive => 3,
            Self::Share => 4,
            Self::ShareRowExclusive => 5,
            Self::Exclusive => 6,
            Self::AccessExclusive => 7,
        }
    }

    /// Whether a statement needing THIS mode has to wait for a lock already
    /// held in `held`.
    pub fn conflicts_with(self, held: Self) -> bool {
        CONFLICTS[self.index()] & (1 << held.index()) != 0
    }
}

/// One lock a connection holds, until the transaction it was taken in ends.
#[derive(Debug, Clone)]
pub(crate) struct HeldLock {
    /// The database the locked table belongs to.
    database_key: String,
    /// The ENGINE name of the locked table.
    table: String,
    /// The mode it is held in.
    mode: LockMode,
    /// The connection holding it.
    owner: String,
}

/// What one statement did to the locks its connection holds, reported by the
/// execution and carried out where the lock registry lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockEffect {
    /// `LOCK`: hold these tables in this mode until the transaction ends.
    Acquire {
        /// The ENGINE names of the locked tables.
        tables: Vec<String>,
        /// The mode asked for.
        mode: LockMode,
    },
    /// The transaction ended (`COMMIT`, `ROLLBACK`, `ABORT`), so the connection
    /// holds none of its locks any more.
    ReleaseAll,
}

/// The lock ONE statement needs before it can run: the tables it touches and
/// the mode touching them takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockRequest {
    /// The ENGINE names of the tables the statement references.
    pub tables: Vec<String>,
    /// The mode the statement takes on them.
    pub mode: LockMode,
}

/// The temporary relations ONE session's connection holds.
#[derive(Debug, Default)]
pub(crate) struct SessionTemp {
    /// The database they live in, so only an execution on that database drops
    /// them.
    pub(crate) database_key: String,
    /// Their names, lower-cased, as [`TempRelations`] matches them.
    pub(crate) relations: BTreeSet<String>,
}

/// Everything about the connections of this service's statements: the temporary
/// relations each live session holds, and the locks they hold.
#[derive(Debug, Default)]
pub(crate) struct Connections {
    /// The temporary relations of each live session's connection, by
    /// `SessionId`. Bounded by the sessions this service holds, each bounded by
    /// [`MAX_TEMP_RELATIONS`].
    temp: HashMap<String, SessionTemp>,
    /// The locks the live sessions hold, bounded by [`MAX_HELD_LOCKS`].
    locks: Vec<HeldLock>,
}

impl ComAmazonawsRedshiftdataRedshiftdataState {
    /// The connection the submission recorded under `id` runs on.
    ///
    /// It also RETIRES the temporary relations of the connections this service
    /// no longer holds a live session for and that live in the same database:
    /// the execution this scope is built for is what can drop them, because it
    /// is what holds that database.
    pub fn connection_scope(&mut self, id: &str) -> ConnectionScope {
        let Some(statement) = self.statements.get(id) else {
            return ConnectionScope::detached();
        };
        let session = statement.session_id.clone();
        let database_key = statement
            .target
            .database_key(statement.database.as_deref().unwrap_or_default());
        let key = session.clone().unwrap_or_else(|| statement.id.clone());
        let temp = session
            .as_deref()
            .and_then(|session| self.connections.temp.get(session))
            .map(|held| held.relations.clone())
            .unwrap_or_default();
        let retired = self.retire_closed_connections(&database_key);
        let locks_held = self
            .connections
            .locks
            .iter()
            .filter(|held| held.owner == key)
            .count();
        ConnectionScope::new(key, session.is_some(), temp, retired, locks_held)
    }

    /// The ENGINE names of the temporary relations held in `database_key` by
    /// connections this service no longer holds, dropped from the registry here
    /// and from the engine by the execution that is given them.
    fn retire_closed_connections(&mut self, database_key: &str) -> Vec<String> {
        let live = self.live_sessions();
        let mut retired: Vec<String> = Vec::new();
        self.connections.temp.retain(|session, held| {
            if live.contains(session) || held.database_key != database_key {
                return true;
            }
            for relation in &held.relations {
                retired.push(format!("{TEMP_PREFIX}{session}.{relation}"));
            }
            false
        });
        retired
    }

    /// The sessions a statement may still run on: the ones this service holds
    /// and the clock has not closed.
    fn live_sessions(&self) -> BTreeSet<String> {
        let now = SystemTime::now();
        self.sessions
            .values()
            .filter(|session| !session.is_closed(now))
            .map(|session| session.id.clone())
            .collect()
    }

    /// Release the locks of every connection this service no longer holds a
    /// live session for: a closed session "can no longer run SQL statements",
    /// so its connection is closed and holds nothing — a release the clock owes
    /// whether or not anybody asked for it.
    pub(crate) fn release_closed_connections(&mut self) {
        if self.connections.locks.is_empty() {
            return;
        }
        let live = self.live_sessions();
        self.connections
            .locks
            .retain(|lock| live.contains(&lock.owner));
    }

    /// The table a statement of the submission recorded under `id` has to wait
    /// for: one its `requests` need a lock on that another connection already
    /// holds in a conflicting mode. `None` when nothing blocks it.
    pub fn conflicting_lock(&self, id: &str, requests: &[LockRequest]) -> Option<String> {
        let statement = self.statements.get(id)?;
        let owner = statement
            .session_id
            .clone()
            .unwrap_or_else(|| statement.id.clone());
        let database_key = statement
            .target
            .database_key(statement.database.as_deref().unwrap_or_default());
        self.connections
            .locks
            .iter()
            .filter(|held| held.database_key == database_key && held.owner != owner)
            .find(|held| {
                requests.iter().any(|request| {
                    request.mode.conflicts_with(held.mode)
                        && request
                            .tables
                            .iter()
                            .any(|table| table.eq_ignore_ascii_case(&held.table))
                })
            })
            .map(|held| held.table.clone())
    }

    /// Carry out what the statements of the submission recorded under `id` did
    /// to the locks their connection holds.
    ///
    /// Only a SESSION's connection outlives its submission, so only its locks
    /// are recorded: a statement submitted without a session runs on a
    /// connection that closes when it ends, and a connection that is gone holds
    /// nothing.
    pub(crate) fn apply_lock_effects(&mut self, id: &str, effects: &[LockEffect]) {
        if effects.is_empty() {
            return;
        }
        let Some(statement) = self.statements.get(id) else {
            return;
        };
        let Some(owner) = statement.session_id.clone() else {
            return;
        };
        let database_key = statement
            .target
            .database_key(statement.database.as_deref().unwrap_or_default());
        for effect in effects {
            match effect {
                LockEffect::Acquire { tables, mode } => {
                    for table in tables {
                        // The registry's last word. A `LOCK` that would take
                        // its connection past [`MAX_LOCKS_PER_CONNECTION`] is
                        // refused while it RUNS, so a statement that reported
                        // FINISHED never reaches this — it is here so the
                        // registry cannot grow past its bound whatever reaches
                        // it.
                        if self.connections.locks.len() >= MAX_HELD_LOCKS {
                            return;
                        }
                        self.connections.locks.push(HeldLock {
                            database_key: database_key.clone(),
                            table: table.clone(),
                            mode: *mode,
                            owner: owner.clone(),
                        });
                    }
                }
                LockEffect::ReleaseAll => {
                    self.connections.locks.retain(|held| held.owner != owner);
                }
            }
        }
    }

    /// Record the temporary relations the statements of the submission recorded
    /// under `id` created, when they ran on a connection that outlives them.
    pub(crate) fn hold_temp_relations(&mut self, id: &str, created: &[String]) {
        if created.is_empty() {
            return;
        }
        let Some(statement) = self.statements.get(id) else {
            return;
        };
        let Some(session) = statement.session_id.clone() else {
            return;
        };
        let database_key = statement
            .target
            .database_key(statement.database.as_deref().unwrap_or_default());
        let held = self.connections.temp.entry(session).or_default();
        held.database_key = database_key;
        for relation in created {
            if held.relations.len() >= MAX_TEMP_RELATIONS {
                return;
            }
            held.relations.insert(relation.to_ascii_lowercase());
        }
    }

    /// Whether this service holds NO lock at all, so nothing can be waiting for
    /// one: the common case, answered before a submission's statements are read
    /// for what they would need locked.
    pub(crate) fn no_locks_held(&self) -> bool {
        self.connections.locks.is_empty()
    }

    /// A new place in the queue of the database the submission recorded under
    /// `id` runs on, for a submission that handed its turn on while it waited
    /// for a lock.
    ///
    /// `None` when that database is not open any more, in which case nothing is
    /// queued on it and the submission waits for no turn.
    pub(crate) fn requeue(&mut self, id: &str) -> Option<QueueSlot> {
        let statement = self.statements.get(id)?;
        let key = statement
            .target
            .database_key(statement.database.as_deref().unwrap_or_default());
        let open = self.databases.get_mut(&key)?;
        Some(super::queue_slot(&mut open.tail))
    }
}

/// What a statement waiting for another connection's lock ended up doing.
#[derive(Debug)]
pub enum LockWait {
    /// Nothing holds a conflicting lock any more: the statement may run.
    Free,
    /// The statement was cancelled while it waited, so it never ran.
    Abandoned,
    /// The wait reached [`MAX_LOCK_WAIT`], carrying the message the statement
    /// reports as its `Error`.
    GaveUp(String),
}

/// Hold the submission recorded under `id` until no OTHER connection holds a
/// lock that conflicts with what its statements need.
///
/// A submission that has to wait HANDS ON its turn on the database first
/// (`slot` is released) and takes a new place in that queue once the lock is
/// free: the connection holding the lock releases it by running a statement of
/// its own, which needs a turn on the same database, so a waiting submission
/// that kept its turn would be waiting for something that could never happen.
/// A waiting statement is reported as PICKED — it has been chosen to be run —
/// and it is cancellable, because the wait watches its execution's own bound.
pub async fn await_lock_release<Eng: Send + Sync + 'static>(
    engine: &std::sync::Arc<Eng>,
    id: &str,
    statements: Vec<String>,
    scope: &ConnectionScope,
    bound: &ExecutionBound,
    slot: &mut Option<QueueSlot>,
) -> LockWait {
    let Some(state) = service_state(&**engine) else {
        return LockWait::Free;
    };
    // Nothing is held, so nothing can be waited for: the statements are not read
    // at all in the common case, and no submission pays for a lock registry
    // that is empty.
    if state.lock().no_locks_held() {
        return LockWait::Free;
    }
    // Reading what the statements need is bounded but BLOCKING work on the
    // caller's own text, so it runs on the blocking pool rather than on an
    // async worker.
    let requests = {
        let scope = scope.clone();
        match tokio::task::spawn_blocking(move || lock_requests(&statements, &scope)).await {
            Ok(requests) => requests,
            // The read itself did not run, which says nothing about any lock:
            // the statement goes on to its turn rather than waiting.
            Err(_joined) => return LockWait::Free,
        }
    };
    if requests.is_empty() {
        return LockWait::Free;
    }

    let deadline = Instant::now() + MAX_LOCK_WAIT;
    let mut blocked: Option<String> = None;
    loop {
        let conflict = {
            let mut store = state.lock();
            // A session the clock has closed holds nothing, so the locks that
            // release owes are released before this asks.
            store.materialise_due_transitions();
            store.conflicting_lock(id, &requests)
        };
        let Some(table) = conflict else {
            // Free: a submission that had handed its turn on takes a new place
            // in the queue, and one that never waited keeps the turn it holds.
            if blocked.is_some() {
                *slot = state.lock().requeue(id);
            }
            return LockWait::Free;
        };
        if blocked.is_none() {
            // The query has been chosen to be run and is waiting for a lock, so
            // a caller polling it sees it running rather than queued — and the
            // database it is waiting on is handed to the submissions behind it.
            state.lock().pick(id);
            *slot = None;
        }
        blocked = Some(table);
        if bound.abandoned() {
            return LockWait::Abandoned;
        }
        if Instant::now() >= deadline {
            return LockWait::GaveUp(lock_wait_message(blocked.as_deref().unwrap_or_default()));
        }
        tokio::time::sleep(LOCK_POLL).await;
    }
}

/// The `Error` a statement that waited [`MAX_LOCK_WAIT`] for a lock reports.
fn lock_wait_message(table: &str) -> String {
    format!(
        "the statement waited for a lock on \"{table}\" held by another connection and reached \
         this emulator's bound of {} seconds",
        MAX_LOCK_WAIT.as_secs()
    )
}

/// What the statements of one submission need locked, read as Redshift SQL.
fn lock_requests(statements: &[String], scope: &ConnectionScope) -> Vec<LockRequest> {
    let relations = scope.relations();
    statements
        .iter()
        .flat_map(|text| super::redshift_sql::split_statements(text))
        .filter_map(|statement| super::redshift_sql::lock_request(statement, &relations))
        .filter(|request| !request.tables.is_empty())
        .collect()
}
