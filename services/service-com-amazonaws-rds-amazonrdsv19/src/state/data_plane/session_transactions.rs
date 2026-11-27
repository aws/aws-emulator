// SPDX-License-Identifier: CC0-1.0

//! The transaction ONE SESSION has open, and what its snapshot and its commit mean.
//!
//! A transaction belongs to the session that opened it: `BEGIN` … `COMMIT` brackets the
//! statements of THAT connection, two sessions hold two independent transactions at once,
//! and one session's `ROLLBACK` undoes its own work and nobody else's. The embedded engine
//! (SQLite) runs one transaction per database at a time, which is why the transaction a
//! session opens cannot be the engine's. It is kept here instead, as a SNAPSHOT and a set of
//! changed rows:
//!
//! * the database as it stood at the transaction's first statement in it — a page-for-page
//!   copy made by the engine's own online backup, which is the database the transaction's
//!   own statements then run against (`super::sql_engine`), so what it writes is in its copy
//!   and nowhere else until it commits, and what another session commits meanwhile does not
//!   move under it;
//! * WHICH rows it has changed since, from the engine's update hook, which reports every row
//!   an `INSERT`, `UPDATE` or `DELETE` touched, including the rows a cascading foreign-key
//!   action or a trigger touched with it.
//!
//! A `COMMIT` then makes exactly those rows of the shared database what the transaction's own
//! copy holds for them ([`apply_rows`]), so a session opened after it reads them, and a
//! `ROLLBACK` drops the copy, which leaves the shared database exactly as it was. Nothing
//! here reads a statement's text to work out what it did.
//!
//! WHEN that snapshot is taken is the ISOLATION LEVEL the transaction runs at, which is what
//! each level means for what its statements read:
//!
//! * at REPEATABLE READ and SERIALIZABLE it is taken ONCE per database per transaction, so
//!   the transaction reads the same rows throughout however often anybody else commits;
//! * at READ COMMITTED and READ UNCOMMITTED each STATEMENT reads what is committed when IT
//!   runs, so the snapshot is taken again — as a fresh copy of the shared database with this
//!   transaction's own uncommitted work put back onto it ([`apply_schema`] and
//!   [`apply_rows`], the same carry a commit makes, in the other direction) — whenever the
//!   shared database has changed since the copy was taken
//!   ([`OpenTransactions::copy_is_stale`]).
//!
//! The limits of keeping a transaction this way, stated rather than hidden:
//!
//! * what a commit carries over is the ROWS it changed and the SCHEMA CHANGES it made
//!   ([`apply_schema`]) — its own delta against the schema its copy declared when the copy
//!   was taken, so an object the transaction never touched is left exactly as the shared
//!   database holds it.
//!
//! A row the transaction CREATED is ADDED to the shared database rather than put in the place
//! of whatever that database now holds at the same engine row id: the row id of a new row is
//! the engine's to assign, so another writer may well have given it to a row of its own since
//! the snapshot, and a commit that replaced that row would drop a write nobody rolled back.
//! Only a row the transaction found at the snapshot is replaced in place, which is what
//! changing it means ([`apply_rows`]).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use rusqlite::hooks::Action;
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension};

use super::sql_engine::{EngineError, EngineErrorKind};

/// How many distinct rows ONE transaction may hold uncommitted, across every database it
/// touched.
///
/// Each one is a row a `ROLLBACK` has to put back, so this is what bounds the bookkeeping
/// one session can make the engine hold by never committing. Past it the recording stops and
/// the transaction reports that it can no longer be rolled back, rather than quietly putting
/// back only some of its rows.
pub const MAX_UNCOMMITTED_ROWS: usize = 200_000;

/// How many uncommitted rows the transactions of ONE ENDPOINT may hold between them.
///
/// The bound above is one transaction's; this one is what keeps the bookkeeping of many
/// sessions from being that bound times the number of sessions. Past it a transaction stops
/// recording and reports that it can no longer be rolled back, the same way it does at its
/// own ceiling.
pub const MAX_UNCOMMITTED_ROWS_PER_ENDPOINT: usize = 500_000;

/// One row the engine changed: the table it is in and the engine's own row id for it.
pub type ChangedRow = (String, i64);

/// One row of a transaction's own work that came back at ANOTHER row id than the one it had:
/// the table, the row id it was recorded under, and the row id the engine gave it when the
/// row was put back ([`apply_rows`]).
///
/// A row the transaction CREATED is the only kind this happens to, and only where the
/// database it is put back into already holds a row at that row id — another writer's, which
/// is not this transaction's to take. What it is FOR: the transaction's record of the rows it
/// has changed names them by row id, so the record follows the row
/// ([`OpenTransactions::remap_rows`]).
pub type RemappedRow = (String, i64, i64);

/// One RELATION a transaction read or wrote: the database it is in and its name, lower-cased
/// (both engines resolve a table name case-insensitively on the platforms RDS runs them on).
pub type Relation = (String, String);

/// The SCHEMA one database declares: each object by name and kind, with the declaration the
/// engine itself holds for it. What a transaction's copy held when the copy was taken, so a
/// commit can carry over the transaction's OWN schema changes and nothing else
/// ([`apply_schema`]).
pub type SchemaObjects = BTreeMap<(String, String), String>;

/// How many COMMITS one endpoint keeps the write set of, which is what a SERIALIZABLE
/// transaction's reads are checked against ([`OpenTransactions::conflicting_commit`]).
///
/// A transaction whose snapshot predates every commit still kept cannot be shown to be
/// serializable, so it is refused rather than committed on an unknown: the bound is what
/// keeps the log from growing with the endpoint's lifetime.
const MAX_COMMITS_REMEMBERED: usize = 1_024;

/// How many RELATIONS one transaction's read set holds, which is what bounds the
/// bookkeeping one session can make this endpoint keep by naming new relations.
const MAX_RELATIONS_READ: usize = 1_024;

/// How many SCHEMA OBJECTS one transaction may carry over when it commits
/// ([`apply_schema`]).
///
/// A client chooses this number — a transaction can declare as many tables, views, indexes
/// and triggers as it likes before it commits — and the declarations are RETRIED while a
/// round of them resolves something, so the work of carrying them over grows with the square
/// of that number. The commit runs while the endpoint's engine is held, so the count is
/// bounded here as well as by the deadline every round is checked against: past it the commit
/// is refused rather than holding the engine for work one session asked for. Far more objects
/// than a schema a client declares in one transaction, and far fewer than a number that keeps
/// the engine busy.
const MAX_SCHEMA_OBJECTS_CARRIED: usize = 1_024;

/// The open transactions of one engine endpoint's sessions, shared with the update hook
/// installed on every one of its databases' connections.
#[derive(Debug, Default)]
pub struct OpenTransactions {
    state: Mutex<Transactions>,
}

#[derive(Debug, Default)]
struct Transactions {
    /// The session whose statement is running right now, where it has a transaction open:
    /// the one the engine's row changes are recorded against. Statements run one at a time
    /// under the engine's own lock, so one value is enough.
    recording: Option<u32>,
    open: BTreeMap<u32, Transaction>,
    /// How many rows the open transactions hold between them, which is what
    /// [`MAX_UNCOMMITTED_ROWS_PER_ENDPOINT`] bounds.
    rows: usize,
    /// How many transactions have committed on this endpoint, which ORDERS the commits a
    /// transaction's snapshot predates.
    commits: u64,
    /// What the last [`MAX_COMMITS_REMEMBERED`] commits WROTE, oldest first: each one's
    /// place in that order and the relations it changed.
    committed: Vec<(u64, BTreeSet<Relation>)>,
}

/// One session's transaction.
#[derive(Debug, Default)]
struct Transaction {
    /// The rows it has changed, by database.
    changed: BTreeMap<String, BTreeSet<ChangedRow>>,
    /// Which of those rows it CREATED: the ones the engine first reported as an INSERT, so
    /// its copy of the database held no row at that row id when the transaction first
    /// touched it — and neither did the snapshot that copy is. Its commit ADDS them
    /// ([`apply_rows`]) instead of taking the place of whatever the shared database holds
    /// at the same row id now.
    created: BTreeMap<String, BTreeSet<ChangedRow>>,
    /// The databases it holds its own copy of: the names the engine keeps those copies
    /// under ([`super::sql_engine`]), which is where its statements run, each with the
    /// SCHEMA that copy declared when it was taken.
    copies: BTreeMap<String, SchemaObjects>,
    /// How many engine pages each of those copies holds, by the same name: what a copy
    /// TAKEN AGAIN ([`OpenTransactions::note_recopy`]) replaces in the total below.
    copy_pages: BTreeMap<String, i64>,
    /// What the SHARED database of each of those names had changed when the copy was taken
    /// ([`OpenTransactions::copy_is_stale`]): the mark the caller reads off the engine, which
    /// says whether anything has been committed into it since.
    read_at: BTreeMap<String, i64>,
    /// The relations its statements have READ, which is what decides whether it can be
    /// ordered after the transactions that committed while it was open.
    read: BTreeSet<Relation>,
    /// How many engine PAGES those copies hold, which is what the endpoint's own budget for
    /// uncommitted state is counted in.
    pages: i64,
    /// How many rows `changed` holds in all, which is what [`MAX_UNCOMMITTED_ROWS`] bounds:
    /// the count is the transaction's, not one database's.
    rows: usize,
    /// Whether the recording hit [`MAX_UNCOMMITTED_ROWS`] and stopped.
    overflowed: bool,
    /// Which commit this transaction's snapshot was taken after: the commits it cannot see.
    began_after: u64,
}

/// What a transaction that has ended held: the rows it changed, the databases it has its own
/// copy of, the relations it read, and whether the recording of them is complete.
#[derive(Debug, Default)]
pub struct ClosedTransaction {
    pub changed: BTreeMap<String, BTreeSet<ChangedRow>>,
    pub created: BTreeMap<String, BTreeSet<ChangedRow>>,
    pub copies: BTreeMap<String, SchemaObjects>,
    pub read: BTreeSet<Relation>,
    pub overflowed: bool,
    pub began_after: u64,
}

impl ClosedTransaction {
    /// The relations this transaction WROTE, which is what the transactions still open are
    /// checked against once it has committed.
    #[must_use]
    pub fn wrote(&self) -> BTreeSet<Relation> {
        self.changed
            .iter()
            .flat_map(|(database, rows)| {
                rows.iter()
                    .map(move |(table, _)| (database.clone(), table.to_ascii_lowercase()))
            })
            .collect()
    }
}

/// What ONE open transaction has done to ONE of its databases itself: the rows it changed,
/// which of those it created, and the schema its copy of that database declared when the copy
/// was taken.
///
/// What a copy taken again for a statement's own snapshot has to be given back, and the
/// same three things a COMMIT carries over
/// ([`ClosedTransaction`]) for the one database — read out of the transaction while it is
/// still open.
#[derive(Debug, Default)]
pub struct OwnWork {
    pub changed: BTreeSet<ChangedRow>,
    pub created: BTreeSet<ChangedRow>,
    pub schema: SchemaObjects,
}

impl OpenTransactions {
    /// The transactions, recovering a lock a panicking session poisoned: a long-lived
    /// emulator has to keep serving every later session.
    fn state(&self) -> MutexGuard<'_, Transactions> {        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Open a transaction for `session`, answering whether it had none open already.
    pub fn open(&self, session: u32) -> bool {
        let mut state = self.state();
        if state.open.contains_key(&session) {
            return false;
        }
        let began_after = state.commits;
        state.open.insert(
            session,
            Transaction {
                began_after,
                ..Transaction::default()
            },
        );
        true
    }

    /// Whether `session` has a transaction open.
    #[must_use]
    pub fn is_open(&self, session: u32) -> bool {
        self.state().open.contains_key(&session)
    }

    /// Whether `session`'s transaction already has its own copy of the database `database`.
    #[must_use]
    pub fn holds_a_copy_of(&self, session: u32, database: &str) -> bool {
        self.state()
            .open
            .get(&session)
            .is_some_and(|open| open.copies.contains_key(database))
    }

    /// Record that `session`'s transaction now has its own copy of the database `database`,
    /// declaring the schema `schema` and taken while the shared database stood at `read_at`
    /// ([`Self::copy_is_stale`]), and charge the `pages` it holds to that transaction.
    pub fn note_copy(
        &self,
        session: u32,
        database: &str,
        pages: i64,
        schema: SchemaObjects,
        read_at: i64,
    ) {
        if let Some(open) = self.state().open.get_mut(&session) {
            open.copies.insert(database.to_string(), schema);
            open.copy_pages.insert(database.to_string(), pages);
            open.read_at.insert(database.to_string(), read_at);
            open.pages = open.pages.saturating_add(pages);
        }
    }

    /// Record that the copy `session`'s transaction holds of `database` has been TAKEN AGAIN:
    /// it declares the schema `schema`, holds `pages` pages in the place of the pages the copy
    /// before it held, and was taken while the shared database stood at `read_at`.
    pub fn note_recopy(
        &self,
        session: u32,
        database: &str,
        pages: i64,
        schema: SchemaObjects,
        read_at: i64,
    ) {
        if let Some(open) = self.state().open.get_mut(&session) {
            let before = open
                .copy_pages
                .insert(database.to_string(), pages)
                .unwrap_or_default();
            open.copies.insert(database.to_string(), schema);
            open.read_at.insert(database.to_string(), read_at);
            open.pages = open.pages.saturating_sub(before).saturating_add(pages);
        }
    }

    /// Whether the copy `session`'s transaction holds of `database` was taken BEFORE the
    /// shared database reached `read_at` — so a statement that must read what is committed
    /// now has to be given a copy taken again.
    ///
    /// `read_at` is a mark the caller reads off the engine itself and never a time: what it
    /// says is that the shared database has changed since, not how long ago. A transaction
    /// holding no copy of that database has nothing stale.
    #[must_use]
    pub fn copy_is_stale(&self, session: u32, database: &str, read_at: i64) -> bool {
        self.state().open.get(&session).is_some_and(|open| {
            open.copies.contains_key(database) && open.read_at.get(database) != Some(&read_at)
        })
    }

    /// What `session`'s transaction has done to `database` ITSELF: the rows it changed, which
    /// of those it created, and the schema its copy declared when the copy was taken.
    ///
    /// What a copy taken again has to be given back, and `None` for a session with no
    /// transaction open.
    #[must_use]
    pub fn own_work_in(&self, session: u32, database: &str) -> Option<OwnWork> {
        self.state().open.get(&session).map(|open| OwnWork {
            changed: open.changed.get(database).cloned().unwrap_or_default(),
            created: open.created.get(database).cloned().unwrap_or_default(),
            schema: open.copies.get(database).cloned().unwrap_or_default(),
        })
    }

    /// Record that rows of `session`'s own work in `database` came back at other row ids than
    /// the ones they were recorded under ([`RemappedRow`]), so the record of what this
    /// transaction has changed names the rows the copy now holds.
    pub fn remap_rows(&self, session: u32, database: &str, remapped: &[RemappedRow]) {
        let mut state = self.state();
        let Some(open) = state.open.get_mut(&session) else {
            return;
        };
        for rows in [
            open.changed.get_mut(database),
            open.created.get_mut(database),
        ]
        .into_iter()
        .flatten()
        {
            for (table, was, now) in remapped {
                if rows.remove(&(table.clone(), *was)) {
                    rows.insert((table.clone(), *now));
                }
            }
        }
    }

    /// How many engine pages the states the OPEN transactions hold add up to right now,
    /// which is what the endpoint's budget for uncommitted state is checked against.
    #[must_use]
    pub fn charged_pages(&self) -> i64 {
        self.state()
            .open
            .values()
            .map(|open| open.pages)
            .fold(0i64, i64::saturating_add)
    }

    /// End `session`'s transaction and answer what it held, or `None` where it had none
    /// open.
    pub fn close(&self, session: u32) -> Option<ClosedTransaction> {
        let mut state = self.state();
        if state.recording == Some(session) {
            state.recording = None;
        }
        state.open.remove(&session).map(|open| {
            state.rows = state.rows.saturating_sub(open.rows);
            ClosedTransaction {
                changed: open.changed,
                created: open.created,
                copies: open.copies,
                read: open.read,
                overflowed: open.overflowed,
                began_after: open.began_after,
            }
        })
    }

    /// Record that `session`'s transaction has READ the relations `relations` of the
    /// database `database`.
    pub fn note_read(
        &self,
        session: u32,
        database: &str,
        relations: impl IntoIterator<Item = String>,
    ) {
        let mut state = self.state();
        let Some(open) = state.open.get_mut(&session) else {
            return;
        };
        for relation in relations {
            if open.read.len() >= MAX_RELATIONS_READ {
                return;
            }
            open.read
                .insert((database.to_string(), relation.to_ascii_lowercase()));
        }
    }

    /// WHICH relation a transaction that began after `began_after` read and a LATER commit
    /// wrote, where there is one: the read/write dependency that cannot be ordered, which is
    /// what a SERIALIZABLE transaction is refused for.
    ///
    /// A transaction whose snapshot predates every commit this endpoint still remembers
    /// ([`MAX_COMMITS_REMEMBERED`]) cannot be shown to have no such dependency, so the oldest
    /// relation it read is answered rather than nothing.
    #[must_use]
    pub fn conflicting_commit(
        &self,
        began_after: u64,
        read: &BTreeSet<Relation>,
    ) -> Option<Relation> {
        if read.is_empty() {
            return None;
        }
        let state = self.state();
        let oldest = state.committed.first().map(|(at, _)| *at);
        if oldest.is_some_and(|oldest| began_after + 1 < oldest) {
            return read.iter().next().cloned();
        }
        state
            .committed
            .iter()
            .filter(|(at, _)| *at > began_after)
            .flat_map(|(_, wrote)| wrote.iter())
            .find(|relation| read.contains(*relation))
            .cloned()
    }

    /// Record that a transaction has COMMITTED, writing the relations `wrote`: its place in
    /// the endpoint's commit order, which the transactions still open are checked against.
    pub fn note_commit(&self, wrote: BTreeSet<Relation>) {
        let mut state = self.state();
        state.commits += 1;
        let at = state.commits;
        state.committed.push((at, wrote));
        // Only the commits an OPEN transaction could still conflict with have to be kept,
        // and never more than the endpoint's own bound.
        let earliest_open = state
            .open
            .values()
            .map(|open| open.began_after)
            .min()
            .unwrap_or(at);
        state.committed.retain(|(at, _)| *at > earliest_open);
        let over = state.committed.len().saturating_sub(MAX_COMMITS_REMEMBERED);
        state.committed.drain(..over);
    }

    /// The sessions holding a transaction open right now.
    #[must_use]
    pub fn open_sessions(&self) -> Vec<u32> {
        self.state().open.keys().copied().collect()
    }

    /// Record the rows the statement about to run changes against `session`'s transaction,
    /// where it has one; `None` for a statement whose changes belong to no transaction — a
    /// statement outside one, the server's own statements, and the rollback itself.
    pub fn recording_for(&self, session: Option<u32>) {
        let mut state = self.state();
        state.recording = session.filter(|session| state.open.contains_key(session));
    }

    /// Record the rows `rows` of `database` as changed by `session`'s transaction, whatever
    /// is recording right now.
    ///
    /// What the engine's own hook does not report: a `DELETE` of every row of a table is
    /// served by the engine's truncate path, which changes the rows without reporting them
    /// one by one, so the statement's OWN rows are recorded here as well — they are the rows
    /// it wrote.
    pub fn note_rows_for(
        &self,
        session: u32,
        database: &str,
        rows: impl IntoIterator<Item = ChangedRow>,
    ) {
        self.note(session, database, rows, false);
    }

    /// Record the rows `rows` of `database` as changed by whichever transaction is
    /// recording, `inserted` saying whether the engine reported them as rows an `INSERT`
    /// put in — the rows the transaction CREATED, which its commit adds rather than
    /// installs over ([`Transaction::created`]).
    pub fn note_rows(
        &self,
        database: &str,
        rows: impl IntoIterator<Item = ChangedRow>,
        inserted: bool,
    ) {
        let session = self.state().recording;
        if let Some(session) = session {
            self.note(session, database, rows, inserted);
        }
    }

    /// The one place a changed row is recorded, whichever of the two above asked for it.
    ///
    /// A row is CREATED where the engine reported an `INSERT` the FIRST time this
    /// transaction touched it: at that moment its copy held no row at that row id, so the
    /// snapshot the copy is held none either. A row it deleted or updated first is a row of
    /// the snapshot's however the transaction left it afterwards.
    fn note(
        &self,
        session: u32,
        database: &str,
        rows: impl IntoIterator<Item = ChangedRow>,
        inserted: bool,
    ) {
        let mut state = self.state();
        let endpoint_rows = state.rows;
        let Some(open) = state.open.get_mut(&session) else {
            return;
        };
        let own_rows = open.rows;
        let changed = open.changed.entry(database.to_string()).or_default();
        let mut added = 0usize;
        let mut overflowed = false;
        let mut created = BTreeSet::new();
        for row in rows {
            let at_its_own_ceiling = own_rows + added >= MAX_UNCOMMITTED_ROWS;
            let at_the_endpoints = endpoint_rows + added >= MAX_UNCOMMITTED_ROWS_PER_ENDPOINT;
            if (at_its_own_ceiling || at_the_endpoints) && !changed.contains(&row) {
                overflowed = true;
                break;
            }
            if !changed.contains(&row) {
                if inserted {
                    created.insert(row.clone());
                }
                changed.insert(row);
                added += 1;
            }
        }
        if !created.is_empty() {
            open.created
                .entry(database.to_string())
                .or_default()
                .extend(created);
        }
        open.rows += added;
        open.overflowed = open.overflowed || overflowed;
        state.rows = endpoint_rows.saturating_add(added);
    }
}

/// Install the recording hook on `conn`, the connection the database `database` runs on.
///
/// Every row the engine changes on this connection is reported to `transactions`, which
/// keeps it only while a transaction is recording ([`OpenTransactions::recording_for`]).
///
/// The engine's own error when it will not install the hook: a connection whose changes are
/// not reported cannot have a session's rows put back, so it is never handed out as one
/// that can.
pub fn record_changes_of(
    conn: &mut Connection,
    database: &str,
    transactions: &Arc<OpenTransactions>,
) -> rusqlite::Result<()> {
    let transactions = Arc::clone(transactions);
    let database = database.to_string();
    conn.update_hook(Some(
        move |action, _schema: &str, table: &str, rowid: i64| {
            transactions.note_rows(
                &database,
                [(table.to_string(), rowid)],
                matches!(action, Action::SQLITE_INSERT),
            );
        },
    ))
}

/// Carry the rows `rows` of the committing transaction over into the shared database `live`.
///
/// `source` is the transaction's own copy of the database, so for every row it CHANGED — one
/// its copy, and therefore the snapshot, already held — the shared database ends up holding
/// the value that copy holds: the row as the transaction left it, or no row at all where it
/// deleted one.
///
/// A row in `created` is one the transaction itself made, at a row id the engine assigned in
/// its own copy. It is ADDED here: the shared database may have given that row id to a row of
/// another writer's since the snapshot, and such a row is not this transaction's to remove, so
/// nothing is deleted for a created row and one whose row id is taken is inserted wherever the
/// engine puts it — exactly as it would have been had it been inserted here in the first
/// place. Where the row id is the client's own value (a declared `INTEGER PRIMARY KEY`), the
/// insert carries it, so two writers' rows of the same key meet the engine's own unique
/// violation instead of one of them being dropped.
///
/// Two passes over the CHANGED rows, because a row's old value can be the value another of
/// them holds now: every one of them is removed first, then the ones `source` holds are put
/// in. It runs as ONE engine transaction with the foreign-key checks deferred to its end, so a
/// row that is the parent of another need not be the first one in.
///
/// Bounded by `deadline`, which the caller also installed on the engine itself: it gives up —
/// leaving the engine transaction rolled back — rather than running past the budget a commit
/// gets, because this runs while the endpoint's engine is held.
///
/// A table that is not in both ends is left alone, which is why [`apply_schema`] runs first:
/// a table the transaction created is in `source` only until the schema it declares is
/// carried over too.
///
/// Answers the rows that came back at ANOTHER row id than the one they were asked for
/// ([`RemappedRow`]) — a created row whose row id the destination had already given away —
/// so a caller whose transaction goes on afterwards can keep naming them.
pub fn apply_rows(
    live: &Connection,
    source: &Connection,
    rows: &BTreeSet<ChangedRow>,
    created: &BTreeSet<ChangedRow>,
    deadline: Instant,
) -> Result<Vec<RemappedRow>, EngineError> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let mut by_table: BTreeMap<&str, Vec<i64>> = BTreeMap::new();
    for (table, rowid) in rows {
        by_table.entry(table.as_str()).or_default().push(*rowid);
    }
    live.execute_batch("BEGIN; PRAGMA defer_foreign_keys = 1")
        .map_err(internal)?;
    match restore_each(live, source, &by_table, created, deadline) {
        Ok(remapped) => live
            .execute_batch("COMMIT")
            .map_err(internal)
            .map(|()| remapped),
        Err(error) => {
            let _ = live.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

/// Make the SCHEMA of the shared database `live` carry the schema changes the committing
/// transaction itself made: the tables, indexes, views and triggers it created are created,
/// the ones it dropped are dropped, and the ones whose DECLARATION it changed (an `ALTER
/// TABLE`, which both engines run inside a transaction) are declared as it left them.
///
/// `before` is the schema the transaction's own copy held when it was taken
/// ([`SchemaObjects`]), so what is carried over is the DELTA between that and the copy now —
/// the transaction's own DDL and nothing else. An object the transaction never touched is
/// left exactly as the shared database holds it, whatever another session committed on it
/// meanwhile, and an object this transaction dropped is dropped only while the shared
/// database still holds the declaration it was dropped from.
///
/// Read out of the engine's own `sqlite_master`, so what is carried over is the declaration
/// the engine itself holds for each object rather than anything reconstructed from the
/// statement that made it. An object whose declaration the engine does not hold (the engine's
/// own internal indexes, whose `sql` is NULL) belongs to a table that carries it, so it is
/// not created on its own.
///
/// The objects are declared in DEPENDENCY order ([`SCHEMA_ORDER`]), because an index, a view
/// and a trigger each name a table: the tables come first, and anything the engine still
/// refuses is retried while a round declares something, so a dependency between two objects
/// of the same kind is the engine's to resolve rather than this list's. An object the engine
/// refuses once nothing more can be declared is reported in the engine's own words rather
/// than skipped.
///
/// Bounded exactly as the row carry beside it is, because it runs in the same place — while
/// the endpoint's engine is held — and because the number of objects is the CLIENT's to
/// choose: `deadline` is the commit's own budget, checked before each declaration and each
/// retry round, and [`MAX_SCHEMA_OBJECTS_CARRIED`] bounds how many objects one commit carries
/// at all, so the retries cannot grow with the square of a count a session picked.
///
/// Answers the TABLES whose declaration the transaction changed, whose rows are then carried
/// over WHOLE ([`copy_table`]): a row of such a table no longer has the columns the shared
/// database held it under, so the table as the transaction left it is what the commit
/// carries, which is also what the lock PostgreSQL takes for the statement that changed it
/// leaves behind.
pub fn apply_schema(
    live: &Connection,
    source: &Connection,
    before: &SchemaObjects,
    deadline: Instant,
) -> Result<BTreeSet<String>, EngineError> {
    let declared = schema_objects(source)?;
    let held = schema_objects(live)?;
    let mut redeclared = BTreeSet::new();
    // What the TRANSACTION dropped: an object its copy held when the copy was taken and no
    // longer declares. It goes only while the shared database still holds the declaration it
    // was dropped from — another session's own DDL on it since is that session's.
    for (object, sql) in before {
        out_of_time(deadline)?;
        if !declared.contains_key(object) && held.get(object) == Some(sql) {
            let (name, kind) = object;
            let _ = live.execute_batch(&format!("DROP {kind} IF EXISTS {}", quoted(name)));
        }
    }
    // What it created or declared DIFFERENTLY: the shared database's own object of that name
    // goes, so the declaration this transaction left is the one that stands.
    let mut changed: Vec<(&(String, String), &String)> = Vec::new();
    for (object, sql) in &declared {
        if before.get(object) == Some(sql) {
            continue;
        }
        out_of_time(deadline)?;
        if changed.len() >= MAX_SCHEMA_OBJECTS_CARRIED {
            return Err(EngineError::new(
                EngineErrorKind::Internal,
                format!(
                    "this endpoint carries at most {MAX_SCHEMA_OBJECTS_CARRIED} schema objects \
                     of one transaction over"
                ),
            ));
        }
        let (name, kind) = object;
        if held.contains_key(object) {
            let _ = live.execute_batch(&format!("DROP {kind} IF EXISTS {}", quoted(name)));
            if kind == "table" {
                redeclared.insert(name.clone());
            }
        }
        changed.push((object, sql));
    }
    // Then the declarations themselves, in DEPENDENCY order: a table before the views,
    // indexes and triggers that name it. An object whose own dependency is another of these
    // (a view over a view) is retried while the round before it declared something, so the
    // order inside one kind is the engine's to decide rather than this list's.
    let mut pending: Vec<&String> = Vec::new();
    let present = schema_objects(live)?;
    for kind in SCHEMA_ORDER {
        for (object, sql) in &changed {
            if object.1 == *kind && !present.contains_key(*object) {
                pending.push(sql);
            }
        }
    }
    let mut rounds = 0usize;
    while !pending.is_empty() {
        out_of_time(deadline)?;
        let mut refused: Vec<&String> = Vec::new();
        let mut last: Option<EngineError> = None;
        for sql in pending.iter().copied() {
            out_of_time(deadline)?;
            if let Err(error) = live.execute_batch(sql).map_err(internal) {
                last = Some(error);
                refused.push(sql);
            }
        }
        if refused.is_empty() {
            break;
        }
        // Nothing this round declared succeeded, or there have been as many rounds as there
        // are objects: another round would refuse the same objects, so the engine's own
        // refusal is what the commit reports.
        if refused.len() == pending.len() || rounds >= pending.len() {
            return Err(last.unwrap_or_else(|| {
                EngineError::new(
                    EngineErrorKind::Internal,
                    "the schema a transaction declared could not be carried over",
                )
            }));
        }
        pending = refused;
        rounds += 1;
    }
    Ok(redeclared)
}

/// The order the KINDS of schema object are declared in: a table carries the columns a view,
/// an index and a trigger name, so it is declared before them.
///
/// `pub(super)` because it is the order of ANY carry of one engine's schema into another's,
/// not only a commit's: the carry of another endpoint's databases into a copy of a cluster's
/// data ([`super::sql_engine::InstanceDatabase::carry_tables_from`]) declares them in the same
/// order, for the same reason.
pub(super) const SCHEMA_ORDER: [&str; 4] = ["table", "view", "index", "trigger"];

/// Make the rows of `table` in the shared database `live` the rows `source` holds for it,
/// which is what carrying over a table whose DECLARATION the transaction changed means
/// ([`apply_schema`]).
///
/// Bounded by `deadline`, as every other part of a commit is, and by the number of rows one
/// transaction may carry ([`MAX_UNCOMMITTED_ROWS`]): a table larger than that is reported
/// rather than copied.
pub fn copy_table(
    live: &Connection,
    source: &Connection,
    table: &str,
    deadline: Instant,
) -> Result<(), EngineError> {
    let columns = column_names(source, table);
    if columns.is_empty() {
        return Ok(());
    }
    let quoted_table = quoted(table);
    live.execute(&format!("DELETE FROM {quoted_table}"), [])
        .map_err(internal)?;
    let mut names: Vec<String> = vec!["rowid".to_string()];
    names.extend(columns.iter().map(|name| quoted(name)));
    let placeholders: Vec<String> = (1..=names.len()).map(|at| format!("?{at}")).collect();
    let statement = format!(
        "INSERT INTO {quoted_table} ({}) VALUES ({})",
        names.join(", "),
        placeholders.join(", ")
    );
    let mut reading = source
        .prepare(&format!("SELECT rowid, * FROM {quoted_table}"))
        .map_err(internal)?;
    let mut rows = reading.query([]).map_err(internal)?;
    let mut carried = 0usize;
    while let Some(row) = rows.next().map_err(internal)? {
        out_of_time(deadline)?;
        if carried >= MAX_UNCOMMITTED_ROWS {
            return Err(EngineError::new(
                EngineErrorKind::Internal,
                format!(
                    "this endpoint carries at most {MAX_UNCOMMITTED_ROWS} rows of a table whose \
                     declaration a transaction changed"
                ),
            ));
        }
        let mut bound: Vec<Value> = Vec::with_capacity(names.len());
        for index in 0..names.len() {
            bound.push(row.get::<usize, Value>(index).map_err(internal)?);
        }
        live.execute(&statement, rusqlite::params_from_iter(bound))
            .map_err(internal)?;
        carried += 1;
    }
    Ok(())
}

/// The objects `conn`'s schema declares, by name and kind, each with the declaration the
/// engine holds for it.
///
/// The engine's OWN objects are left out, which is every object whose name carries the prefix
/// the engine reserves for itself ([`ENGINE_OBJECT_PREFIX`]) and every object with no
/// declaration of its own (the internal index of a constraint): a transaction neither declares
/// nor drops one, and the engine refuses a statement that names one — so an object of the
/// engine's that a transaction's own statement brought into being (the sequence table an
/// auto-incrementing key is kept in) is the engine's to keep on both sides rather than
/// anything a commit carries over.
pub fn schema_objects(conn: &Connection) -> Result<SchemaObjects, EngineError> {
    let mut statement = conn
        .prepare(
            "SELECT name, type, sql FROM sqlite_master \
             WHERE sql IS NOT NULL AND type IN ('table', 'index', 'view', 'trigger')",
        )
        .map_err(internal)?;
    let mut objects = BTreeMap::new();
    let mut rows = statement.query([]).map_err(internal)?;
    while let Some(row) = rows.next().map_err(internal)? {
        let name: String = row.get(0).map_err(internal)?;
        let kind: String = row.get(1).map_err(internal)?;
        let sql: String = row.get(2).map_err(internal)?;
        if name
            .to_ascii_lowercase()
            .starts_with(ENGINE_OBJECT_PREFIX)
        {
            continue;
        }
        objects.insert((name, kind), sql);
    }
    Ok(objects)
}

/// The prefix the embedded engine reserves for the objects IT defines: "any table (or index)
/// whose name begins with `sqlite_` is reserved for internal use", which is also why the
/// engine refuses a statement that declares one.
const ENGINE_OBJECT_PREFIX: &str = "sqlite_";

/// The two passes of [`apply_rows`], under the transaction it opened. Answers the rows that
/// came back at another row id ([`RemappedRow`]).
fn restore_each(
    live: &Connection,
    before: &Connection,
    by_table: &BTreeMap<&str, Vec<i64>>,
    created: &BTreeSet<ChangedRow>,
    deadline: Instant,
) -> Result<Vec<RemappedRow>, EngineError> {
    let mut remapped = Vec::new();
    for (table, rowids) in by_table {
        let columns = column_names(before, table);
        if columns.is_empty() || column_names(live, table).is_empty() {
            continue;
        }
        let quoted_table = quoted(table);
        let images = row_images(before, &quoted_table, &columns, rowids, deadline)?;
        // Which of this table's rows the transaction CREATED: they stand for no row of the
        // shared database, so the first pass passes over them.
        let own: BTreeSet<i64> = rowids
            .iter()
            .copied()
            .filter(|rowid| created.contains(&((*table).to_string(), *rowid)))
            .collect();
        for rowid in rowids {
            if own.contains(rowid) {
                continue;
            }
            out_of_time(deadline)?;
            live.execute(
                &format!("DELETE FROM {quoted_table} WHERE rowid = ?1"),
                [rowid],
            )
            .map_err(internal)?;
        }
        if images.is_empty() {
            continue;
        }
        let mut names: Vec<String> = vec!["rowid".to_string()];
        names.extend(columns.iter().map(|name| quoted(name)));
        let placeholders: Vec<String> = (1..=names.len()).map(|at| format!("?{at}")).collect();
        let statement = format!(
            "INSERT INTO {quoted_table} ({}) VALUES ({})",
            names.join(", "),
            placeholders.join(", ")
        );
        // The same row without its row id, for a created row whose row id the shared
        // database has since given to another writer's row: the engine assigns this one's,
        // which is what it does for every new row.
        let added = format!(
            "INSERT INTO {quoted_table} ({}) VALUES ({})",
            names[1..].join(", "),
            placeholders[..columns.len()].join(", ")
        );
        for (rowid, values) in images {
            out_of_time(deadline)?;
            if own.contains(&rowid) && holds_a_row_at(live, &quoted_table, rowid)? {
                live.execute(&added, rusqlite::params_from_iter(values))
                    .map_err(super::sql_engine::from_engine)?;
                remapped.push(((*table).to_string(), rowid, live.last_insert_rowid()));
                continue;
            }
            let mut bound: Vec<Value> = vec![Value::Integer(rowid)];
            bound.extend(values);
            live.execute(&statement, rusqlite::params_from_iter(bound))
                .map_err(internal)?;
        }
    }
    Ok(remapped)
}

/// Whether the shared database holds a row at `rowid` of `quoted_table` — which is what
/// decides whether a row the committing transaction created can keep the row id its own copy
/// gave it.
fn holds_a_row_at(
    live: &Connection,
    quoted_table: &str,
    rowid: i64,
) -> Result<bool, EngineError> {
    live.query_row(
        &format!("SELECT 1 FROM {quoted_table} WHERE rowid = ?1"),
        [rowid],
        |_| Ok(()),
    )
    .optional()
    .map(|found| found.is_some())
    .map_err(internal)
}

/// Whether the commit's budget has run out, as the error that says so.
fn out_of_time(deadline: Instant) -> Result<(), EngineError> {
    if Instant::now() >= deadline {
        return Err(EngineError::new(
            EngineErrorKind::Cancelled,
            "carrying a transaction's schema and rows over took longer than this endpoint serves",
        ));
    }
    Ok(())
}

/// The rows `rowids` as `source` holds them: the ones that are there, with their values in
/// the table's own column order.
fn row_images(
    before: &Connection,
    quoted_table: &str,
    columns: &[String],
    rowids: &[i64],
    deadline: Instant,
) -> Result<Vec<(i64, Vec<Value>)>, EngineError> {
    let mut statement = before
        .prepare(&format!("SELECT * FROM {quoted_table} WHERE rowid = ?1"))
        .map_err(internal)?;
    let mut images = Vec::new();
    for rowid in rowids {
        out_of_time(deadline)?;
        let mut rows = statement.query([rowid]).map_err(internal)?;
        let Some(row) = rows.next().map_err(internal)? else {
            continue;
        };
        let mut values = Vec::with_capacity(columns.len());
        for index in 0..columns.len() {
            values.push(row.get::<usize, Value>(index).map_err(internal)?);
        }
        images.push((*rowid, values));
    }
    Ok(images)
}

/// The columns of `table` on `conn`, in the engine's own order, or none for a table that
/// connection does not hold.
fn column_names(conn: &Connection, table: &str) -> Vec<String> {
    let Ok(statement) = conn.prepare(&format!("SELECT * FROM {} LIMIT 0", quoted(table))) else {
        return Vec::new();
    };
    statement.column_names().map(str::to_string).collect()
}

/// One identifier, quoted so a name that is also a keyword reaches the engine as a name.
fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// A commit the engine itself refused, which the caller reports as a failure of this
/// emulator rather than as the client's.
fn internal(error: rusqlite::Error) -> EngineError {
    EngineError::new(EngineErrorKind::Internal, error.to_string())
}
